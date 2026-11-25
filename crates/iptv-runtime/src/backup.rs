//! Whole-app snapshots. Restores are staged, then activated before any
//! settings, SQLite connection, import job or player is started. Directory
//! renames keep the previous state recoverable across an interrupted swap.

pub mod error;
mod instance;
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::store_service::Services;

use iptv_settings::Settings;
use iptv_store_sqlite::Store;
use serde_json::json;

const MAGIC: &[u8; 9] = b"PATINABK\x01";
use iptv_import::backup_manifest::{
    self, MAX_BYTES, MAX_FILES, MAX_MANIFEST, allowed, entry_limit,
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "android")]
pub fn next_document_request() -> String {
    SEQUENCE.fetch_add(1, Ordering::Relaxed).to_string()
}
use error::{Context, Error, Rejection, Result};
type Started = Arc<OnceLock<std::result::Result<(), StartupError>>>;
/// The first activation outcome of each storage root in this process. A
/// process-lifetime fact about a directory, not owner state: every owner of
/// the same root shares it, and an owner of another root has its own.
static STARTED: Mutex<Option<HashMap<PathBuf, Started>>> = Mutex::new(None);

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
    runtime.backup_progress.set(phase);
    runtime.emit(super::RuntimeEvent::BackupProgress(phase));
}
/// One backup operation at a time per owner; ends when dropped.
pub struct Busy(std::rc::Rc<super::Runtime>);
impl Busy {
    pub(crate) fn begin(runtime: std::rc::Rc<super::Runtime>) -> Option<Self> {
        if runtime.backup_busy.replace(true) {
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
        self.0.backup_busy.set(false);
        self.0.backup_progress.set(Progress::Idle);
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

pub fn sibling(root: &Path, suffix: &str) -> PathBuf {
    root.with_file_name(format!(
        "{}.{}",
        root.file_name().unwrap_or_default().to_string_lossy(),
        suffix
    ))
}
fn scratch(services: &Services) -> Result<Scratch> {
    let path = sibling(
        services.directory(),
        &format!(
            "backup-work-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ),
    );
    private_dir(services, &path).context("Could not create temporary backup storage")?;
    Ok(Scratch(path))
}
fn private_dir(services: &Services, path: &Path) -> io::Result<()> {
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    let mut builder = builder;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    let result = (services.ports().exclude_from_backup)(path);
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
fn validate_pending(
    _: &crate::Ports,
    path: &Path,
) -> std::result::Result<(), iptv_store_sqlite::StoreError> {
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
    let root = services.directory();
    let work = scratch(services)?;
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
    seal_restore(services, &work.0)?;
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
    if let Err(error) = fs::rename(&work.0, &pending) {
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
    let started = STARTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_default()
        .entry(root.clone())
        .or_default()
        .clone();
    // Keep the first outcome, including a fatal recovery failure. Recreating
    // an engine must neither rerun activation nor bypass failed recovery.
    *started.get_or_init(|| {
        activate(ports, root).map_err(|error| StartupError {
            error,
            fatal: !root.exists(),
        })
    })
}

fn activate(ports: &crate::Ports, root: &Path) -> Result<()> {
    remove_stale_work(root)?;
    recover_replacement(root)?;
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
    let pending = sibling(root, "restore-pending");
    let previous = sibling(root, "restore-previous");
    if previous.exists() {
        if root.join("iptv.sqlite").is_file() {
            // The validated tree was renamed into place before interruption.
            fs::remove_dir_all(&previous).context("Could not finish the previous restore")?;
        } else {
            // Platform initialization may recreate the missing directory before
            // recovery runs. Its existence is not an activation commit marker.
            if root.exists() {
                fs::remove_dir_all(root).context("Could not recover application storage")?;
            }
            fs::rename(&previous, root).context("Could not recover application storage")?;
        }
    }
    if !pending.exists() {
        if !root.exists() && previous.exists() {
            fs::rename(previous, root).context("Could not recover application storage")?;
        }
        return Ok(());
    }
    // Revalidate persisted staging before moving the working state aside.
    for entry in fs::read_dir(&pending).context("Could not read pending backup")? {
        let entry = entry.context("Could not read pending backup entry")?;
        if let Some(name) = entry.file_name().to_str() {
            entry_limit(
                name,
                entry
                    .metadata()
                    .context("Could not read backup file size")?
                    .len(),
            )
            .context("Backup structured entry is too large")?;
        }
    }
    validate_pending(ports, &pending.join("iptv.sqlite"))
        .context("The pending backup library is invalid; current data was kept")?;
    validate_settings(&read_small_file(
        &pending.join("settings.json"),
        "settings.json",
    )?)?;
    // Recording media stays beside the catalog directory under this host's
    // job ids, so the restored library takes this host's recording rows and
    // keeps reopening it. The backup's own rows were dropped when it was
    // staged. One transaction on the pending file: rerunning after a crash
    // gives the same result. A failure keeps the restore and leaves the media
    // without rows, as before.
    let host = root.join("iptv.sqlite");
    if host.is_file()
        && let Err(error) = adopt_host_recordings(ports, &host, &pending.join("iptv.sqlite"))
    {
        log::warn!("iptv backup: keeping this device's recordings failed: {error}");
    }
    #[cfg(all(test, feature = "native"))]
    tests::crash_boundary("validated");
    if root.exists() {
        fs::rename(root, &previous)
            .context("Could not prepare the restore; current data was kept")?;
    }
    #[cfg(all(test, feature = "native"))]
    tests::crash_boundary("previous");
    if let Err(error) = fs::rename(&pending, root) {
        if previous.exists() {
            let _ = fs::rename(&previous, root);
        }
        return Err(Error::new(
            "Could not activate the restore; current data was kept",
            error,
        ));
    }
    #[cfg(all(test, feature = "native"))]
    tests::crash_boundary("installed");
    sync_dir(root.parent().ok_or("Application storage has no parent")?)?;
    #[cfg(all(test, feature = "native"))]
    tests::crash_boundary("synced");
    if previous.exists() {
        fs::remove_dir_all(previous)
            .context("Restore succeeded; could not remove the previous private copy")?;
    }
    Ok(())
}

#[cfg(all(test, feature = "native"))]
mod tests;
