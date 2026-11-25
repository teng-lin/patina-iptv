//! Platform dialogs and document transport for runtime-owned backups.
use crate::i18n::{tr, trf};
use crate::runtime::backup::error::{Context, Error, Rejection, Result};
#[cfg(target_os = "android")]
use crate::runtime::backup::next_document_request;
use crate::runtime::backup::{Busy, report};

#[cfg(target_os = "android")]
use serde_json::Value;
#[cfg(target_os = "android")]
use serde_json::json;
#[cfg(target_os = "android")]
use std::cell::Cell;
use std::fs::{self, File};
#[cfg(target_os = "android")]
use std::path::Path;
use std::path::PathBuf;

mod review;
pub(crate) use review::{RestoreReview, back, is_open, key, layer};

/// What a backup holds, before its file is chosen.
const EXPORT_NOTE: &str = crate::i18n::key(
    "The unencrypted backup includes playlist logins and passwords, settings, \
     channel arrangements, history and guide setup. Downloaded guide listings are excluded. \
     Keep the file private.",
);
/// A restore is already scheduled: what happens to changes until then.
const PENDING_NOTE: &str = crate::i18n::key(
    "The backup replaces current data when you next open Patina. \
     Changes made before then will be replaced.",
);
/// Android: the app has to be closed and opened for the restore to apply.
#[cfg(target_os = "android")]
const RESTART_NOTE: &str = crate::i18n::key(
    "Close Patina, then open it again to apply the backup. Keep the \
     app open to cancel the scheduled restore from Settings first.",
);

pub(crate) fn export_ui() {
    crate::data::spawn_in_app(async move {
        let Some(busy) = crate::runtime().begin_backup() else {
            return;
        };
        let answer = nsh_base::alert_dialog()
            .title(tr("Back up Patina"))
            .description(tr(EXPORT_NOTE))
            .cancel("cancel", tr("Cancel"))
            .action("export", tr("Choose file…"))
            .show()
            .await;
        if !answer.is_ok_and(|a| a.action.as_deref() == Some("export")) {
            return;
        }
        if !crate::favorites::flush().await {
            crate::settings_view::notify(tr(
                "Recent channel changes could not be saved; try again",
            ));
            return;
        }
        let settings = crate::settings::get().to_json_string();
        let root = crate::data::data_dir();
        crate::settings_view::notify(tr("Writing backup…"));
        let result = save_export(&busy, root, settings).await;
        crate::settings_view::notify(tr(match result {
            Ok(true) => "Backup saved",
            Ok(false) => "Backup canceled",
            Err(error) => report("writing a backup", error),
        }));
    });
}

/// The document pickers a device has: one that creates a file (export)
/// and one that opens one (import). Each backup action is offered only
/// where its picker is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pickers {
    pub create: bool,
    pub open: bool,
}

impl Pickers {
    pub const NONE: Pickers = Pickers {
        create: false,
        open: false,
    };
    pub const BOTH: Pickers = Pickers {
        create: true,
        open: true,
    };

    /// What `platform` has before (or without) asking the device.
    pub const fn of(platform: iptv_core::capabilities::Platform) -> Pickers {
        use iptv_core::capabilities::Platform;
        match platform {
            // NSH's save and open dialogs.
            Platform::Linux | Platform::Macos | Platform::Windows => Pickers::BOTH,
            // The iPad's document transport is unfinished: no backup rows
            // until it works.
            Platform::Ipad => Pickers::NONE,
            // Android's host says which pickers resolve (`tv.backup`'s
            // `pickers` event); none until it answers.
            Platform::Android | Platform::Unknown => Pickers::NONE,
        }
    }
}

#[cfg(feature = "test-support")]
static PICKERS_FOR_TEST: std::sync::Mutex<Option<Pickers>> = std::sync::Mutex::new(None);

/// The pickers apps started from now on have, whatever their platform
/// (headless tests: a device without one, or the iPad's).
#[cfg(feature = "test-support")]
pub fn set_pickers_for_test(pickers: Pickers) {
    *PICKERS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pickers);
}

fn initial_pickers() -> Pickers {
    #[cfg(feature = "test-support")]
    if let Some(pickers) = *PICKERS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        return pickers;
    }
    Pickers::of(crate::host::PLATFORM)
}

thread_local! {
    /// This device's pickers, made with the service layer. Android's host
    /// reports them when the layer mounts, when Settings opens
    /// ([`probe_pickers`]) and when the app resumes.
    static PICKERS: std::cell::Cell<Option<nsh_core::prelude::Signal<Pickers>>> =
        const { std::cell::Cell::new(None) };
}

/// This device's pickers (reactive once the service layer is made).
pub(crate) fn pickers() -> Pickers {
    PICKERS
        .with(std::cell::Cell::get)
        .map_or_else(initial_pickers, |pickers| pickers.get())
}

/// Asks the host again which pickers resolve (Settings opening): one may
/// have been installed, enabled or removed since.
pub(crate) fn probe_pickers() {
    #[cfg(target_os = "android")]
    android::probe();
}

/// Whether the backup command `key` (`general.backup` or
/// `general.restore`) can run here. Exporting needs a picker that creates
/// a file. Restoring needs one that opens a file, except to review, keep
/// or cancel a restore already scheduled, which needs none.
pub(crate) fn offered(key: &str) -> bool {
    let pickers = pickers();
    match key {
        "general.backup" => pickers.create,
        _ => pickers.open || crate::runtime().restore_is_pending(),
    }
}

/// Whether this host offers the restore entry point (the welcome screen's
/// and Settings').
pub(crate) fn available() -> bool {
    matches!(
        crate::settings::offer_now("general.restore"),
        iptv_presentation::settings::screen::Offer::Active
    )
}

pub(crate) fn request_restore(ui: crate::Ui) {
    if available() {
        crate::parental_view::request(ui, iptv_core::parental::Target::settings(), move || {
            restore_ui(ui)
        });
    }
}

fn restore_ui(ui: crate::Ui) {
    crate::data::spawn_in_app(async move {
        let Some(busy) = crate::runtime().begin_backup() else {
            return;
        };
        let root = crate::data::data_dir();
        if crate::runtime().restore_is_pending() {
            let dialog = nsh_base::alert_dialog()
                .title(tr("Restore scheduled"))
                .description(tr(PENDING_NOTE))
                .cancel("keep", tr("Keep scheduled"))
                .action("cancel", tr("Cancel restore"));
            // Without a picker that opens files the scheduled restore can be
            // kept or canceled, not replaced.
            let dialog = if pickers().open {
                dialog.action("replace", tr("Choose another backup…"))
            } else {
                dialog
            };
            let answer = dialog.show().await;
            match answer.ok().and_then(|a| a.action) {
                Some(action) if action.as_str() == "cancel" => {
                    let result = busy.worker(|archive| archive.cancel()).await;
                    crate::settings_view::refresh(ui);
                    crate::settings_view::notify(tr(match result {
                        Ok(()) => "Scheduled restore canceled",
                        Err(error) => report("canceling the scheduled restore", error),
                    }));
                    return;
                }
                Some(action) if action.as_str() == "replace" => {}
                _ => {
                    offer_restart().await;
                    return;
                }
            }
        }
        let mut file = match select_import(root).await {
            Ok(Some(file)) => file,
            Ok(None) => return,
            Err(error) => {
                crate::settings_view::notify(tr(report("choosing a backup", error)));
                return;
            }
        };
        if !review::confirm(ui, file.name.take()).await {
            return;
        }
        crate::settings_view::notify(tr("Checking backup…"));
        let result = busy
            .worker(move |archive| {
                let mut input =
                    File::open(&file.path).context("Could not read the chosen backup")?;
                archive.stage(&mut input)
            })
            .await;
        crate::settings_view::refresh(ui);
        // Release operation ownership before presenting a dismissible failure.
        drop(busy);
        match result {
            Ok(()) => {
                crate::settings_view::notify(tr(
                    "Restore scheduled. Close and reopen Patina to apply it",
                ));
                offer_restart().await;
            }
            Err(error) => {
                if error.rejection().is_some() {
                    report("scheduling a restore", error);
                    let _ = nsh_base::alert_dialog()
                        .title(tr("Backup could not be restored"))
                        .description(rejection_description(error))
                        .cancel("close", tr("Close"))
                        .show()
                        .await;
                } else {
                    crate::settings_view::notify(tr(report("scheduling a restore", error)));
                }
            }
        }
    });
}

/// Independent facts are paragraphs so recovery guidance remains readable in
/// native dialogs at enlarged text sizes. No archive-provided text is shown.
fn rejection_description(error: Error) -> String {
    let (detail, newer) = match error.rejection() {
        Some(Rejection::Format { found: Some(found), supported }) => (
            trf("Backup format version {found} is not supported; this Patina supports version {supported}.",
                &[("found", found.into()), ("supported", supported.into())]),
            found > supported,
        ),
        Some(Rejection::Format { found: None, .. }) => (
            tr("This backup format is not supported.").to_owned(), false,
        ),
        Some(Rejection::Settings { found: Some(found), supported }) => (
            trf("This Patina cannot restore these backup settings (version {found}; supported version {supported}).",
                &[("found", found.into()), ("supported", supported.into())]),
            found > supported,
        ),
        Some(Rejection::Settings { found: None, .. }) => (
            tr("This backup contains settings in an unsupported format.").to_owned(), false,
        ),
        Some(Rejection::Database { found, supported }) => (
            trf("Backup database schema {found} is not supported; this Patina supports schema {supported}.",
                &[("found", found.into()), ("supported", supported.into())]),
            found > supported,
        ),
        _ => return [
            tr("This backup is damaged or incomplete. Choose another backup, or create a new backup on the original device."),
            tr("Your current data and any previously scheduled restore have been kept."),
        ].join("\n\n"),
    };
    let next = if newer {
        tr("Use a newer Patina version that supports this backup, or choose another backup.")
    } else {
        tr("Choose another backup, or use a Patina version that supports this backup.")
    };
    [
        detail.as_str(),
        next,
        tr("Your current data and any previously scheduled restore have been kept."),
    ]
    .join("\n\n")
}

#[cfg(not(target_os = "android"))]
async fn save_export(busy: &Busy, _root: PathBuf, settings: String) -> Result<bool> {
    use nsh_core::file_dialog::{FileFilter, SaveDialog};
    let destination = nsh_core::element::pick_save_path(
        SaveDialog::new()
            .file_name("Patina.patinabackup")
            .filter(FileFilter::new(tr("Patina backup"), ["patinabackup"])),
    )
    .await;
    let destination = match destination {
        Ok(Some(path)) => path,
        Ok(None) => return Ok(false),
        Err(_) => return Err("Saving backup files is unavailable on this device".into()),
    };
    busy.worker(move |archive| archive.export(&destination, &settings))
        .await?;
    Ok(true)
}

#[cfg(target_os = "android")]
mod android {
    use super::*;
    use nsh_core::prelude::*;
    use std::cell::RefCell;

    enum Document {
        Complete(Option<String>),
        Canceled,
    }

    thread_local! {
        static REQUEST: Cell<Option<Signal<Value>>> = const { Cell::new(None) };
        static ANSWER: RefCell<Option<(String, futures_channel::oneshot::Sender<Result<Document>>)>> = const { RefCell::new(None) };
        /// Bumped to have the host report its pickers again.
        static PROBE: Cell<Option<Signal<u64>>> = const { Cell::new(None) };
    }
    pub fn probe() {
        if let Some(probe) = PROBE.with(Cell::get) {
            probe.update(|n| *n += 1);
        }
    }
    pub fn layer(ui: crate::Ui) -> impl IntoElement {
        let request = signal(Value::Null);
        REQUEST.with(|slot| slot.set(Some(request)));
        let pickers = signal(initial_pickers());
        PICKERS.with(|slot| slot.set(Some(pickers)));
        // The host reports its pickers once mounted, and again each time
        // this changes and each time the app resumes: never per frame.
        let probe = signal(0u64);
        PROBE.with(|slot| slot.set(Some(probe)));
        native("tv.backup")
            .absolute()
            .w(px(0.))
            .h(px(0.))
            .prop("archivePath", move || request.get()["archivePath"].clone())
            .prop("requestId", move || request.get()["requestId"].clone())
            .prop("operation", move || request.get()["operation"].clone())
            .prop("probe", move || Value::from(probe.get()))
            .on_native_event(move |event: &NativeEvent| {
                if event.name.as_ref() == "pickers" {
                    let found = Pickers {
                        create: event.data["create"].as_bool() == Some(true),
                        open: event.data["open"].as_bool() == Some(true),
                    };
                    if pickers.get_untracked() != found {
                        pickers.set(found);
                        // An open welcome screen follows.
                        let restore = available();
                        ui.wizard.update(|open| {
                            if let Some(open) = open {
                                open.wizard.set_restore(restore);
                            }
                        });
                    }
                    return;
                }
                if event.name.as_ref() != "complete" {
                    return;
                }
                ANSWER.with(|slot| {
                    let mut answer = slot.borrow_mut();
                    if answer.as_ref().is_some_and(|(id, _)| {
                        event.data["requestId"].as_str() == Some(id.as_str())
                    }) {
                        let (_, tx) = answer.take().unwrap();
                        let value = if event.data["success"].as_bool() == Some(true) {
                            let name = event.data["displayName"].as_str().and_then(backup_name);
                            Ok(Document::Complete(name))
                        } else if event.data["canceled"].as_bool() == Some(true) {
                            Ok(Document::Canceled)
                        } else {
                            Err("Could not read or write the chosen backup document".into())
                        };
                        let _ = tx.send(value);
                        request.set(Value::Null);
                    }
                });
            })
    }
    async fn document(request_id: String, path: &Path, operation: &str) -> Result<Document> {
        let Some(request) = REQUEST.with(Cell::get) else {
            return Err("The document service is unavailable".into());
        };
        let (tx, rx) = futures_channel::oneshot::channel();
        ANSWER.with(|slot| *slot.borrow_mut() = Some((request_id.clone(), tx)));
        request.set(json!({"requestId": request_id, "archivePath": path.to_string_lossy(), "operation": operation}));
        rx.await.context("The document service stopped")?
    }
    pub async fn save(busy: &Busy, root: PathBuf, settings: String) -> Result<bool> {
        let request_id = next_document_request();
        let path = root.join(format!("backup-export-{request_id}.patinabackup"));
        let prepared = path.clone();
        busy.worker(move |archive| archive.export(&prepared, &settings))
            .await?;
        let result = document(request_id, &path, "export")
            .await
            .map(|result| matches!(result, Document::Complete(_)));
        let _ = fs::remove_file(path);
        result
    }
    pub async fn close() {
        let request_id = next_document_request();
        let _ = document(request_id, &crate::data::data_dir(), "close").await;
    }
    pub async fn select(root: PathBuf) -> Result<Option<ImportFile>> {
        let request_id = next_document_request();
        let path = root.join(format!("backup-import-{request_id}.patinabackup"));
        let mut file = ImportFile {
            path,
            name: None,
            temporary: true,
        };
        match document(request_id, &file.path, "import").await? {
            Document::Complete(name) => {
                file.name = name;
                Ok(Some(file))
            }
            Document::Canceled => Ok(None),
        }
    }
}

#[cfg(target_os = "android")]
async fn save_export(busy: &Busy, root: PathBuf, settings: String) -> Result<bool> {
    android::save(busy, root, settings).await
}

pub(crate) fn service_layer(ui: crate::Ui) -> impl nsh_core::prelude::IntoElement {
    #[cfg(target_os = "android")]
    {
        android::layer(ui)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = ui;
        use nsh_core::prelude::*;
        view().absolute().w(px(0.)).h(px(0.))
    }
}

struct ImportFile {
    path: PathBuf,
    name: Option<String>,
    temporary: bool,
}

/// A document's display label is a basename without hidden directional controls.
fn backup_name(name: &str) -> Option<String> {
    let name: String = name
        .rsplit(['/', '\\'])
        .next()?
        .chars()
        .filter(|ch| {
            !ch.is_control()
                && !matches!(ch, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(255)
        .collect();
    match name.trim() {
        "" | "." | ".." => None,
        name => Some(name.to_owned()),
    }
}
impl Drop for ImportFile {
    fn drop(&mut self) {
        if self.temporary {
            let _ = fs::remove_file(&self.path);
        }
    }
}
#[cfg(not(target_os = "android"))]
async fn select_import(_root: PathBuf) -> Result<Option<ImportFile>> {
    use nsh_core::file_dialog::{FileFilter, OpenDialog};
    match nsh_core::element::pick_paths(
        OpenDialog::files().filter(FileFilter::new(tr("Patina backup"), ["patinabackup"])),
    )
    .await
    {
        Ok(Some(paths)) => Ok(paths.into_iter().next().map(|path| ImportFile {
            name: path
                .file_name()
                .and_then(|name| backup_name(&name.to_string_lossy())),
            path,
            temporary: false,
        })),
        Ok(None) => Ok(None),
        Err(_) => Err("Opening backup documents is unavailable on this device".into()),
    }
}
#[cfg(target_os = "android")]
async fn select_import(root: PathBuf) -> Result<Option<ImportFile>> {
    android::select(root).await
}

async fn offer_restart() {
    #[cfg(target_os = "android")]
    {
        let answer = nsh_base::alert_dialog()
            .title(tr("Restore scheduled"))
            .description(tr(RESTART_NOTE))
            .cancel("keep", tr("Keep app open"))
            .action("close", tr("Close Patina"))
            .show()
            .await;
        if answer.is_ok_and(|a| a.action.as_deref() == Some("close")) {
            android::close().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::backup_name;

    #[test]
    fn backup_names_show_only_the_safe_basename() {
        assert_eq!(
            backup_name("/generated/path/家庭.patinabackup").as_deref(),
            Some("家庭.patinabackup")
        );
        assert_eq!(
            backup_name("C:\\generated\\archive\u{202e}\n.patinabackup").as_deref(),
            Some("archive.patinabackup")
        );
        assert_eq!(backup_name(" /.. "), None);
        assert_eq!(backup_name("\u{2067}\u{2069}"), None);
    }
}
