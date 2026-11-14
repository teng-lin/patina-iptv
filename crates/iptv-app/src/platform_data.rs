//! Platform-owned app storage location. Android supplies its actual user-scoped
//! files directory before the Rust root starts; package names are not paths.

#[cfg(test)]
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(target_os = "android")]
static ANDROID_FILES: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub(crate) fn data_dir() -> PathBuf {
    #[cfg(target_os = "android")]
    {
        ANDROID_FILES
            .get()
            .expect("Android host must initialize its files directory")
            .clone()
    }
    #[cfg(windows)]
    {
        home_directory(std::env::var_os("LOCALAPPDATA"))
            .expect("LOCALAPPDATA must identify an absolute directory before app startup")
            .join("Patina")
    }
    #[cfg(not(any(target_os = "android", windows)))]
    {
        let home = home_directory(std::env::var_os("HOME"))
            .expect("HOME must identify an absolute directory before app startup");
        if cfg!(any(target_os = "macos", target_os = "ios")) {
            home.join("Library/Application Support/iptv-rs")
        } else {
            home.join(".local/share/iptv-rs")
        }
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
    match std::panic::catch_unwind(|| {
        crate::runtime::flush_pending_writes(std::time::Duration::from_secs(1))
    }) {
        Ok(true) => {}
        Ok(false) => log::warn!("iptv files: queued writes did not finish before the stop"),
        Err(_) => log::error!("iptv files: waiting for queued writes panicked"),
    }
}

/// Called before opening settings, the database, restore state, or any worker.
/// The restrictive process mask also covers SQLite WAL/SHM and temporary files.
pub(crate) fn initialize() -> io::Result<()> {
    #[cfg(windows)]
    home_directory(std::env::var_os("LOCALAPPDATA"))?;
    #[cfg(not(any(target_os = "android", windows)))]
    home_directory(std::env::var_os("HOME"))?;
    #[cfg(unix)]
    // SAFETY: umask has no pointer arguments; making the process mask more
    // restrictive is intentional and happens before app storage workers start.
    unsafe {
        libc::umask(0o077);
    }
    protect_directory(&data_dir())
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
            initialize().unwrap();
            let root = data_dir();
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
