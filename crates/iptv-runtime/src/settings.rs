//! Settings persistence and effective values, owned by the application runtime.
use super::file_writer::WriteError;
use crate::network::NetworkConfig;
use iptv_settings::keys::{self, BoolKey, DurationKey, EnumKey};
use iptv_settings::{Settings, SettingsError, Target, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

/// One owner's effective settings, shared with its workers.
pub(crate) type Shared = RwLock<Option<Arc<Settings>>>;

const FILE: &str = "settings.json";

const UNREADABLE: &str = "settings.unreadable.json";

/// The effective settings (defaults until [`load`]).
pub(crate) fn get(shared: &Shared) -> Arc<Settings> {
    shared
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_default()
}

fn replace(shared: &Shared, settings: Arc<Settings>) {
    *shared.write().unwrap_or_else(PoisonError::into_inner) = Some(settings);
}

fn path(directory: &Path) -> PathBuf {
    directory.join(FILE)
}

/// Loads the saved settings (startup).
pub(crate) fn load(owner: &super::Runtime) {
    let services = owner.services();
    let (settings, read_only) = read_file(services.files(), &path(services.directory()));
    owner.settings_read_only.set(read_only);
    let settings = Arc::new(settings);
    owner
        .settings_saved
        .with(|saved| *saved.borrow_mut() = Some((owner.settings_changed.get(), settings.clone())));
    replace(services.shared_settings(), settings);
}

/// The settings saved at `path` (or queued for it); defaults when there
/// are none. A file that cannot be read as settings is moved aside (never
/// overwritten); when that fails, the reason saving is disabled.
fn read_file(
    files: &super::file_writer::Files,
    path: &Path,
) -> (Settings, Option<std::io::ErrorKind>) {
    let text = match files.read(path).map(String::from_utf8) {
        Some(Ok(text)) => text,
        Some(Err(_)) => {
            log::warn!("iptv settings: invalid UTF-8; keeping the file aside");
            return (Settings::new(), preserve_unreadable(path));
        }
        None if !path.exists() => return (Settings::new(), None),
        _ => {
            log::warn!("iptv settings: cannot read the settings; using defaults");
            return (Settings::new(), preserve_unreadable(path));
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
            (settings, None)
        }
        Err(e) => {
            log::warn!("iptv settings: {e}; using defaults and keeping the file aside");
            (Settings::new(), preserve_unreadable(path))
        }
    }
}

/// Moves an unreadable settings file aside; `Some` when it could not be
/// kept, so nothing may be saved over it.
fn preserve_unreadable(path: &Path) -> Option<std::io::ErrorKind> {
    if !path.is_file() {
        return Some(std::io::ErrorKind::InvalidInput);
    }
    if let Some(dir) = path.parent()
        && let Err(error) = std::fs::rename(path, dir.join(UNREADABLE))
    {
        log::warn!("iptv settings: could not preserve unreadable file; saving disabled: {error}");
        return Some(error.kind());
    }
    None
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
    ParentalLocked,
    Storage(std::io::ErrorKind),
    /// Published to disk, but directory synchronization failed.
    Durability(std::io::ErrorKind),
}

/// What the UI must distinguish after an asynchronous save failure.
/// Detailed OS errors remain in diagnostics, outside user-facing messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveFailure {
    /// The write did not publish a replacement settings file.
    NotPublished,
    /// The replacement is live, but its directory synchronization failed.
    DurabilityUnconfirmed,
}

impl From<WriteError> for SaveFailure {
    fn from(error: WriteError) -> Self {
        match error {
            WriteError::Storage(_) => Self::NotPublished,
            WriteError::Durability(_) => Self::DurabilityUnconfirmed,
        }
    }
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
            Self::ParentalLocked => {
                f.write_str("Unlock parental controls before changing settings")
            }
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
pub(crate) fn set(
    owner: &std::rc::Rc<super::Runtime>,
    key: &str,
    value: Value,
) -> Result<(), SaveError> {
    set_in(owner, key, &Target::Global, value)
}

/// [`set`] at `at`: a playlist's own value overrides the global one there.
/// A value equal to what `at` would inherit removes its own value instead,
/// so later global changes apply to it again.
pub(crate) fn set_in(
    owner: &std::rc::Rc<super::Runtime>,
    key: &str,
    at: &Target,
    value: Value,
) -> Result<(), SaveError> {
    let protect_all = owner.settings().get("parental.require_pin_for").ok()
        == Some(Value::Enum("blocked_content_and_settings".into()));
    if (key.starts_with("parental.") || protect_all) && !owner.parental_settings_allowed() {
        return Err(SaveError::ParentalLocked);
    }
    apply(owner, key, |next| {
        if *at != Target::Global {
            let mut inheriting = next.clone();
            inheriting.clear_in(key, at)?;
            if inheriting.get_in(key, at)? == value {
                return next.clear_in(key, at);
            }
        }
        next.set_in(key, at, value)
    })
}

/// Drops the own settings of deleted playlist `source`: a later playlist
/// given the same id starts from the global ones.
pub(crate) fn forget_playlist(
    owner: &std::rc::Rc<super::Runtime>,
    source: iptv_library::SourceId,
) -> Result<(), SaveError> {
    let playlist = source.0.to_string();
    let mut without = (*owner.settings()).clone();
    without.remove_playlist(&playlist);
    if without == *owner.settings() {
        return Ok(());
    }
    apply(owner, "playlist_overrides", |next| {
        next.remove_playlist(&playlist);
        Ok(Vec::new())
    })
}

/// The own movie and series import switch (`playlists.include_vod`) of
/// every playlist in `playlists`, or (`None`) of every playlist not in
/// `kept`, is removed: a deletion whose settings save was lost (the app
/// stopped first, or the save failed) must not hand it to a new playlist
/// given the same id. Only this switch: the app sets no other playlist
/// value, and others may be prepared for a playlist before it is added.
pub(crate) fn forget_import_switches(
    owner: &std::rc::Rc<super::Runtime>,
    playlists: Option<&[iptv_library::SourceId]>,
    kept: &[iptv_library::SourceId],
) -> Result<(), SaveError> {
    let key = iptv_settings::keys::PLAYLISTS_INCLUDE_VOD.as_str();
    let kept: std::collections::BTreeSet<String> = kept.iter().map(|id| id.0.to_string()).collect();
    let settings = owner.settings();
    let targets: Vec<Target> = match playlists {
        Some(ids) => ids
            .iter()
            .map(|id| Target::playlist(id.0.to_string()))
            .collect::<Vec<_>>(),
        None => settings
            .playlists()
            .into_iter()
            .filter(|p| !kept.contains(*p))
            .map(Target::playlist)
            .collect::<Vec<_>>(),
    }
    .into_iter()
    .filter(|at| settings.stored(key, at).is_some())
    .collect();
    if targets.is_empty() {
        return Ok(());
    }
    apply(owner, key, |next| {
        let mut changes = Vec::new();
        for at in &targets {
            changes.extend(next.clear_in(key, at)?);
        }
        Ok(changes)
    })
}

/// Replaces the settings with what `edit` makes of them, tells the views
/// and saves in the background.
fn apply(
    owner: &std::rc::Rc<super::Runtime>,
    key: &str,
    edit: impl FnOnce(&mut Settings) -> Result<Vec<iptv_settings::Change>, SettingsError>,
) -> Result<(), SaveError> {
    if let Some(error) = owner.settings_read_only.get() {
        return Err(SaveError::Storage(error));
    }
    let services = owner.services();
    let mut next = (*services.settings()).clone();
    let changes = edit(&mut next).map_err(SaveError::Invalid)?;
    let json = next.to_json_string();
    let next = Arc::new(next);
    replace(services.shared_settings(), next.clone());
    if key.starts_with("parental.") {
        owner.parental_policy_changed();
    }
    if !changes.is_empty() {
        log::info!("iptv settings: {key} changed");
        owner.emit(super::RuntimeEvent::SettingsChanged);
    }
    let revision = owner.settings_changed.get() + 1;
    owner.settings_changed.set(revision);
    let saved = services
        .files()
        .write(path(services.directory()), json.into_bytes());
    let task_owner = owner.clone();
    owner.spawn(async move {
        let owner = task_owner;
        let result = saved.await;
        match settle(result) {
            Settled::Saved => owner.settings_saved.with(|saved| {
                let mut saved = saved.borrow_mut();
                if saved.as_ref().is_none_or(|(at, _)| *at < revision) {
                    *saved = Some((revision, next));
                }
            }),
            Settled::Failed if owner.settings_changed.get() == revision => {
                if let Some((_, previous)) =
                    owner.settings_saved.with(|saved| saved.borrow().clone())
                {
                    replace(services.shared_settings(), previous);
                    owner.emit(super::RuntimeEvent::SettingsChanged);
                }
            }
            Settled::Failed => {}
        }
        if let Err(error) = result {
            log::warn!("iptv settings: {}", SaveError::from(error));
            owner.emit(super::RuntimeEvent::SettingsSaveFailed(error.into()));
        }
    });
    Ok(())
}

pub fn flag(settings: &Settings, key: BoolKey) -> bool {
    settings.bool(key, &Target::Global)
}

pub fn duration_ms(settings: &Settings, key: DurationKey) -> i64 {
    settings.duration_ms(key, &Target::Global)
}

pub fn choice(settings: &Settings, key: EnumKey) -> &'static str {
    settings.choice(key, &Target::Global)
}

/// `general.user_agent`, when set.
pub fn user_agent(settings: &Settings) -> Option<String> {
    let ua = settings.text(keys::USER_AGENT, &Target::Global);
    (!ua.trim().is_empty()).then(|| ua.trim().to_owned())
}

/// The part of the provider network configuration that no setting changes
/// (limits, the development fixture override). Playback transport reads it
/// on player threads, which act for no owner.
pub fn base_network_config() -> NetworkConfig {
    #[cfg_attr(not(debug_assertions), allow(unused_mut))]
    let mut config = NetworkConfig::default();
    #[cfg(debug_assertions)]
    {
        config.allow_private_fixture =
            cfg!(any(test, feature = "test-support")) || super::imports::DEV_PLAYLIST.is_some();
    }
    config
}

/// The network settings of provider requests: [`base_network_config`] with
/// `general.user_agent` for requests that do not name their own.
pub fn network_config(settings: &Settings) -> NetworkConfig {
    let mut config = base_network_config();
    if let Some(ua) = user_agent(settings) {
        config.user_agent = ua;
    }
    config
}

/// [`base_network_config`] as the native adapter's configuration (trust
/// anchors included).
#[cfg(feature = "native")]
pub fn base_net_config() -> iptv_net::NetConfig {
    crate::native::network::net_config(&base_network_config())
}

/// [`network_config`] as the native adapter's configuration.
#[cfg(feature = "native")]
pub fn net_config(settings: &Settings) -> iptv_net::NetConfig {
    crate::native::network::net_config(&network_config(settings))
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
pub fn long_press_ms(settings: &Settings) -> u64 {
    u64::try_from(duration_ms(settings, keys::LONG_PRESS)).unwrap_or(0)
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    /// What a restart reads, with no write queued.
    fn read_file(path: &Path) -> Settings {
        super::read_file(
            &crate::file_writer::Files::new(std::sync::Arc::new(
                crate::native::execution::NativeExecution,
            )),
            path,
        )
        .0
    }
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
        assert_eq!(
            SaveFailure::from(result.unwrap_err()),
            SaveFailure::DurabilityUnconfirmed,
            "a published value must not be reported as unsaved"
        );
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
        assert_eq!(
            SaveFailure::from(result.unwrap_err()),
            SaveFailure::NotPublished
        );
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
