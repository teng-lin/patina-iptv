//! Whole-app snapshots. Restores are staged, then activated before any
//! settings, SQLite connection, import job or player is started. Directory
//! renames keep the previous state recoverable across an interrupted swap.

pub mod error;
mod instance;
use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::store_service::Services;

use iptv_settings::Settings;
use iptv_store_sqlite::Store;
use serde_json::json;

pub(crate) struct State {
    busy: Cell<bool>,
    progress: Cell<crate::model::BackupProgress>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            busy: Cell::new(false),
            progress: Cell::new(crate::model::BackupProgress::Idle),
        }
    }
}

const MAGIC: &[u8; 9] = b"PATINABK\x01";
/// Maximum number of set-aside corrupt library folders retained under `<data_dir>/recovered/`.
pub const MAX_RECOVERED_LIBRARIES: usize = 3;
const RECOVERED_README: &[u8] = b"Patina moved this unreadable library database aside so the app could start with a fresh library.\n\
Your settings, system keystore entries, and update state were kept in place.\n\
The files in this folder remain encrypted with your device's database key.\n";
use iptv_import::backup_manifest::{
    self, MAX_BYTES, MAX_FILES, MAX_MANIFEST, allowed, entry_limit,
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "android")]
pub fn next_document_request() -> String {
    SEQUENCE.fetch_add(1, Ordering::Relaxed).to_string()
}
use error::{Context, Error, Rejection, Result};
#[derive(Default)]
struct DirectoryStartup {
    outcome: OnceLock<std::result::Result<(), StartupError>>,
    resumed_settings_dirty: AtomicBool,
}
type Started = Arc<DirectoryStartup>;
/// The first activation outcome of each storage root in this process. A
/// process-lifetime fact about a directory, not owner state: every owner of
/// the same root shares it, and an owner of another root has its own.
static STARTED: Mutex<Option<HashMap<PathBuf, Started>>> = Mutex::new(None);

fn started_for(root: &Path) -> Started {
    STARTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_default()
        .entry(root.to_owned())
        .or_default()
        .clone()
}

pub(crate) fn take_resumed_settings_dirty(root: &Path) -> bool {
    started_for(root)
        .resumed_settings_dirty
        .swap(false, Ordering::SeqCst)
}

/// Why activating or recovering a restore failed at startup; `error` is
/// logged by the caller, its message shown when the failure is `fatal`.
#[derive(Clone, Copy)]
pub struct StartupError {
    pub error: Error,
    pub fatal: bool,
}

/// The one log line of a failed backup operation (its cause as a class,
/// without paths: [`error`]); what the user is told.
pub fn report(operation: &str, error: Error) -> &'static str {
    log::warn!("iptv backup: {operation} failed: {error}");
    error.message()
}
pub use crate::model::BackupProgress as Progress;

fn progress(runtime: &super::Runtime, phase: Progress) {
    runtime.backup.progress.set(phase);
    runtime.emit(super::RuntimeEvent::BackupProgress(phase));
}
/// One backup operation at a time per owner; ends when dropped.
pub struct Busy(std::rc::Rc<super::Runtime>);
impl Busy {
    pub(crate) fn begin(runtime: std::rc::Rc<super::Runtime>) -> Option<Self> {
        if runtime.backup.busy.replace(true) {
            runtime.emit(super::RuntimeEvent::Notify(
                "A backup operation is already running",
            ));
            None
        } else {
            progress(&runtime, Progress::Selecting);
            Some(Self(runtime))
        }
    }
}
impl Drop for Busy {
    fn drop(&mut self) {
        self.0.backup.busy.set(false);
        self.0.backup.progress.set(Progress::Idle);
        self.0
            .emit(super::RuntimeEvent::BackupProgress(Progress::Idle));
    }
}

impl Busy {
    /// Runs `job` off the UI thread with this owner's storage.
    pub async fn worker<T: Send + 'static>(
        &self,
        job: impl FnOnce(&Archive) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.0
            .parental_refresh()
            .await
            .map_err(|_| Error::from("Parental settings are unavailable"))?;
        if self.0.parental_has_pin() && !self.0.parental_settings_allowed() {
            return Err("Unlock parental controls before changing settings".into());
        }
        let (tx, rx) = futures_channel::oneshot::channel();
        progress(&self.0, Progress::Working);
        let archive = Archive {
            services: self.0.services(),
        };
        drop(archive.services.clone().spawn_blocking(move || {
            let _ = tx.send(job(&archive));
        }));
        rx.await.context("The backup task stopped")?
    }
}

/// The storage of the owner a backup operation was started for.
pub struct Archive {
    services: Services,
}
impl Archive {
    /// The owner's private storage directory.
    pub fn root(&self) -> &Path {
        self.services.directory()
    }
    /// Writes a whole-app backup of this owner to `destination`.
    pub fn export(&self, destination: &Path, settings: &str) -> Result<()> {
        export(&self.services, destination, settings)
    }
    /// Validates the backup read from `input` and schedules it for the
    /// next launch.
    pub fn stage(&self, input: &mut impl Read) -> Result<()> {
        stage(&self.services, input)
    }
    /// Calls a scheduled restore off.
    pub fn cancel(&self) -> Result<()> {
        cancel(self.root())
    }
}

/// A restore is scheduled for the next launch of `root`.
pub(crate) fn is_pending(root: &Path) -> bool {
    sibling(root, "restore-pending").exists()
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub use crate::storage::sibling;
fn scratch(services: &Services) -> Result<Scratch> {
    scratch_for_ports(services.ports())
}
fn scratch_for_ports(ports: &crate::Ports) -> Result<Scratch> {
    let path = sibling(
        &ports.directory,
        &format!(
            "backup-work-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ),
    );
    private_dir_with(&path, ports.exclude_from_backup)
        .context("Could not create temporary backup storage")?;
    Ok(Scratch(path))
}
fn private_dir_with(path: &Path, exclude: fn(&Path) -> io::Result<()>) -> io::Result<()> {
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    let mut builder = builder;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    let result = exclude(path);
    if result.is_err() {
        // Scratch ownership starts only after admission succeeds. This directory
        // was just created, but no guard exists yet to clean a refused one.
        // Remove only an empty directory, never an unexpected foreign entry.
        let _ = fs::remove_dir(path);
    }
    result
}
fn private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
#[cfg(not(windows))]
fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .context("Could not finish writing backup storage")
}

// Windows cannot FlushFileBuffers on a directory handle. Payload files are
// flushed before publication; recovery still uses the pending/previous trees.
#[cfg(windows)]
fn sync_dir(_: &Path) -> Result<()> {
    Ok(())
}

// Select from the standalone snapshot, not the live database: its references
// and its document list must describe the same committed library state.
enum DocumentCheck {
    Export,
    Restore,
}

fn referenced_documents(
    root: &Path,
    files: &Path,
    snapshot: &Store,
    operation: DocumentCheck,
) -> Result<BTreeSet<String>> {
    let (outside_storage, missing_file) = match operation {
        DocumentCheck::Export => (
            "A local playlist is outside application storage; add it with the file picker before backing up",
            "A local playlist file is missing; add it again before backing up",
        ),
        DocumentCheck::Restore => (
            "Backup references a local playlist outside application storage",
            "Backup is missing a local playlist file",
        ),
    };
    let mut names = BTreeSet::new();
    let mut include = |location: &str| -> Result<()> {
        let Some(local) = iptv_providers::content_url::local_path(location)
            .context("Backup contains an invalid local playlist URL")?
        else {
            return Ok(());
        };
        let relative = local.strip_prefix(root).context(outside_storage)?;
        let name = relative
            .to_str()
            .ok_or("Invalid local playlist path")?
            .replace(std::path::MAIN_SEPARATOR, "/");
        if !name.starts_with("playlists/")
            || !allowed(&name)
            || relative
                .components()
                .any(|part| super::private_files::temporary_name(part.as_os_str()))
        {
            return Err("Invalid local playlist path".into());
        }
        // Check every component, including the managed directory itself. A
        // linked parent must not let an otherwise valid reference escape it.
        let mut path = files.to_owned();
        for part in relative.components() {
            path.push(part);
            let kind = fs::symlink_metadata(&path)
                .context(missing_file)?
                .file_type();
            if kind.is_symlink() {
                return Err("Linked local playlist files cannot be backed up".into());
            }
            if path == files.join(relative) && !kind.is_file() {
                return Err("Invalid local playlist path".into());
            }
        }
        names.insert(name);
        if names.len() + 2 > MAX_FILES {
            return Err("Too many local playlist files".into());
        }
        Ok(())
    };
    for summary in snapshot
        .sources()
        .context("Could not check backup sources")?
    {
        let source = snapshot
            .source(summary.id)
            .context("Could not check backup source")?
            .ok_or("Backup source is missing")?;
        if let iptv_providers::model::SourceConfig::M3u(config) = source.source.config {
            include(config.location.expose())?;
        }
        for guide in snapshot
            .guide_sources(summary.id)
            .context("Could not check backup guide sources")?
        {
            include(guide.url.expose())?;
        }
        // Retained feed metadata can still reference a document after the
        // configured guide list has changed. Restore rebases these URLs too.
        if let Some(guide) = snapshot
            .epg_guide(summary.id)
            .context("Could not check backup guide sources")?
        {
            for feed in guide.feeds {
                include(feed.meta.config.url.expose())?;
            }
        }
    }
    Ok(names)
}

/// An online SQLite snapshot includes committed WAL data. The caller first
/// drains queued favorite/history writes and captures settings on the UI thread.
/// Unit tests own independent synthetic plaintext roots; they must never
/// reach an encrypted store lane or the operating-system credential broker.
#[cfg(test)]
fn snapshot_library(services: &Services, snapshot: &Path) -> Result<()> {
    Store::open(services.directory().join("iptv.sqlite"))
        .and_then(|store| store.snapshot_to(snapshot))
        .context("Could not snapshot the library")
}
#[cfg(not(test))]
fn snapshot_library(services: &Services, snapshot: &Path) -> Result<()> {
    let snapshot = snapshot.to_owned();
    services
        .blocking(move |store| store.snapshot_to(&snapshot))
        .context("Could not snapshot the library")
}

// Backup unit tests own independent plaintext roots. Production exports stream
// decrypted private chunks directly into the explicitly requested v1 archive.
#[cfg(test)]
fn document_reader(_: &Services, path: &Path) -> Result<(u64, Box<dyn Read>)> {
    let file = File::open(path).context("Could not read backup document")?;
    let len = file
        .metadata()
        .context("Could not read backup document")?
        .len();
    Ok((len, Box::new(file)))
}
#[cfg(not(test))]
fn document_reader(services: &Services, path: &Path) -> Result<(u64, Box<dyn Read>)> {
    let path = path.to_owned();
    let reader = services
        .blocking(move |store| store.read_private_file(&path))
        .context("Could not decrypt backup document")?;
    Ok((reader.len(), Box::new(reader)))
}

#[cfg(any(not(test), feature = "native"))]
fn seal_restore_with_store(
    store: &Store,
    directory: &Path,
) -> std::result::Result<(), iptv_store_sqlite::StoreError> {
    super::private_files::seal_tree(store, &directory.join("playlists"))?;
    store.seal_snapshot(&directory.join("iptv.sqlite"))
}
#[cfg(test)]
fn seal_restore(_: &Services, _: &Path) -> Result<()> {
    Ok(())
}
#[cfg(not(test))]
fn seal_restore(services: &Services, directory: &Path) -> Result<()> {
    let directory = directory.to_owned();
    services
        .blocking(move |store| seal_restore_with_store(store, &directory))
        .context("Could not encrypt the pending restore")
}
#[cfg(test)]
fn seal_restore_without_live_store(ports: &crate::Ports, directory: &Path) -> Result<()> {
    let path = directory.join("iptv.sqlite");
    if let Ok(key) = (ports.database_key)(&path) {
        super::credential_store::validate_staged_with_key(&path, &key)
            .context("Could not encrypt the pending restore")
    } else {
        Ok(())
    }
}
#[cfg(not(test))]
fn seal_restore_without_live_store(ports: &crate::Ports, directory: &Path) -> Result<()> {
    super::credential_store::validate_staged_database(ports, &directory.join("iptv.sqlite"))
        .context("Could not encrypt the pending restore")
}
#[cfg(test)]
fn validate_pending(
    ports: &crate::Ports,
    path: &Path,
) -> std::result::Result<(), iptv_store_sqlite::StoreError> {
    if let Ok(key) = (ports.database_key)(path) {
        let mut header = [0; 16];
        let plaintext = File::open(path)
            .and_then(|mut f| f.read_exact(&mut header))
            .is_ok_and(|()| &header == b"SQLite format 3\0");
        if !plaintext {
            return Store::validate_encrypted_snapshot(path, &key);
        }
    }
    Store::validate_snapshot(path)
}
#[cfg(not(test))]
fn validate_pending(
    ports: &crate::Ports,
    path: &Path,
) -> std::result::Result<(), iptv_store_sqlite::StoreError> {
    super::credential_store::validate_staged_database(ports, path)
}

#[cfg(test)]
fn adopt_host_recordings(
    _: &crate::Ports,
    host: &Path,
    pending: &Path,
) -> std::result::Result<(), iptv_store_sqlite::StoreError> {
    Store::open(pending)?.adopt_host_recordings(host)
}
#[cfg(not(test))]
fn adopt_host_recordings(
    ports: &crate::Ports,
    host: &Path,
    pending: &Path,
) -> std::result::Result<(), iptv_store_sqlite::StoreError> {
    super::credential_store::adopt_host_recordings(ports, host, pending)
}

fn remove_stale_work(root: &Path) -> Result<()> {
    let Some(parent) = root.parent() else {
        return Ok(());
    };
    let Some(name) = root.file_name().and_then(|n| n.to_str()) else {
        return Ok(());
    };
    let prefix = format!("{name}.backup-work-");
    let Ok(entries) = fs::read_dir(parent) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry.context("Could not inspect abandoned backup storage")?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let kind = entry
                .file_type()
                .context("Could not inspect abandoned backup storage")?;
            let result = if kind.is_dir() {
                fs::remove_dir_all(entry.path())
            } else {
                fs::remove_file(entry.path())
            };
            result.context("Could not remove abandoned backup storage")?;
        }
    }
    Ok(())
}

fn export(services: &Services, destination: &Path, settings: &str) -> Result<()> {
    let root = services.directory();
    validate_settings(settings)?;
    // Small files written in the background (a last channel) reach the disk
    // before they are copied.
    if !services.files().flush(std::time::Duration::from_secs(5)) {
        log::warn!("iptv backup: queued file writes did not finish; copying what is on disk");
    }
    let absolute_destination = if destination.is_absolute() {
        destination.to_owned()
    } else {
        std::env::current_dir()
            .context("Could not resolve backup destination")?
            .join(destination)
    };
    let parent = absolute_destination
        .parent()
        .ok_or("Invalid backup destination")?
        .canonicalize()
        .context("Could not open backup destination")?;
    let absolute_destination = parent.join(
        absolute_destination
            .file_name()
            .ok_or("Invalid backup destination")?,
    );
    let canonical_root = root
        .canonicalize()
        .context("Could not open application storage")?;
    let is_prepared_export = absolute_destination.parent() == Some(canonical_root.as_path())
        && absolute_destination
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("backup-export-") && n.ends_with(".patinabackup"));
    if absolute_destination.starts_with(&canonical_root) && !is_prepared_export {
        return Err("Choose a backup destination outside application storage".into());
    }
    let destination = absolute_destination.as_path();
    let work = scratch(services)?;
    let snapshot = work.0.join("iptv.sqlite");
    snapshot_library(services, &snapshot)?;
    let mut names = vec!["iptv.sqlite".to_owned(), "settings.json".to_owned()];
    let snapshot_store = Store::open(&snapshot).context("Could not check backup sources")?;
    names.extend(referenced_documents(
        root,
        root,
        &snapshot_store,
        DocumentCheck::Export,
    )?);
    drop(snapshot_store);
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries {
            let entry = entry.context("Could not read application files")?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if allowed(&name) && name != "iptv.sqlite" && name != "settings.json" {
                if !entry.file_type().is_ok_and(|t| t.is_file()) {
                    return Err("Linked application files cannot be backed up".into());
                }
                names.push(name);
            }
        }
    }
    names.sort();
    if names.len() > MAX_FILES {
        return Err("Too many local playlist files".into());
    }
    let mut entries = Vec::new();
    let mut total = 0u64;
    for name in &names {
        let len = if name == "settings.json" {
            settings.len() as u64
        } else if name.starts_with("playlists/") {
            document_reader(services, &root.join(name))?.0
        } else {
            fs::metadata(if name == "iptv.sqlite" {
                work.0.join(name)
            } else {
                root.join(name)
            })
            .context("Could not read backup files")?
            .len()
        };
        entry_limit(name, len).context("Backup structured entry is too large")?;
        total = total.checked_add(len).ok_or("Backup is too large")?;
        if total > MAX_BYTES {
            return Err("Backup is too large".into());
        }
        entries.push(json!({"name": name, "length": len}));
    }
    let root_text = root.to_str().ok_or("Invalid application storage path")?;
    let manifest = serde_json::to_vec(
        &json!({"format": "patina-whole-app", "version": 1, "root": root_text, "files": entries}),
    )
    .context("Could not encode backup")?;
    if manifest.len() as u64 > MAX_MANIFEST {
        return Err("Backup file list is too large".into());
    }
    let temporary = sibling(
        destination,
        &format!(
            "writing-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ),
    );
    let result = (|| {
        let mut output = private_file(&temporary).context("Could not create the backup file")?;
        output
            .write_all(MAGIC)
            .and_then(|_| output.write_all(&(manifest.len() as u64).to_le_bytes()))
            .and_then(|_| output.write_all(&manifest))
            .context("Could not write the backup file")?;
        for (name, entry) in names.iter().zip(&entries) {
            if name == "settings.json" {
                output
                    .write_all(settings.as_bytes())
                    .context("Could not write settings")?;
            } else {
                let path = if name == "iptv.sqlite" {
                    work.0.join(name)
                } else {
                    root.join(name)
                };
                let expected = entry["length"].as_u64().unwrap();
                let (length, reader): (u64, Box<dyn Read>) = if name.starts_with("playlists/") {
                    document_reader(services, &path)?
                } else {
                    let file = File::open(path).context("Could not read backup files")?;
                    let length = file
                        .metadata()
                        .context("Could not read backup files")?
                        .len();
                    (length, Box::new(file))
                };
                if length != expected {
                    return Err("Application files changed; try the backup again".into());
                }
                let count = io::copy(&mut reader.take(expected), &mut output)
                    .context("Could not write backup files")?;
                if count != expected {
                    return Err("Application files changed; try the backup again".into());
                }
            }
        }
        output
            .sync_all()
            .context("Could not finish writing the backup")?;
        fs::rename(&temporary, destination).context("Could not save the backup file")?;
        sync_dir(destination.parent().unwrap_or(Path::new(".")))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn read_small_file(path: &Path, name: &str) -> Result<String> {
    let maximum = if name == "last_channel" {
        64 << 10
    } else {
        1 << 20
    };
    if !fs::symlink_metadata(path)
        .context("Could not read backup file size")?
        .file_type()
        .is_file()
    {
        return Err("Backup structured entry is not a regular file".into());
    }
    let file = File::open(path).context("Could not read backup settings")?;
    entry_limit(
        name,
        file.metadata()
            .context("Could not read backup file size")?
            .len(),
    )
    .context("Backup structured entry is too large")?;
    let mut text = String::new();
    file.take(maximum + 1)
        .read_to_string(&mut text)
        .context("Could not read backup settings")
        .map_err(Error::damaged)?;
    entry_limit(name, text.len() as u64)
        .context("Backup structured entry is too large")
        .map_err(Error::damaged)?;
    Ok(text)
}

fn validate_settings(text: &str) -> Result<()> {
    entry_limit("settings.json", text.len() as u64)
        .context("Backup structured entry is too large")?;
    // Inspect the recognized header before interpreting current settings layers:
    // a future version may have changed their shape entirely.
    let document: serde_json::Value = serde_json::from_str(text)
        .context("Backup settings are invalid or from a newer version")
        .map_err(Error::damaged)?;
    let format = document
        .get("format")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            Error::from("Backup settings are invalid or from a newer version").damaged()
        })?;
    if format != iptv_settings::backup::FORMAT {
        return Err(Error::rejected(
            "Backup settings are not supported by this version",
            Rejection::Settings {
                found: None,
                supported: iptv_settings::backup::VERSION.into(),
            },
        ));
    }
    let version = document
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .filter(|version| *version > 0)
        .ok_or_else(|| {
            Error::from("Backup settings are invalid or from a newer version").damaged()
        })?;
    if version > u64::from(iptv_settings::backup::VERSION) {
        return Err(Error::rejected(
            "Backup settings are not supported by this version",
            Rejection::Settings {
                found: Some(version),
                supported: iptv_settings::backup::VERSION.into(),
            },
        ));
    }
    let (_, report) = Settings::from_json_value(document)
        .context("Backup settings are invalid or from a newer version")
        .map_err(Error::damaged)?;
    // An older document is migrated like the live settings file at startup,
    // so a backup made before a settings version bump still restores.
    let migrated = iptv_settings::LoadReport {
        migrations: Vec::new(),
        ..report
    };
    if !migrated.is_clean() {
        return Err(Error::rejected(
            "Backup settings are not supported by this version",
            Rejection::Settings {
                found: Some(version),
                supported: iptv_settings::backup::VERSION.into(),
            },
        ));
    }
    Ok(())
}

/// Validates all content before replacing an existing pending restore. No
/// running database or settings file is changed by staging or cancellation.
fn stage(services: &Services, input: &mut impl Read) -> Result<()> {
    stage_with(services.ports(), input, |work| seal_restore(services, work))
}

fn stage_into_scratch_without_live_store(
    ports: &crate::Ports,
    input: &mut impl Read,
) -> Result<Scratch> {
    stage_into_scratch(ports, input, |work| {
        seal_restore_without_live_store(ports, work)
    })
}

fn stage_with(
    ports: &crate::Ports,
    input: &mut impl Read,
    seal: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let work = stage_into_scratch(ports, input, seal)?;
    publish_pending_restore(&ports.directory, work)
}

fn stage_into_scratch(
    ports: &crate::Ports,
    input: &mut impl Read,
    seal: impl FnOnce(&Path) -> Result<()>,
) -> Result<Scratch> {
    let root = ports.directory.as_path();
    let work = scratch_for_ports(ports)?;
    let mut magic = [0; 9];
    input
        .read_exact(&mut magic)
        .context("This is not a complete Patina backup")
        .map_err(Error::damaged)?;
    if magic[..8] != MAGIC[..8] {
        return Err(Error::rejected(
            "This is not a supported Patina backup",
            Rejection::Format {
                found: None,
                supported: 1,
            },
        ));
    }
    if magic[8] == 0 {
        return Err(Error::from("Backup header is invalid").damaged());
    }
    if magic[8] != MAGIC[8] {
        return Err(Error::rejected(
            "This is not a supported Patina backup",
            Rejection::Format {
                found: Some(magic[8].into()),
                supported: 1,
            },
        ));
    }
    let mut size = [0; 8];
    input
        .read_exact(&mut size)
        .context("Backup header is incomplete")
        .map_err(Error::damaged)?;
    let size = u64::from_le_bytes(size);
    if size > MAX_MANIFEST {
        return Err(Error::from("Backup header is too large").damaged());
    }
    let mut bytes = vec![0; size as usize];
    input
        .read_exact(&mut bytes)
        .context("Backup header is incomplete")
        .map_err(Error::damaged)?;
    let header: serde_json::Value = serde_json::from_slice(&bytes)
        .context("Backup header is invalid")
        .map_err(Error::damaged)?;
    let format = header
        .get("format")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::from("Backup header is invalid").damaged())?;
    if format != "patina-whole-app" {
        return Err(Error::rejected(
            "Backup format is not supported by this version",
            Rejection::Format {
                found: None,
                supported: 1,
            },
        ));
    }
    let version = header
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .filter(|version| *version > 0)
        .ok_or_else(|| Error::from("Backup header is invalid").damaged())?;
    if version != 1 {
        return Err(Error::rejected(
            "Backup format is not supported by this version",
            Rejection::Format {
                found: Some(version),
                supported: 1,
            },
        ));
    }
    let manifest =
        backup_manifest::parse(&bytes).map_err(|message| Error::from(message).damaged())?;
    let old_root = Path::new(&manifest.root);
    for entry in &manifest.files {
        let name = &entry.name;
        let len = entry.length;
        let path = work.0.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Could not stage backup files")?;
        }
        let mut output = private_file(&path).context("Could not stage backup files")?;
        if io::copy(&mut input.take(len), &mut output)
            .context("Could not read backup files")
            .map_err(Error::damaged)?
            != len
        {
            return Err(Error::from("Backup is incomplete").damaged());
        }
        output
            .sync_all()
            .context("Could not finish staging backup files")?;
    }
    let mut extra = [0];
    if input.read(&mut extra).context("Could not read backup")? != 0 {
        return Err(Error::from("Backup has unexpected trailing data").damaged());
    }
    let settings = read_small_file(&work.0.join("settings.json"), "settings.json")?;
    validate_settings(&settings)?;
    Store::validate_snapshot(&work.0.join("iptv.sqlite")).map_err(|error| {
        if let iptv_store_sqlite::StoreError::SchemaTooNew { found, supported } = error {
            Error::rejected(
                "Backup library is invalid or from a newer version",
                Rejection::Database { found, supported },
            )
        } else {
            let content = error.is_snapshot_content_error();
            let error = Error::new("Could not check backup library", error);
            if content { error.damaged() } else { error }
        }
    })?;
    {
        let mut store =
            Store::open(work.0.join("iptv.sqlite")).context("Could not open backup library")?;
        store
            .rebase_local_playlists(old_root, root)
            .map_err(|error| {
                let invalid = matches!(error, iptv_store_sqlite::StoreError::InvalidData(_));
                let error = Error::new("Backup contains unavailable local playlist files", error);
                if invalid { error.damaged() } else { error }
            })?;
        store
            .invalidate_guide_repair_authority()
            .context("Could not check backup library")?;
        store
            .discard_recording_jobs()
            .context("Could not clear host recording jobs")?;
    }
    // Check rewritten local references point to files included in this archive.
    let store =
        Store::open(work.0.join("iptv.sqlite")).context("Could not check backup library")?;
    referenced_documents(root, &work.0, &store, DocumentCheck::Restore)
        .map_err(Error::invalid_reference)?;
    drop(store);
    // All archive validation and reference rewriting succeeded. Encrypt the
    // persistent pending tree before its rename can make it survive restart.
    seal(&work.0)?;
    Ok(work)
}

fn publish_pending_restore(root: &Path, work: Scratch) -> Result<()> {
    publish_staged_path(root, &work.0)
}

fn publish_staged_path(root: &Path, staged: &Path) -> Result<()> {
    // Close/checkpoint WAL before directory swap; SQLite open on next launch
    // sees a complete database, and no staging handles remain.
    recover_replacement(root)?;
    let pending = sibling(root, "restore-pending");
    let replaced = sibling(root, "restore-replaced");
    if replaced.exists() {
        fs::remove_dir_all(&replaced).context("Could not replace an older pending restore")?;
    }
    if pending.exists() {
        fs::rename(&pending, &replaced).context("Could not replace the pending restore")?;
    }
    if let Err(error) = fs::rename(staged, &pending) {
        if replaced.exists() {
            let _ = fs::rename(&replaced, &pending);
        }
        return Err(Error::new("Could not schedule the restore", error));
    }
    sync_dir(root.parent().ok_or("Application storage has no parent")?)?;
    let _ = fs::remove_dir_all(replaced);
    Ok(())
}

fn recover_replacement(root: &Path) -> Result<()> {
    let pending = sibling(root, "restore-pending");
    let replaced = sibling(root, "restore-replaced");
    if replaced.exists() {
        if pending.exists() {
            fs::remove_dir_all(replaced)
                .context("Could not clean up the replaced pending restore")?;
        } else {
            fs::rename(replaced, pending).context("Could not recover the pending restore")?;
        }
    }
    Ok(())
}

fn cancel(root: &Path) -> Result<()> {
    // Remove fallback first, so a crash during cancellation cannot resurrect
    // a superseded scheduled restore on the next launch.
    let replaced = sibling(root, "restore-replaced");
    if replaced.exists() {
        fs::remove_dir_all(replaced).context("Could not cancel the pending restore")?;
    }
    let pending = sibling(root, "restore-pending");
    if pending.exists() {
        fs::remove_dir_all(pending).context("Could not cancel the pending restore")?;
    }
    Ok(())
}

/// Called before opening any file under `root`: this process holds the
/// storage for as long as it lives.
pub(crate) fn lock(root: &Path) -> std::result::Result<(), Error> {
    instance::acquire(root).inspect_err(|error| {
        log::warn!("iptv backup: locking application storage failed: {error}");
    })
}

/// Called before opening any file under the owner's storage. If a crash
/// happened after moving the original aside, finish the validated restore,
/// or recover it.
pub(crate) fn startup(ports: &crate::Ports) -> std::result::Result<(), StartupError> {
    let root = &ports.directory;
    let started = started_for(root);
    // Keep the first outcome, including a fatal recovery failure. Recreating
    // an engine must neither rerun activation nor bypass failed recovery.
    *started.outcome.get_or_init(|| {
        activate(ports, root).map_err(|error| StartupError {
            error,
            fatal: !root.exists(),
        })
    })
}

fn activate(ports: &crate::Ports, root: &Path) -> Result<()> {
    crate::storage::with_directory_replacement(root, || activate_locked(ports, root))
}

fn reconcile_previous_restore(root: &Path) -> Result<()> {
    let previous = sibling(root, "restore-previous");
    if previous.exists() {
        if root.join("iptv.sqlite").is_file() {
            // The validated tree was renamed into place before interruption.
            preserve_non_backup_entries(&previous, root)?;
            fs::remove_dir_all(&previous).context("Could not finish the previous restore")?;
            let recovered_root = root.join("recovered");
            if recovered_root.is_dir() {
                let preserved = latest_recovered_slot(root);
                prune_recovered_copies(
                    &recovered_root,
                    MAX_RECOVERED_LIBRARIES,
                    preserved.as_deref(),
                );
                let _ = sync_dir_io(&recovered_root);
            }
        } else {
            // Platform initialization may recreate the missing directory before
            // recovery runs. Its existence is not an activation commit marker.
            if root.exists() {
                fs::remove_dir_all(root).context("Could not recover application storage")?;
            }
            fs::rename(&previous, root).context("Could not recover application storage")?;
        }
    }
    Ok(())
}

pub(crate) fn has_interrupted_corrupt_restore(root: &Path) -> bool {
    sibling(root, "restore-corrupt-active").exists()
}

pub(crate) fn resume_interrupted_corrupt_restore(
    ports: &crate::Ports,
    root: &Path,
) -> std::result::Result<(), iptv_store_sqlite::StoreError> {
    match activate_locked_outcome(ports, root) {
        Ok(_) => Ok(()),
        Err(ActivateFailure::PreInstallStore(store_err)) => Err(store_err),
        Err(ActivateFailure::PreInstall(_) | ActivateFailure::PostInstall(_)) => {
            Err(iptv_store_sqlite::StoreError::Unavailable(
                iptv_core::message::key("The saved library could not be opened"),
            ))
        }
    }
}

enum ActiveMarker {
    Commit(PathBuf),
    Rollback(PathBuf),
}

fn read_active_marker(root: &Path, active_marker: &Path) -> Option<ActiveMarker> {
    let raw = fs::read_to_string(active_marker).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("rollback:") {
        if rest.is_empty() {
            return None;
        }
        Some(ActiveMarker::Rollback(root.join("recovered").join(rest)))
    } else {
        let slot = root.join("recovered").join(trimmed);
        slot.join("iptv.sqlite")
            .is_file()
            .then_some(ActiveMarker::Commit(slot))
    }
}

#[cfg(not(windows))]
fn sync_dir_io(path: &Path) -> io::Result<()> {
    File::open(path).and_then(|f| f.sync_all())
}

#[cfg(windows)]
fn sync_dir_io(_: &Path) -> io::Result<()> {
    Ok(())
}

fn write_sync_marker(path: &Path, content: &[u8]) -> io::Result<()> {
    let temp = sibling(path, "writing");
    let _ = fs::remove_file(&temp);
    let mut file = private_file(&temp)?;
    file.write_all(content)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, path)?;
    if let Some(parent) = path.parent() {
        sync_dir_io(parent)?;
    }
    Ok(())
}

fn rollback_rejected_resumed_restore(
    root: &Path,
    slot: &Path,
    active_corrupt: &Path,
    prior: &Path,
    pending: &Path,
) {
    let slot_text = slot.file_name().unwrap_or_default().to_string_lossy();
    let _ = cancel(root);
    let _ = write_sync_marker(active_corrupt, format!("rollback:{slot_text}").as_bytes());
    if rollback_set_aside(&root.join("iptv.sqlite"), slot).is_ok() {
        let promoted = if prior.exists() && !pending.exists() {
            fs::rename(prior, pending).is_ok()
        } else {
            true
        };
        if promoted {
            let _ = fs::remove_file(active_corrupt);
        }
    }
}

enum ActivateOutcome {
    NoPendingWork,
    Installed {
        was_corrupt_restore: bool,
        active_slot: Option<PathBuf>,
    },
}

enum ActivateFailure {
    PreInstallStore(iptv_store_sqlite::StoreError),
    PreInstall(Error),
    PostInstall(Error),
}

impl ActivateFailure {
    fn into_error(self) -> Error {
        match self {
            Self::PreInstallStore(store_err) => Error::new(
                "The pending backup library is invalid; current data was kept",
                store_err,
            ),
            Self::PreInstall(error) | Self::PostInstall(error) => error,
        }
    }
}

fn activate_locked(ports: &crate::Ports, root: &Path) -> Result<()> {
    activate_locked_outcome(ports, root)
        .map(|_| ())
        .map_err(ActivateFailure::into_error)
}

fn activate_locked_outcome(
    ports: &crate::Ports,
    root: &Path,
) -> std::result::Result<ActivateOutcome, ActivateFailure> {
    let outcome = prepare_and_install_locked(ports, root)?;
    if let ActivateOutcome::Installed {
        was_corrupt_restore,
        ref active_slot,
    } = outcome
    {
        finish_installed_locked(root, was_corrupt_restore, active_slot.as_deref())
            .map_err(ActivateFailure::PostInstall)?;
    }
    Ok(outcome)
}

fn prepare_and_install_locked(
    ports: &crate::Ports,
    root: &Path,
) -> std::result::Result<ActivateOutcome, ActivateFailure> {
    remove_stale_work(root).map_err(ActivateFailure::PreInstall)?;
    recover_replacement(root).map_err(ActivateFailure::PreInstall)?;
    // No native export is active before App starts. A killed process can leave
    // a private prepared archive; never retain those credential-bearing copies.
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if (name.starts_with("backup-export-") || name.starts_with("backup-import-"))
                && name.ends_with(".patinabackup")
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    reconcile_previous_restore(root).map_err(ActivateFailure::PreInstall)?;
    let prior = sibling(root, "restore-prior");
    let staged_corrupt = sibling(root, "restore-corrupt-staged");
    let active_corrupt = sibling(root, "restore-corrupt-active");
    let pending = sibling(root, "restore-pending");
    let previous = sibling(root, "restore-previous");
    let active_state = read_active_marker(root, &active_corrupt);
    if root.join("iptv.sqlite").is_file() {
        if staged_corrupt.exists() {
            let _ = fs::remove_dir_all(&staged_corrupt);
        }
        if matches!(active_state, Some(ActiveMarker::Rollback(_))) {
            if prior.exists() && !pending.exists() {
                fs::rename(&prior, &pending)
                    .context("Could not recover the pending restore")
                    .map_err(ActivateFailure::PreInstall)?;
            }
            let _ = fs::remove_file(&active_corrupt);
            return Ok(ActivateOutcome::NoPendingWork);
        }
        let _ = fs::remove_file(&active_corrupt);
        if prior.exists() {
            let _ = fs::remove_dir_all(&prior);
        }
    } else {
        if let Some(ActiveMarker::Rollback(slot)) = &active_state {
            if staged_corrupt.exists() {
                let _ = fs::remove_dir_all(&staged_corrupt);
            }
            let _ = cancel(root);
            rollback_set_aside(&root.join("iptv.sqlite"), slot)
                .context("Could not move the damaged library aside")
                .map_err(ActivateFailure::PreInstall)?;
            if prior.exists() && !pending.exists() {
                fs::rename(&prior, &pending)
                    .context("Could not recover the pending restore")
                    .map_err(ActivateFailure::PreInstall)?;
            }
            let _ = fs::remove_file(&active_corrupt);
            return Ok(ActivateOutcome::NoPendingWork);
        }
        finish_interrupted_sidecar_recovery(root, &root.join("iptv.sqlite"))
            .context("Could not move the damaged library aside")
            .map_err(ActivateFailure::PreInstall)?;
        if staged_corrupt.exists() {
            if pending.exists() && !prior.exists() {
                fs::rename(&pending, &prior)
                    .context("Could not schedule the restore")
                    .map_err(ActivateFailure::PreInstall)?;
            }
            publish_staged_path(root, &staged_corrupt).map_err(ActivateFailure::PreInstall)?;
        } else if !active_corrupt.exists() && prior.exists() && !pending.exists() {
            let _ = fs::rename(&prior, &pending);
        }
    }
    let active_slot = match active_state {
        Some(ActiveMarker::Commit(slot)) => Some(slot),
        _ => None,
    };
    if !pending.exists() {
        if !root.exists() && previous.exists() {
            fs::rename(previous, root)
                .context("Could not recover application storage")
                .map_err(ActivateFailure::PreInstall)?;
        }
        if active_corrupt.exists() && !root.join("iptv.sqlite").is_file() {
            if let Some(slot) = &active_slot {
                rollback_set_aside(&root.join("iptv.sqlite"), slot)
                    .context("Could not move the damaged library aside")
                    .map_err(ActivateFailure::PreInstall)?;
                if prior.exists() {
                    fs::rename(&prior, &pending)
                        .context("Could not recover the pending restore")
                        .map_err(ActivateFailure::PreInstall)?;
                }
                let _ = fs::remove_file(&active_corrupt);
                return Ok(ActivateOutcome::NoPendingWork);
            }
            return Err(ActivateFailure::PreInstall(
                "Could not move the damaged library aside".into(),
            ));
        }
        let _ = fs::remove_file(&active_corrupt);
        return Ok(ActivateOutcome::NoPendingWork);
    }
    // Revalidate persisted staging before moving the working state aside.
    for entry in fs::read_dir(&pending)
        .context("Could not read pending backup")
        .map_err(ActivateFailure::PreInstall)?
    {
        let entry = entry
            .context("Could not read pending backup entry")
            .map_err(ActivateFailure::PreInstall)?;
        if let Some(name) = entry.file_name().to_str() {
            let len = entry
                .metadata()
                .context("Could not read backup file size")
                .map_err(ActivateFailure::PreInstall)?
                .len();
            if let Err(err) = entry_limit(name, len) {
                if let Some(slot) = &active_slot {
                    rollback_rejected_resumed_restore(
                        root,
                        slot,
                        &active_corrupt,
                        &prior,
                        &pending,
                    );
                }
                return Err(ActivateFailure::PreInstall(Error::new(
                    "Backup structured entry is too large",
                    err,
                )));
            }
        }
    }
    if let Err(store_err) = validate_pending(ports, &pending.join("iptv.sqlite")) {
        let definitive = !store_err.is_not_database()
            && (store_err.is_snapshot_content_error()
                || matches!(
                    store_err,
                    iptv_store_sqlite::StoreError::SchemaTooNew { .. }
                ));
        if definitive && let Some(slot) = &active_slot {
            rollback_rejected_resumed_restore(root, slot, &active_corrupt, &prior, &pending);
        }
        return Err(ActivateFailure::PreInstallStore(store_err));
    }
    let settings_text = match read_small_file(&pending.join("settings.json"), "settings.json") {
        Ok(text) => text,
        Err(err) => {
            if err.rejection().is_some()
                && let Some(slot) = &active_slot
            {
                rollback_rejected_resumed_restore(root, slot, &active_corrupt, &prior, &pending);
            }
            return Err(ActivateFailure::PreInstall(err));
        }
    };
    if let Err(err) = validate_settings(&settings_text) {
        if err.rejection().is_some()
            && let Some(slot) = &active_slot
        {
            rollback_rejected_resumed_restore(root, slot, &active_corrupt, &prior, &pending);
        }
        return Err(ActivateFailure::PreInstall(err));
    }
    // Recording media stays beside the catalog directory under this host's
    // job ids, so the restored library takes this host's recording rows and
    // keeps reopening it. The backup's own rows were dropped when it was
    // staged. One transaction on the pending file: rerunning after a crash
    // gives the same result. A failure keeps the restore and leaves the media
    // without rows, as before.
    let host = root.join("iptv.sqlite");
    if host.is_file() {
        if let Err(error) = adopt_host_recordings(ports, &host, &pending.join("iptv.sqlite")) {
            let redacted = crate::Error::from(error);
            log::warn!("iptv backup: keeping this device's recordings failed: {redacted}");
        }
    } else if let Some(slot) = active_slot.as_ref()
        && let Ok(copy_work) = scratch_for_ports(ports)
    {
        let copy_db = copy_work.0.join("iptv.sqlite");
        if fs::copy(slot.join("iptv.sqlite"), &copy_db).is_ok() {
            let slot_wal = slot.join("iptv.sqlite-wal");
            if slot_wal.is_file() {
                let _ = fs::copy(slot_wal, copy_work.0.join("iptv.sqlite-wal"));
            }
            if let Err(error) = adopt_host_recordings(ports, &copy_db, &pending.join("iptv.sqlite"))
            {
                let redacted = crate::Error::from(error);
                log::warn!("iptv backup: keeping this device's recordings failed: {redacted}");
            }
        }
    }
    #[cfg(all(test, feature = "native"))]
    tests::crash_boundary("validated");
    if root.exists() {
        fs::rename(root, &previous)
            .context("Could not prepare the restore; current data was kept")
            .map_err(ActivateFailure::PreInstall)?;
    }
    #[cfg(all(test, feature = "native"))]
    tests::crash_boundary("previous");
    let was_corrupt_restore = active_corrupt.exists();
    if let Err(error) = fs::rename(&pending, root) {
        if previous.exists() {
            let _ = fs::rename(&previous, root);
        }
        return Err(ActivateFailure::PreInstall(Error::new(
            "Could not activate the restore; current data was kept",
            error,
        )));
    }
    if was_corrupt_restore {
        started_for(root)
            .resumed_settings_dirty
            .store(true, Ordering::SeqCst);
    }
    Ok(ActivateOutcome::Installed {
        was_corrupt_restore,
        active_slot,
    })
}

fn finish_installed_locked(
    root: &Path,
    was_corrupt_restore: bool,
    active_slot: Option<&Path>,
) -> Result<()> {
    let active_corrupt = sibling(root, "restore-corrupt-active");
    let prior = sibling(root, "restore-prior");
    let previous = sibling(root, "restore-previous");
    #[cfg(all(test, feature = "native"))]
    tests::crash_boundary("installed");
    sync_dir(root.parent().ok_or("Application storage has no parent")?)?;
    let _ = fs::remove_file(&active_corrupt);
    let _ = fs::remove_dir_all(&prior);
    #[cfg(all(test, feature = "native"))]
    tests::crash_boundary("synced");
    if previous.exists() {
        preserve_non_backup_entries(&previous, root)?;
        fs::remove_dir_all(previous)
            .context("Restore succeeded; could not remove the previous private copy")?;
    }
    if was_corrupt_restore {
        let preserved = active_slot
            .and_then(Path::file_name)
            .map(|name| root.join("recovered").join(name));
        let recovered_root = root.join("recovered");
        prune_recovered_copies(
            &recovered_root,
            MAX_RECOVERED_LIBRARIES,
            preserved.as_deref(),
        );
        let _ = sync_dir_io(&recovered_root);
    }
    Ok(())
}

fn preserve_non_backup_entries(previous: &Path, root: &Path) -> Result<()> {
    let mut moved_any = false;
    for name in ["recovered", "iptv.sqlite.encrypted"] {
        let from = previous.join(name);
        let to = root.join(name);
        if from.exists() && !to.exists() {
            fs::rename(&from, &to).context("Could not finish the previous restore")?;
            moved_any = true;
        }
    }
    if moved_any {
        sync_dir(root)?;
        sync_dir(previous)?;
    }
    Ok(())
}

fn db_sidecar(database: &Path, suffix: &str) -> PathBuf {
    let mut name = database.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(any(test, feature = "test-support"))]
fn recovery_crash_point(point: &str) {
    if std::env::var_os("IPTV_TEST_CORRUPT_CRASH_POINT").is_some_and(|val| val == point) {
        std::process::exit(0);
    }
}
#[cfg(not(any(test, feature = "test-support")))]
fn recovery_crash_point(_: &str) {}

fn latest_recovered_slot(root: &Path) -> Option<PathBuf> {
    let recovered_base = root.join("recovered");
    let marker = recovered_base.join(".latest");
    let marked = fs::read_to_string(&marker)
        .ok()
        .map(|name| recovered_base.join(name.trim()))
        .filter(|path| path.join("iptv.sqlite").is_file());
    marked.or_else(|| {
        let entries = fs::read_dir(&recovered_base).ok()?;
        let mut latest: Option<(String, PathBuf)> = None;
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let path = entry.path();
            if !path.join("iptv.sqlite").is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if latest.as_ref().is_none_or(|(prev, _)| name > *prev) {
                latest = Some((name, path));
            }
        }
        latest.map(|(_, path)| path)
    })
}

/// Completes moving leftover `-wal`/`-shm`/`-journal` sidecars and `playlists`
/// if a previous process exited right after moving `iptv.sqlite` into `recovered/`.
pub(crate) fn finish_interrupted_sidecar_recovery(root: &Path, database: &Path) -> io::Result<()> {
    let marker = root.join("recovered/.latest");
    let Some(destination) = latest_recovered_slot(root) else {
        return Ok(());
    };
    let mut moved_any = false;
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = db_sidecar(database, suffix);
        if !sidecar.exists() {
            continue;
        }
        let target = destination.join(format!("iptv.sqlite{suffix}"));
        let fallback = destination.join(format!("iptv.sqlite.orphan{suffix}"));
        let dest = if target.exists() { &fallback } else { &target };
        if let Err(error) = fs::rename(&sidecar, dest)
            && sidecar.exists()
        {
            return Err(error);
        }
        moved_any = true;
    }
    let playlists = root.join("playlists");
    if playlists.exists() {
        let target = destination.join("playlists");
        let fallback = destination.join("playlists.orphan");
        let dest = if target.exists() { &fallback } else { &target };
        if let Err(error) = fs::rename(&playlists, dest)
            && playlists.exists()
        {
            return Err(error);
        }
        moved_any = true;
    }
    if moved_any || marker.exists() {
        let recovered_root = recovered_root_of(&destination);
        sync_dir_io(&destination)?;
        sync_dir_io(&recovered_root)?;
        sync_dir_io(root)?;
        if !has_interrupted_corrupt_restore(root) {
            prune_recovered_copies(&recovered_root, MAX_RECOVERED_LIBRARIES, Some(&destination));
        }
        if marker.exists() {
            fs::remove_file(&marker)?;
            sync_dir_io(&recovered_root)?;
        }
    }
    Ok(())
}

fn recovered_root_of(slot: &Path) -> PathBuf {
    slot.parent().unwrap_or(slot).to_owned()
}

fn recovered_timestamp_prefix() -> String {
    let secs = i64::try_from(crate::time::since_epoch().as_secs()).unwrap_or(0);
    match chrono::DateTime::from_timestamp(secs, 0) {
        Some(dt) => dt.format("%Y%m%d-%H%M%SZ").to_string(),
        None => format!("{secs:010}"),
    }
}

fn next_slot_attempt_for_prefix(recovered_root: &Path, base: &str) -> u32 {
    let Ok(entries) = fs::read_dir(recovered_root) else {
        return 0;
    };
    let mut next = 0u32;
    let dashed = format!("{base}-");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let text = name.to_string_lossy();
        if text == base {
            next = next.max(1);
        } else if let Some(rest) = text.strip_prefix(&dashed)
            && let Ok(num) = rest.parse::<u32>()
        {
            next = next.max(num.saturating_add(1));
        }
    }
    next
}

fn create_recovered_slot(
    recovered_root: &Path,
    exclude: fn(&Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    if !recovered_root.exists() {
        private_dir_with(recovered_root, exclude)?;
    }
    let base = recovered_timestamp_prefix();
    let start = next_slot_attempt_for_prefix(recovered_root, &base);
    for attempt in start..start.saturating_add(10_000) {
        let folder = if attempt == 0 {
            base.clone()
        } else {
            format!("{base}-{attempt:04}")
        };
        let candidate = recovered_root.join(folder);
        match private_dir_with(&candidate, exclude) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

fn prune_recovered_copies(recovered_root: &Path, keep: usize, preserve: Option<&Path>) {
    let Ok(entries) = fs::read_dir(recovered_root) else {
        return;
    };
    let mut has_preserved = false;
    let mut others: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                return None;
            }
            let path = entry.path();
            if preserve == Some(path.as_path()) {
                has_preserved = path.join("iptv.sqlite").is_file();
                return None;
            }
            if !path.join("iptv.sqlite").is_file() {
                // Clean up an empty slot left behind if a crash happened before rename.
                if path.join("README.txt").is_file() {
                    let _ = fs::remove_file(path.join("README.txt"));
                    let _ = fs::remove_dir(&path);
                }
                return None;
            }
            Some((entry.file_name().to_string_lossy().into_owned(), path))
        })
        .collect();
    let max_others = if has_preserved {
        keep.saturating_sub(1)
    } else {
        keep
    };
    others.sort_by(|a, b| a.0.cmp(&b.0));
    if others.len() > max_others {
        for (_, oldest) in others.drain(..others.len() - max_others) {
            let _ = fs::remove_dir_all(oldest);
        }
    }
}

fn move_corrupt_files_aside(ports: &crate::Ports, database: &Path) -> io::Result<PathBuf> {
    let recovered_root = ports.directory.join("recovered");
    let slot = create_recovered_slot(&recovered_root, ports.exclude_from_backup)?;
    move_corrupt_files_into_slot(ports, database, &slot, true)?;
    Ok(slot)
}

fn move_corrupt_files_into_slot(
    ports: &crate::Ports,
    database: &Path,
    slot: &Path,
    prune_after_sync: bool,
) -> io::Result<()> {
    let root = ports.directory.as_path();
    let recovered_root = root.join("recovered");
    let readme_path = slot.join("README.txt");
    let marker_path = recovered_root.join(".latest");
    let slot_name = slot
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let prepared = (|| -> io::Result<()> {
        let mut readme = private_file(&readme_path)?;
        readme.write_all(RECOVERED_README)?;
        readme.sync_all()?;
        write_sync_marker(&marker_path, slot_name.as_bytes())?;
        fs::rename(database, slot.join("iptv.sqlite"))
    })();
    if let Err(error) = prepared {
        let _ = fs::remove_file(&marker_path);
        let _ = fs::remove_file(&readme_path);
        let _ = fs::remove_dir(slot);
        return Err(error);
    }
    recovery_crash_point("before-sidecars");
    let move_companions = (|| -> io::Result<()> {
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = db_sidecar(database, suffix);
            if sidecar.exists()
                && let Err(error) = fs::rename(&sidecar, slot.join(format!("iptv.sqlite{suffix}")))
                && sidecar.exists()
            {
                return Err(error);
            }
        }
        let playlists = root.join("playlists");
        if playlists.exists()
            && let Err(error) = fs::rename(&playlists, slot.join("playlists"))
            && playlists.exists()
        {
            return Err(error);
        }
        Ok(())
    })();
    if let Err(error) = move_companions {
        if rollback_set_aside(database, slot).is_ok() {
            let _ = fs::remove_file(&marker_path);
        }
        return Err(error);
    }
    sync_dir_io(slot)?;
    sync_dir_io(&recovered_root)?;
    sync_dir_io(root)?;
    if prune_after_sync {
        prune_recovered_copies(&recovered_root, MAX_RECOVERED_LIBRARIES, Some(slot));
    }
    if marker_path.exists() {
        fs::remove_file(&marker_path)?;
    }
    sync_dir_io(&recovered_root)?;
    Ok(())
}

/// Restores companions (`-wal`, `-shm`, `-journal`, `playlists`) BEFORE
/// republishing `iptv.sqlite` so the live database never reappears while its
/// WAL or documents remain stranded in `recovered/`.
fn rollback_set_aside(database: &Path, slot: &Path) -> io::Result<()> {
    if !database.exists() && slot.join("iptv.sqlite").is_file() {
        for suffix in ["-wal", "-shm", "-journal"] {
            let moved = slot.join(format!("iptv.sqlite{suffix}"));
            let target = db_sidecar(database, suffix);
            if moved.exists() && !target.exists() {
                fs::rename(&moved, target)?;
            }
        }
        if let Some(root) = database.parent() {
            let moved_playlists = slot.join("playlists");
            let root_playlists = root.join("playlists");
            if moved_playlists.exists() && !root_playlists.exists() {
                fs::rename(&moved_playlists, root_playlists)?;
            }
        }
        fs::rename(slot.join("iptv.sqlite"), database)?;
        let _ = fs::remove_file(slot.join("README.txt"));
        let _ = fs::remove_dir(slot);
    }
    Ok(())
}

/// Re-checks positive database corruption (`is_corrupt_database()`), closes any
/// native background recording host, and atomically renames the corrupt library
/// and its WAL/SHM sidecars into `<data_dir>/recovered/<timestamp>/`.
///
/// Refuses `SQLITE_NOTADB` (wrong/unavailable key), newer schemas, healthy
/// stores, and missing files. Credentials, settings, and update state stay.
pub fn set_aside_corrupt_library(
    ports: &crate::Ports,
) -> std::result::Result<PathBuf, crate::Error> {
    if !ports.file_storage {
        return Err(crate::Error::unavailable(iptv_core::message::key(
            "Private app storage is unavailable",
        )));
    }
    let root = ports.directory.as_path();
    let database = root.join("iptv.sqlite");
    let destination = crate::recordings::host::with_stopped_background_host(root, || {
        crate::storage::with_directory_replacement(root, || {
            reconcile_previous_restore(root).map_err(|_| {
                crate::Error::unavailable(iptv_core::message::key(
                    "Could not move the damaged library aside",
                ))
            })?;
            super::credential_store::verify_corrupt_database(ports, &database)
                .map_err(crate::Error::from)?;
            move_corrupt_files_aside(ports, &database).map_err(|_| {
                crate::Error::unavailable(iptv_core::message::key(
                    "Could not move the damaged library aside",
                ))
            })
        })
    })
    .ok_or_else(|| {
        crate::Error::unavailable(iptv_core::message::key(
            "The saved library could not be opened",
        ))
    })??;
    log::warn!("iptv recovery: corrupt library set aside for a fresh start");
    Ok(destination)
}

/// Distinguishes library-precondition failures from backup-archive failures
/// when restoring into an unreadable library at startup.
#[derive(Debug)]
pub enum RecoveryRestoreError {
    Store(crate::Error),
    Backup(Error),
}

/// Validates a `.patinabackup` archive completely before modifying anything,
/// moves the positively-corrupt library aside into `<data_dir>/recovered/`, and
/// activates the validated backup so reopening loads the restored library.
pub fn restore_backup_into_empty_library(
    ports: &crate::Ports,
    input: &mut impl Read,
) -> Result<PathBuf> {
    check_corrupt_restore_preconditions(ports).map_err(|err| {
        Error::new(
            "The saved library could not be opened",
            err.into_store_error(),
        )
    })?;
    let work = stage_into_scratch_without_live_store(ports, input)?;
    commit_corrupt_restore(ports, work).map_err(|err| match err {
        RecoveryRestoreError::Store(store_err) => Error::new(
            "The saved library could not be opened",
            store_err.into_store_error(),
        ),
        RecoveryRestoreError::Backup(backup_err) => backup_err,
    })
}

pub(crate) fn restore_backup_file_into_empty_library(
    ports: &crate::Ports,
    backup_path: &Path,
) -> std::result::Result<PathBuf, RecoveryRestoreError> {
    check_corrupt_restore_preconditions(ports).map_err(RecoveryRestoreError::Store)?;
    let work = {
        let mut input = File::open(backup_path)
            .context(iptv_core::message::key("Could not read the chosen backup"))
            .map_err(RecoveryRestoreError::Backup)?;
        stage_into_scratch_without_live_store(ports, &mut input)
            .map_err(RecoveryRestoreError::Backup)?
    };
    commit_corrupt_restore(ports, work)
}

fn check_corrupt_restore_preconditions(
    ports: &crate::Ports,
) -> std::result::Result<(), crate::Error> {
    if !ports.file_storage {
        return Err(crate::Error::unavailable(iptv_core::message::key(
            "Private app storage is unavailable",
        )));
    }
    let database = ports.directory.join("iptv.sqlite");
    super::credential_store::verify_corrupt_database(ports, &database).map_err(crate::Error::from)
}

fn commit_corrupt_restore(
    ports: &crate::Ports,
    work: Scratch,
) -> std::result::Result<PathBuf, RecoveryRestoreError> {
    let root = ports.directory.as_path();
    let database = root.join("iptv.sqlite");
    crate::recordings::host::with_stopped_background_host(root, || {
        crate::storage::with_directory_replacement(root, || {
            reconcile_previous_restore(root).map_err(RecoveryRestoreError::Backup)?;
            super::credential_store::verify_corrupt_database(ports, &database)
                .map_err(|err| RecoveryRestoreError::Store(crate::Error::from(err)))?;
            let staged = sibling(root, "restore-corrupt-staged");
            if staged.exists() {
                fs::remove_dir_all(&staged)
                    .context("Could not schedule the restore")
                    .map_err(RecoveryRestoreError::Backup)?;
            }
            fs::rename(&work.0, &staged)
                .context("Could not schedule the restore")
                .map_err(RecoveryRestoreError::Backup)?;
            let recovered_root = root.join("recovered");
            let slot = match create_recovered_slot(&recovered_root, ports.exclude_from_backup) {
                Ok(slot) => slot,
                Err(err) => {
                    let _ = fs::remove_dir_all(&staged);
                    return Err(RecoveryRestoreError::Backup(Error::new(
                        iptv_core::message::key("Could not move the damaged library aside"),
                        err,
                    )));
                }
            };
            let slot_name = slot.file_name().unwrap_or_default().to_os_string();
            let slot_text = slot_name.to_string_lossy().into_owned();
            let active_marker = sibling(root, "restore-corrupt-active");
            if let Err(err) = write_sync_marker(&active_marker, slot_text.as_bytes()) {
                let _ = fs::remove_dir(&slot);
                let _ = fs::remove_dir_all(&staged);
                return Err(RecoveryRestoreError::Backup(Error::new(
                    "Could not schedule the restore",
                    err,
                )));
            }
            if let Err(err) = move_corrupt_files_into_slot(ports, &database, &slot, false) {
                if database.is_file() {
                    let _ = fs::remove_file(&active_marker);
                    let _ = fs::remove_dir_all(&staged);
                }
                return Err(RecoveryRestoreError::Backup(Error::new(
                    iptv_core::message::key("Could not move the damaged library aside"),
                    err,
                )));
            }
            let pending = sibling(root, "restore-pending");
            let prior = sibling(root, "restore-prior");
            let rollback_failed_attempt = |slot: &Path| {
                let _ = fs::remove_dir_all(&staged);
                let _ = cancel(root);
                let _ =
                    write_sync_marker(&active_marker, format!("rollback:{slot_text}").as_bytes());
                if rollback_set_aside(&database, slot).is_ok() {
                    let promoted = if prior.exists() && !pending.exists() {
                        fs::rename(&prior, &pending).is_ok()
                    } else {
                        true
                    };
                    if promoted {
                        let _ = fs::remove_file(&active_marker);
                    }
                }
            };
            if prior.exists() {
                let _ = fs::remove_dir_all(&prior);
            }
            if pending.exists()
                && let Err(err) = fs::rename(&pending, &prior)
            {
                let _ = fs::remove_dir_all(&staged);
                let _ =
                    write_sync_marker(&active_marker, format!("rollback:{slot_text}").as_bytes());
                if rollback_set_aside(&database, &slot).is_ok() {
                    let _ = fs::remove_file(&active_marker);
                }
                return Err(RecoveryRestoreError::Backup(Error::new(
                    "Could not schedule the restore",
                    err,
                )));
            }
            recovery_crash_point("after-set-aside-before-publish");
            if let Err(error) = publish_staged_path(root, &staged) {
                rollback_failed_attempt(&slot);
                return Err(RecoveryRestoreError::Backup(error));
            }
            recovery_crash_point("after-publish-before-activate");
            match activate_locked_outcome(ports, root) {
                Ok(ActivateOutcome::Installed { .. }) => {}
                Ok(ActivateOutcome::NoPendingWork) => {
                    if !database.is_file() {
                        rollback_failed_attempt(&slot);
                    }
                    return Err(RecoveryRestoreError::Backup(Error::from(
                        "Could not activate the restore; current data was kept",
                    )));
                }
                Err(ActivateFailure::PreInstallStore(store_err)) => {
                    if !database.is_file() {
                        rollback_failed_attempt(&slot);
                    }
                    if store_err.is_not_database()
                        || matches!(store_err, iptv_store_sqlite::StoreError::KeyUnavailable(_))
                    {
                        return Err(RecoveryRestoreError::Store(crate::Error::from(store_err)));
                    }
                    return Err(RecoveryRestoreError::Backup(Error::new(
                        "The pending backup library is invalid; current data was kept",
                        store_err,
                    )));
                }
                Err(ActivateFailure::PreInstall(error)) => {
                    if !database.is_file() {
                        rollback_failed_attempt(&slot);
                    }
                    return Err(RecoveryRestoreError::Backup(error));
                }
                Err(ActivateFailure::PostInstall(error)) => {
                    // The validated pending restore was already installed into `root`;
                    // only delete `.restore-previous` once non-backup entries moved out.
                    let previous = sibling(root, "restore-previous");
                    if previous.exists() && preserve_non_backup_entries(&previous, root).is_ok() {
                        let _ = fs::remove_dir_all(&previous);
                    }
                    let preserved = root.join("recovered").join(&slot_name);
                    let recovered_root = root.join("recovered");
                    prune_recovered_copies(
                        &recovered_root,
                        MAX_RECOVERED_LIBRARIES,
                        Some(&preserved),
                    );
                    let _ = sync_dir_io(&recovered_root);
                    log::warn!("iptv backup: post-restore cleanup warning: {error}");
                }
            }
            let _ = sync_dir(root.parent().unwrap_or(Path::new(".")));
            let _ = fs::remove_file(&active_marker);
            let _ = fs::remove_dir_all(&prior);
            log::warn!("iptv recovery: corrupt library set aside and backup restored");
            Ok(root.join("recovered").join(slot_name))
        })
    })
    .ok_or_else(|| {
        RecoveryRestoreError::Backup(Error::from(iptv_core::message::key(
            "Recording host did not close; restart Patina before restoring",
        )))
    })?
}

#[cfg(all(test, feature = "native"))]
mod tests;
