//! Settings persistence and effective values, owned by the application runtime.
use super::file_writer::WriteError;
use iptv_net::NetConfig;
use iptv_settings::keys::{self, BoolKey, DurationKey, EnumKey};
use iptv_settings::{Settings, SettingsError, Target, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock, Weak};

pub(crate) type Shared = RwLock<Option<Arc<Settings>>>;
pub(crate) type Current = Arc<Shared>;
thread_local! { static CONTEXT: RefCell<Option<Weak<Shared>>> = const { RefCell::new(None) }; }

pub(crate) fn current_handle() -> Current {
    CONTEXT
        .with(|context| context.borrow().as_ref().and_then(Weak::upgrade))
        .unwrap_or_else(|| super::store_service::service().settings.clone())
}

pub(crate) fn with_current<R>(current: Current, run: impl FnOnce() -> R) -> R {
    struct Restore(Option<Weak<Shared>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CONTEXT.set(self.0.take());
        }
    }
    let _restore = Restore(CONTEXT.replace(Some(Arc::downgrade(&current))));
    run()
}

const FILE: &str = "settings.json";

const UNREADABLE: &str = "settings.unreadable.json";

/// The effective settings (defaults until [`load`]).
pub fn get() -> Arc<Settings> {
    current_handle()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_default()
}

fn path() -> PathBuf {
    super::paths::data_dir().join(FILE)
}

/// Loads the saved settings (startup).
pub fn load() {
    crate::current().settings_read_only.set(None);
    let settings = Arc::new(read_file(&path()));
    crate::current().settings_saved.with(|saved| {
        *saved.borrow_mut() = Some((crate::current().settings_changed.get(), settings.clone()))
    });
    *current_handle()
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(settings);
}

/// The settings saved at `path` (or queued for it); defaults when there
/// are none. A file that cannot be read as settings is moved aside (never
/// overwritten).
fn read_file(path: &Path) -> Settings {
    let text = match super::file_writer::read(path).map(String::from_utf8) {
        Some(Ok(text)) => text,
        Some(Err(_)) => {
            log::warn!("iptv settings: invalid UTF-8; keeping the file aside");
            return preserve_unreadable(path);
        }
        None if !path.exists() => return Settings::new(),
        _ => {
            log::warn!("iptv settings: cannot read the settings; using defaults");
            return preserve_unreadable(path);
        }
    };
    match Settings::from_json(&text) {
        Ok((settings, report)) => {
            if !report.is_clean() {
                log::info!(
                    "iptv settings: loaded with {} unknown, {} adjusted and {} rejected entries",
                    report.unknown.len(),
                    report.clamped.len(),
                    report.rejected.len()
                );
            }
            settings
        }
        Err(e) => {
            log::warn!("iptv settings: {e}; using defaults and keeping the file aside");
            preserve_unreadable(path)
        }
    }
}

fn preserve_unreadable(path: &Path) -> Settings {
    if !path.is_file() {
        if let Some(runtime) = crate::try_current() {
            runtime
                .settings_read_only
                .set(Some(std::io::ErrorKind::InvalidInput));
        }
        return Settings::new();
    }
    if let Some(dir) = path.parent()
        && let Err(error) = std::fs::rename(path, dir.join(UNREADABLE))
    {
        log::warn!("iptv settings: could not preserve unreadable file; saving disabled: {error}");
        if let Some(runtime) = crate::try_current() {
            runtime.settings_read_only.set(Some(error.kind()));
        }
    }
    Settings::new()
}

/// What a finished save means for the live settings.
#[derive(Debug, PartialEq, Eq)]
enum Settled {
    /// The file holds them (perhaps not yet synced): they are the saved ones.
    Saved,
    /// The file still holds the previous ones.
    Failed,
}

fn settle(result: Result<(), WriteError>) -> Settled {
    match result {
        Ok(()) | Err(WriteError::Durability(_)) => Settled::Saved,
        Err(WriteError::Storage(_)) => Settled::Failed,
    }
}

/// Validation and persistence are distinct: a failed save is never success.
#[derive(Debug)]
pub enum SaveError {
    Invalid(SettingsError),
    Storage(std::io::ErrorKind),
    /// Published to disk, but directory synchronization failed.
    Durability(std::io::ErrorKind),
}

impl From<WriteError> for SaveError {
    fn from(error: WriteError) -> Self {
        match error {
            WriteError::Storage(kind) => Self::Storage(kind),
            WriteError::Durability(kind) => Self::Durability(kind),
        }
    }
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(error) => error.fmt(f),
            Self::Storage(kind) => write!(f, "Could not save settings ({kind})"),
            Self::Durability(kind) => write!(
                f,
                "Settings updated, but saving safely could not be confirmed ({kind})"
            ),
        }
    }
}

impl std::error::Error for SaveError {}

/// Stores `value` for `key` (globally), tells the views, and saves in the
/// background. Only an invalid value is an error here; a save that fails
/// later is reported, and the settings go back to the saved ones unless a
/// newer change is on its way to the file.
pub fn set(key: &str, value: Value) -> Result<(), SaveError> {
    if let Some(error) = crate::current().settings_read_only.get() {
        return Err(SaveError::Storage(error));
    }
    let mut next = (*get()).clone();
    let changes = next.set(key, value).map_err(SaveError::Invalid)?;
    let json = next.to_json_string();
    let next = Arc::new(next);
    *current_handle()
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(next.clone());
    if !changes.is_empty() {
        log::info!("iptv settings: {key} changed");
        crate::current().emit(super::RuntimeEvent::SettingsChanged);
    }
    let revision = crate::current().settings_changed.get() + 1;
    crate::current().settings_changed.set(revision);
    let saved = super::file_writer::write(path(), json.into_bytes());
    crate::current().spawn(async move {
        let result = saved.await;
        match settle(result) {
            Settled::Saved => crate::current().settings_saved.with(|saved| {
                let mut saved = saved.borrow_mut();
                if saved.as_ref().is_none_or(|(at, _)| *at < revision) {
                    *saved = Some((revision, next));
                }
            }),
            Settled::Failed if crate::current().settings_changed.get() == revision => {
                if let Some((_, previous)) = crate::current()
                    .settings_saved
                    .with(|saved| saved.borrow().clone())
                {
                    *current_handle()
                        .write()
                        .unwrap_or_else(PoisonError::into_inner) = Some(previous);
                    crate::current().emit(super::RuntimeEvent::SettingsChanged);
                }
            }
            Settled::Failed => {}
        }
        if let Err(error) = result {
            let error = SaveError::from(error);
            log::warn!("iptv settings: {error}");
            crate::current().emit(super::RuntimeEvent::Message(error.to_string()));
        }
    });
    Ok(())
}

pub fn flag(key: BoolKey) -> bool {
    get().bool(key, &Target::Global)
}

pub fn duration_ms(key: DurationKey) -> i64 {
    get().duration_ms(key, &Target::Global)
}

pub fn choice(key: EnumKey) -> &'static str {
    get().choice(key, &Target::Global)
}

/// `general.user_agent`, when set.
pub fn user_agent() -> Option<String> {
    let ua = get().text(keys::USER_AGENT, &Target::Global);
    (!ua.trim().is_empty()).then(|| ua.trim().to_owned())
}

/// The network settings of provider requests: `general.user_agent` for
/// requests that do not name their own.
pub fn net_config() -> NetConfig {
    let mut config = NetConfig::default();
    #[cfg(debug_assertions)]
    {
        config.allow_private_fixture =
            cfg!(feature = "test-support") || super::imports::DEV_PLAYLIST.is_some();
    }
    if let Some(ua) = user_agent() {
        config.user_agent = ua;
    }
    config
}

/// Adds `general.user_agent` to a stream's headers unless the stream
/// names its own (a playlist's per-channel agent wins).
pub fn add_user_agent(headers: &mut Vec<(String, String)>, ua: Option<String>) {
    let named = headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("user-agent"));
    if let (Some(ua), false) = (ua, named) {
        headers.push(("User-Agent".to_owned(), ua));
    }
}

/// `remote.long_press`, in ms.
pub fn long_press_ms() -> u64 {
    u64::try_from(duration_ms(keys::LONG_PRESS)).unwrap_or(0)
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    /// What the background writer does with a settings save.
    fn write_file(path: &Path, settings: &Settings) -> Result<(), WriteError> {
        super::super::file_writer::publish(path, settings.to_json_string().as_bytes(), |_| Ok(()))
    }

    #[test]
    fn settings_survive_a_save_and_an_unreadable_file_is_kept() {
        let dir = std::env::temp_dir().join(format!("iptv-settings-test-{}", std::process::id()));
        let path = dir.join(FILE);
        assert_eq!(read_file(&path), Settings::new(), "nothing saved yet");
        let mut s = Settings::new();
        s.set("catchup.forward_step", Value::secs(60)).unwrap();
        write_file(&path, &s).unwrap();
        assert_eq!(read_file(&path), s);
        std::fs::write(&path, "not settings").unwrap();
        assert_eq!(read_file(&path), Settings::new());
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(dir.join(UNREADABLE)).unwrap(),
            "not settings"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_utf8_is_preserved_before_a_later_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let original = b"synthetic settings \xff\xfe";
        std::fs::write(&path, original).unwrap();
        let mut settings = read_file(&path);
        assert_eq!(settings, Settings::new());
        settings
            .set("catchup.forward_step", Value::secs(90))
            .unwrap();
        write_file(&path, &settings).unwrap();
        assert_eq!(read_file(&path), settings);
        assert_eq!(
            std::fs::read(dir.path().join(UNREADABLE)).unwrap(),
            original
        );
    }

    #[test]
    fn directory_sync_failure_keeps_live_settings_aligned_with_published_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let live = Settings::new();
        write_file(&path, &live).unwrap();
        let mut next = live.clone();
        next.set("catchup.forward_step", Value::secs(90)).unwrap();
        let result =
            super::super::file_writer::publish(&path, next.to_json_string().as_bytes(), |_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "synthetic directory sync failure",
                ))
            });
        assert!(matches!(
            result.map_err(SaveError::from),
            Err(SaveError::Durability(_))
        ));
        assert_eq!(settle(result), Settled::Saved, "the live settings stay");
        assert_eq!(read_file(&path), next, "restart must read the live setting");
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn failed_write_keeps_the_previous_settings_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let mut saved = Settings::new();
        saved.set("catchup.forward_step", Value::secs(60)).unwrap();
        write_file(&path, &saved).unwrap();
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
        let mut changed = saved.clone();
        changed
            .set("catchup.forward_step", Value::secs(90))
            .unwrap();
        let result = write_file(&path, &changed);
        assert!(result.is_err());
        assert_eq!(settle(result), Settled::Failed, "the live settings go back");
        assert_eq!(read_file(&path), saved);
        std::fs::remove_dir(path.with_extension("json.tmp")).unwrap();
        write_file(&path, &changed).unwrap();
        assert_eq!(read_file(&path), changed);
    }

    #[test]
    fn unknown_entries_are_skipped_and_an_unreadable_path_gives_defaults() {
        let dir = std::env::temp_dir().join(format!("iptv-settings-extra-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        let mut s = Settings::new();
        s.set("catchup.forward_step", Value::secs(60)).unwrap();
        let mut json = s.to_json();
        json["global"]["synthetic.unknown"] = serde_json::Value::from(1);
        std::fs::write(&path, json.to_string()).unwrap();
        let read = read_file(&path);
        assert_eq!(read.get("catchup.forward_step"), Ok(Value::secs(60)));
        assert!(path.exists(), "a readable file stays");
        // A directory where the file should be: defaults, nothing moved.
        assert_eq!(read_file(&dir), Settings::new());
        assert!(dir.exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
