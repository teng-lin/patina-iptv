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
    let settings = Arc::new(adopt_retired(settings));
    owner
        .settings_saved
        .with(|saved| *saved.borrow_mut() = Some((owner.settings_changed.get(), settings.clone())));
    replace(services.shared_settings(), settings);
}

/// Carries retired settings into the ones that replaced them, as the
/// settings load: at startup, which is also when a restored backup's
/// settings are first read. Nothing is rewritten until the next save, and
/// the retired entries stay, so an older build that reads the file keeps
/// its own meaning of them.
///
/// - `guide.stay_on_guide` and `guide.stay_on_list` (#281): one
///   `guide.after_choosing` replaces them. While it is not stored, either
///   one on stands for `stay`: the viewer chose to keep a list open, and
///   one choice now covers both.
pub(crate) fn adopt_retired(mut settings: Settings) -> Settings {
    let at = Target::Global;
    let stayed = settings.bool(keys::GUIDE_STAY_ON_GUIDE, &at)
        || settings.bool(keys::GUIDE_STAY_ON_LIST, &at);
    if stayed
        && settings
            .stored(keys::GUIDE_AFTER_CHOOSING.as_str(), &at)
            .is_none()
        && let Err(error) = settings.set(keys::GUIDE_AFTER_CHOOSING.as_str(), Value::choice("stay"))
    {
        log::warn!("iptv settings: could not carry the stay choice over: {error}");
    }
    settings
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
/// given the same id. Only this switch: other playlist values (its
/// streams at once among them) may be prepared for a playlist before it is
/// added.
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

/// Before #282 one streams-at-once ceiling (a global
/// `playlists.max_connections`) applied to every playlist. Each stored
/// playlist in `stored` without its own value takes it, then the global
/// value goes, so a playlist added later starts from its provider's
/// allowance. Runs when the library opens, so a value from a restored
/// backup (of any version, from any device) moves the same way. With no
/// playlist stored the global value stays until one is.
pub(crate) fn adopt_global_connection_limit(
    owner: &std::rc::Rc<super::Runtime>,
    stored: &[iptv_library::SourceId],
) -> Result<(), SaveError> {
    let key = iptv_settings::keys::PLAYLISTS_MAX_CONNECTIONS.as_str();
    let settings = owner.settings();
    let Some(global) = settings.stored(key, &Target::Global).cloned() else {
        return Ok(());
    };
    if stored.is_empty() {
        return Ok(());
    }
    apply(owner, key, |next| {
        let mut changes = Vec::new();
        for id in stored {
            let at = Target::playlist(id.0.to_string());
            if next.stored(key, &at).is_none() {
                changes.extend(next.set_in(key, &at, global.clone())?);
            }
        }
        changes.extend(next.clear_in(key, &Target::Global)?);
        Ok(changes)
    })
}

/// Stores `value` as `at`'s own value of `key`, even when it equals what
/// `at` would inherit, or (`None`) removes it. For a value whose absence
/// means something else (a playlist's streams at once: without one, the
/// provider's allowance applies).
pub(crate) fn set_own(
    owner: &std::rc::Rc<super::Runtime>,
    key: &'static str,
    at: &Target,
    value: Option<Value>,
) -> Result<(), SaveError> {
    let protect_all = owner.settings().get("parental.require_pin_for").ok()
        == Some(Value::Enum("blocked_content_and_settings".into()));
    if (key.starts_with("parental.") || protect_all) && !owner.parental_settings_allowed() {
        return Err(SaveError::ParentalLocked);
    }
    apply(owner, key, |next| {
        let before = next.stored(key, at).cloned();
        let mut changes = match value {
            Some(value) => next.set_in(key, at, value)?,
            None => next.clear_in(key, at)?,
        };
        // The resolved value can stay the same (Automatic to an explicit
        // value equal to the default): what is stored changed, so the
        // views that show it still hear of it.
        if changes.is_empty() && next.stored(key, at) != before.as_ref() {
            changes.push(iptv_settings::Change {
                key,
                target: at.clone(),
            });
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
    if !services.files().can_write() {
        // Neither a file writer nor a host keeps the files here: the
        // settings apply for this session.
        owner.note_session_only();
        return Ok(());
    }
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

#[cfg(feature = "native")]
pub use crate::native::network::{base_net_config, net_config};

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

#[cfg(all(test, feature = "native"))]
mod persistence_tests {
    use super::*;
    /// What a restart reads, with no write queued.
    fn read_file(path: &Path) -> Settings {
        super::read_file(
            &crate::file_writer::Files::new(
                std::sync::Arc::new(crate::native::execution::NativeExecution),
                None,
            ),
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

#[cfg(all(test, feature = "native"))]
mod host_tests {
    use super::*;

    struct Unpolled;
    impl crate::execution::CooperativeHost for Unpolled {
        fn spawn(&self, _: crate::execution::Task) {}
        fn delay(&self, _: std::time::Duration) -> crate::execution::Task {
            Box::pin(std::future::pending())
        }
        fn now(&self) -> std::time::Duration {
            std::time::Duration::ZERO
        }
    }

    #[test]
    fn without_a_file_writer_settings_apply_for_the_session() {
        let directory = tempfile::tempdir().unwrap();
        let mut ports = crate::Ports::for_tests();
        ports.directory = directory.path().to_owned();
        ports.execution = Arc::new(crate::execution::Cooperative::new(Arc::new(Unpolled)));
        let owner = std::rc::Rc::new(crate::Runtime::new(ports));
        owner.start_services();
        owner.load_settings();
        owner
            .set_setting(
                keys::USER_AGENT.as_str(),
                Value::Text("SyntheticAgent/2".into()),
            )
            .unwrap();
        assert_eq!(
            owner.settings().text(keys::USER_AGENT, &Target::Global),
            "SyntheticAgent/2"
        );
        assert!(!path(directory.path()).exists());
    }

    /// A browser's file port, in memory.
    #[derive(Default)]
    struct Kept(std::sync::Mutex<std::collections::HashMap<PathBuf, Vec<u8>>>);
    impl crate::file_host::FileHost for Kept {
        fn read(&self, path: &Path) -> Option<Vec<u8>> {
            self.0.lock().unwrap().get(path).cloned()
        }
        fn write(&self, path: &Path, content: Option<Vec<u8>>) -> crate::file_host::Stored {
            let mut files = self.0.lock().unwrap();
            match content {
                Some(bytes) => files.insert(path.to_owned(), bytes),
                None => files.remove(path),
            };
            Box::pin(async { true })
        }
    }

    #[test]
    fn with_a_file_host_settings_are_saved_and_read_at_the_next_launch() {
        let directory = tempfile::tempdir().unwrap();
        let kept = Arc::new(Kept::default());
        let launch = || {
            let mut ports = crate::Ports::for_tests();
            ports.directory = directory.path().to_owned();
            ports.execution = Arc::new(crate::execution::Cooperative::new(Arc::new(Unpolled)));
            ports.file_host = Some(kept.clone());
            let owner = std::rc::Rc::new(crate::Runtime::new(ports));
            let tasks = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let spawned = tasks.clone();
            owner.bind_tasks(
                move |task| spawned.borrow_mut().push(task),
                |_| Box::pin(std::future::pending()),
            );
            owner.start_services();
            owner.load_settings();
            (owner, tasks)
        };
        let (owner, tasks) = launch();
        let notices = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = notices.clone();
        owner.bind(move |event| {
            if matches!(event, crate::RuntimeEvent::Notify(_)) {
                seen.set(seen.get() + 1);
            }
        });
        owner
            .set_setting(
                keys::USER_AGENT.as_str(),
                Value::Text("SyntheticAgent/3".into()),
            )
            .unwrap();
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        for task in tasks.borrow_mut().iter_mut() {
            assert!(task.as_mut().poll(&mut context).is_ready());
        }
        assert_eq!(notices.get(), 0, "nothing tells the user changes are lost");
        assert!(crate::file_host::FileHost::read(&*kept, &path(directory.path())).is_some());
        assert!(
            !path(directory.path()).exists(),
            "the host keeps it, not the disk"
        );
        drop(owner);

        let (owner, _) = launch();
        assert_eq!(
            owner.settings().text(keys::USER_AGENT, &Target::Global),
            "SyntheticAgent/3"
        );
    }
}

#[cfg(test)]
mod adopt_tests {
    use super::*;

    fn after_choosing(settings: &Settings) -> Option<&Value> {
        settings.stored(keys::GUIDE_AFTER_CHOOSING.as_str(), &Target::Global)
    }

    #[test]
    fn a_stay_choice_carries_into_the_one_after_choosing() {
        // Nothing chosen: nothing is stored, the default (full screen) holds.
        assert_eq!(after_choosing(&adopt_retired(Settings::new())), None);
        // Either old switch on stands for staying; the old entry stays.
        for key in ["guide.stay_on_guide", "guide.stay_on_list"] {
            let mut old = Settings::new();
            old.set(key, Value::Bool(true)).unwrap();
            let adopted = adopt_retired(old);
            assert_eq!(after_choosing(&adopted), Some(&Value::choice("stay")));
            assert_eq!(adopted.get(key), Ok(Value::Bool(true)), "{key} is kept");
        }
        // Both off, as a viewer who turned one off again stored it.
        let mut off = Settings::new();
        off.set("guide.stay_on_guide", Value::Bool(false)).unwrap();
        assert_eq!(after_choosing(&adopt_retired(off)), None);
        // A choice made since wins over the old switches.
        let mut chosen = Settings::new();
        chosen.set("guide.stay_on_list", Value::Bool(true)).unwrap();
        chosen
            .set("guide.after_choosing", Value::choice("full_screen"))
            .unwrap();
        assert_eq!(
            after_choosing(&adopt_retired(chosen)),
            Some(&Value::choice("full_screen"))
        );
    }
}
