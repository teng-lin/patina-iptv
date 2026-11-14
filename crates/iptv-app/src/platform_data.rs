//! Platform-owned app storage location. Android supplies its actual user-scoped
//! files directory before the Rust root starts; package names are not paths.

#[cfg(all(test, unix))]
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(target_os = "android")]
static ANDROID_FILES: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub(crate) fn resolve_data_dir() -> io::Result<PathBuf> {
    #[cfg(target_os = "android")]
    {
        ANDROID_FILES.get().cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Android host has not initialized private storage",
            )
        })
    }
    #[cfg(windows)]
    {
        Ok(home_directory(std::env::var_os("LOCALAPPDATA"))?.join("Patina"))
    }
    #[cfg(not(any(target_os = "android", windows)))]
    {
        let home = home_directory(std::env::var_os("HOME"))?;
        Ok(if cfg!(any(target_os = "macos", target_os = "ios")) {
            home.join("Library/Application Support/iptv-rs")
        } else {
            home.join(".local/share/iptv-rs")
        })
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_TvActivity_initializeDataDirectory(
    mut env: jni::JNIEnv,
    _activity: jni::objects::JObject,
    path: jni::objects::JString,
) {
    // A panic must not unwind out of an `extern "system"` function into the
    // JVM (the process aborts): it is logged, without its message, and the
    // directory stays as it was.
    let set = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let path = match env.get_string(&path) {
            Ok(path) => PathBuf::from(String::from(path)),
            Err(_) => return, // JNI retains the conversion exception.
        };
        if !path.is_absolute() {
            let _ = env.throw_new(
                "java/lang/IllegalArgumentException",
                "Expected absolute files directory",
            );
            return;
        }
        if let Some(existing) = ANDROID_FILES.get() {
            if existing != &path {
                let _ = env.throw_new(
                    "java/lang/IllegalStateException",
                    "Files directory changed in one process",
                );
            }
        } else {
            let _ = ANDROID_FILES.set(path);
        }
    }));
    if set.is_err() {
        log::error!("iptv data: setting the files directory panicked");
    }
}

/// The Android host's `onStop`: the activity may be the process's last
/// moment, so it waits (at most a second) for queued file writes before
/// it lets the system stop it. NSH tells Rust about the move to the
/// background asynchronously, which the same wait in `data.rs` relies on.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_hantekllc_iptvrs_host_TvActivity_flushPendingWrites(
    _env: jni::JNIEnv,
    _activity: jni::objects::JObject,
) {
    // A panic must not unwind out of an `extern "system"` function into the
    // JVM (the process aborts): it is logged, without its message.
    match std::panic::catch_unwind(|| host_stop_flush(std::time::Duration::from_secs(1))) {
        Ok(true) => {}
        Ok(false) => log::warn!("iptv files: queued writes did not finish before the stop"),
        Err(_) => log::error!("iptv files: waiting for queued writes panicked"),
    }
}

/// The owner a host callback acts for: the one the application root
/// registered. A host callback runs off the application thread and outside
/// any task, so it cannot be handed the shell's handle; the root leaves this
/// `Send` one for it instead. It holds the owner weakly.
static HOST_STOP: std::sync::Mutex<Option<crate::runtime::HostHandle>> =
    std::sync::Mutex::new(None);

/// The application root's owner is the one a host stop flushes from now on.
pub(crate) fn flush_on_host_stop(owner: crate::runtime::HostHandle) {
    *HOST_STOP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(owner);
}

/// The registered root's owner; none before a root exists.
#[cfg(any(target_os = "android", test, feature = "test-support"))]
pub(crate) fn host_handle() -> Option<crate::runtime::HostHandle> {
    HOST_STOP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Waits, at most `limit`, for the registered owner's queued file writes.
#[cfg(any(target_os = "android", test))]
fn host_stop_flush(limit: std::time::Duration) -> bool {
    // A host can stop before the application root exists.
    host_handle().is_none_or(|owner| owner.flush_files(limit))
}

/// Called before opening settings, the database, restore state, or any worker.
/// The restrictive process mask also covers SQLite WAL/SHM and temporary files.
pub(crate) fn initialize() -> io::Result<PathBuf> {
    let directory = resolve_data_dir()?;
    #[cfg(unix)]
    // SAFETY: umask has no pointer arguments; making the process mask more
    // restrictive is intentional and happens before app storage workers start.
    unsafe {
        libc::umask(0o077);
    }
    protect_directory(&directory)?;
    Ok(directory)
}

#[cfg(not(target_os = "android"))]
fn home_directory(home: Option<std::ffi::OsString>) -> io::Result<PathBuf> {
    home.map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "App data base must be an absolute directory; refusing temporary storage",
            )
        })
}

/// Protect storage before opening application services.
pub(crate) fn protect_directory(path: &Path) -> io::Result<()> {
    crate::runtime::storage::protect_directory(path, exclude_from_backup)
}

/// User-initiated exports are separate; OS backups must not copy credentials.
pub(crate) fn exclude_from_backup(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "ios")]
    {
        use objc2_foundation::{NSNumber, NSString, NSURL, NSURLIsExcludedFromBackupKey};
        let name = path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Storage path is not UTF-8")
        })?;
        let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(name), true);
        let value = NSNumber::numberWithBool(true);
        // SAFETY: Apple's excluded-from-backup resource key expects an NSNumber
        // boolean; both Objective-C values live through this synchronous call.
        unsafe { url.setResourceValue_forKey_error(Some(&value), NSURLIsExcludedFromBackupKey) }
            .map_err(|_| io::Error::other("Could not exclude private storage from OS backup"))?;
    }
    #[cfg(not(target_os = "ios"))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_stop_waits_for_the_registered_owner_and_never_for_a_later_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let owner = |name: &str| {
            let path = directory.path().join(name);
            std::fs::create_dir(&path).unwrap();
            crate::runtime::Runtime::new(crate::runtime::Ports::new(path, |_| {
                panic!("a small-file write must not open the credential broker")
            }))
        };
        let root = owner("root");
        flush_on_host_stop(root.host_handle());
        // Constructing (and using) another runtime does not re-register it.
        let other = owner("other");
        root.save_combined_channel("generated-root-channel");
        other.save_combined_channel("generated-other-channel");
        let stopped = std::thread::spawn(|| host_stop_flush(std::time::Duration::from_secs(5)));
        assert!(stopped.join().unwrap());
        assert_eq!(
            std::fs::read_to_string(root.data_dir().join("last_channel_combined")).unwrap(),
            "generated-root-channel"
        );
        assert!(other.flush_files(std::time::Duration::from_secs(5)));
        // The registration does not keep the owner alive.
        drop(root);
        assert!(host_stop_flush(std::time::Duration::ZERO));
    }

    #[test]
    #[cfg(not(target_os = "android"))]
    fn bad_storage_displays_error_before_runtime_construction() {
        const CHILD: &str = "IPTV_TEST_STORAGE_FAILURE";
        const RAN: &str = "IPTV_TEST_STORAGE_FAILURE_RAN";
        if std::env::var_os(CHILD).is_some() {
            use nsh_core::element::Root;
            let mut app = nsh_testing::Headless::start(nsh_engine::EngineConfig::new(Root::of::<
                crate::App,
            >(
            )))
            .unwrap();
            let locale = std::env::var("IPTV_RS_TEST_LOCALE").unwrap();
            let language = iptv_core::i18n::resolve("system", Some(&locale));
            let message =
                iptv_core::i18n::translate(language, "Private app storage is unavailable");
            if locale == "de" {
                assert_ne!(message, "Private app storage is unavailable");
            }
            app.wait_for(&format!("text:{message}")).unwrap();
            assert!(!crate::has_runtime());
            std::fs::write(std::env::var_os(RAN).unwrap(), b"storage error shown").unwrap();
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let blocked = home.path().join("generated-file");
        std::fs::write(&blocked, b"synthetic non-directory").unwrap();
        let base = if cfg!(windows) {
            "LOCALAPPDATA"
        } else {
            "HOME"
        };
        for (mode, locale) in ["missing", "relative", "non-directory"]
            .into_iter()
            .flat_map(|mode| ["en", "de"].map(|locale| (mode, locale)))
        {
            let ran = home.path().join(format!("{mode}-{locale}-ran"));
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "platform_data::tests::bad_storage_displays_error_before_runtime_construction",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .current_dir(home.path())
                .env(CHILD, mode)
                .env("IPTV_RS_TEST_LOCALE", locale)
                .env(RAN, &ran)
                .env("HOME", home.path())
                .env("LOCALAPPDATA", home.path());
            match mode {
                "missing" => {
                    child.env_remove(base);
                }
                "relative" => {
                    child.env(base, "generated-relative-home");
                }
                "non-directory" => {
                    child.env(base, &blocked);
                }
                _ => unreachable!(),
            }
            let result = child.output().unwrap();
            assert!(
                result.status.success(),
                "{mode}/{locale}: {}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(std::fs::read(ran).unwrap(), b"storage error shown");
        }
    }

    #[test]
    #[cfg(not(target_os = "android"))]
    fn missing_or_relative_home_is_refused() {
        assert!(home_directory(None).is_err());
        assert!(home_directory(Some("".into())).is_err());
        assert!(home_directory(Some("relative".into())).is_err());
        assert_eq!(
            home_directory(Some(std::env::temp_dir().into_os_string())).unwrap(),
            std::env::temp_dir()
        );
    }

    #[cfg(unix)]
    #[test]
    fn new_storage_is_private_under_permissive_umask() {
        use std::os::unix::fs::PermissionsExt;
        const CHILD: &str = "PATINA_PRIVATE_STORAGE_TEST";
        if std::env::var_os(CHILD).is_some() {
            // SAFETY: this test runs alone in a separate process.
            unsafe {
                libc::umask(0o022);
            }
            let root = initialize().unwrap();
            let _store = rusqlite::Connection::open(root.join("iptv.sqlite")).unwrap();
            fs::write(root.join("copied.m3u"), b"#EXTM3U\n").unwrap();
            for entry in fs::read_dir(root).unwrap() {
                let entry = entry.unwrap();
                assert_eq!(
                    entry.metadata().unwrap().permissions().mode() & 0o777,
                    0o600,
                    "{}",
                    entry.path().display()
                );
            }
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform_data::tests::new_storage_is_private_under_permissive_umask",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HOME", home.path())
            .status()
            .unwrap();
        assert!(result.success());
    }

    #[cfg(unix)]
    #[test]
    fn repairs_existing_files_and_sidecars_without_following_links() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        fs::create_dir(&app).unwrap();
        fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
        for name in [
            "iptv.sqlite",
            "iptv.sqlite-wal",
            "iptv.sqlite-shm",
            "copied.m3u",
        ] {
            let path = app.join(name);
            fs::write(&path, b"synthetic").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        protect_directory(&app).unwrap();
        assert_eq!(
            fs::metadata(&app).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for entry in fs::read_dir(&app).unwrap() {
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let outside = root.path().join("outside");
        fs::write(&outside, b"synthetic").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&outside, app.join("link")).unwrap();
        assert!(protect_directory(&app).is_err());
        assert_eq!(
            fs::metadata(outside).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }
}
