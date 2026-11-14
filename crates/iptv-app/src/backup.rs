//! Platform dialogs and document transport for runtime-owned backups.
use crate::runtime::backup::error::{Context, Result};
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

/// What a backup holds, before its file is chosen.
const EXPORT_NOTE: &str = "The backup includes playlist logins and passwords, settings, \
                           channel arrangements, history and cached guide data. \
                           Keep the file private.";
/// A restore is already scheduled: what happens to changes until then.
const PENDING_NOTE: &str = "The backup replaces current data when you next open Patina. \
                            Changes made before then will be replaced.";
/// What scheduling a restore replaces, and how to call it off.
const RESTORE_NOTE: &str = "This backup will replace all playlists, settings, favorites, \
                            groups and history when you reopen Patina. You can cancel the \
                            scheduled restore from Settings before closing the app.";
/// Android: the app has to be closed and opened for the restore to apply.
#[cfg(target_os = "android")]
const RESTART_NOTE: &str = "Close Patina, then open it again to apply the backup. Keep the \
                            app open to cancel the scheduled restore from Settings first.";

pub(crate) fn export_ui() {
    crate::data::spawn_in_app(async move {
        let Some(busy) = crate::runtime().begin_backup() else {
            return;
        };
        let answer = nsh_base::alert_dialog()
            .title("Back up Patina")
            .description(EXPORT_NOTE)
            .cancel("cancel", "Cancel")
            .action("export", "Choose file…")
            .show()
            .await;
        if !answer.is_ok_and(|a| a.action.as_deref() == Some("export")) {
            return;
        }
        if !crate::favorites::flush().await {
            crate::settings_view::notify("Recent channel changes could not be saved; try again");
            return;
        }
        let settings = crate::settings::get().to_json_string();
        let root = crate::data::data_dir();
        crate::settings_view::notify("Writing backup…");
        let result = save_export(&busy, root, settings).await;
        crate::settings_view::notify(match result {
            Ok(true) => "Backup saved",
            Ok(false) => "Backup cancelled",
            Err(error) => report("writing a backup", error),
        });
    });
}

pub(crate) fn restore_ui(ui: crate::Ui) {
    crate::data::spawn_in_app(async move {
        let Some(busy) = crate::runtime().begin_backup() else {
            return;
        };
        let root = crate::data::data_dir();
        if crate::runtime().restore_is_pending() {
            let answer = nsh_base::alert_dialog()
                .title("Restore scheduled")
                .description(PENDING_NOTE)
                .cancel("keep", "Keep scheduled")
                .action("cancel", "Cancel restore")
                .action("replace", "Choose another backup…")
                .show()
                .await;
            match answer.ok().and_then(|a| a.action) {
                Some(action) if action.as_str() == "cancel" => {
                    let result = busy.worker(|archive| archive.cancel()).await;
                    crate::settings_view::refresh(ui);
                    crate::settings_view::notify(match result {
                        Ok(()) => "Scheduled restore cancelled",
                        Err(error) => report("cancelling the scheduled restore", error),
                    });
                    return;
                }
                Some(action) if action.as_str() == "replace" => {}
                _ => {
                    offer_restart().await;
                    return;
                }
            }
        }
        let file = match select_import(root.clone()).await {
            Ok(Some(file)) => file,
            Ok(None) => return,
            Err(error) => {
                crate::settings_view::notify(report("choosing a backup", error));
                return;
            }
        };
        let answer = nsh_base::alert_dialog()
            .title("Replace data on next launch?")
            .description(RESTORE_NOTE)
            .cancel("cancel", "Cancel")
            .destructive("restore", "Schedule restore")
            .show()
            .await;
        if !answer.is_ok_and(|a| a.action.as_deref() == Some("restore")) {
            return;
        }
        crate::settings_view::notify("Checking backup…");
        let result = busy
            .worker(move |archive| {
                let mut input =
                    File::open(&file.path).context("Could not read the chosen backup")?;
                archive.stage(&mut input)
            })
            .await;
        crate::settings_view::refresh(ui);
        crate::settings_view::notify(match result {
            Ok(()) => "Restore scheduled. Close and reopen Patina to apply it",
            Err(error) => report("scheduling a restore", error),
        });
        if result.is_ok() {
            offer_restart().await;
        }
    });
}

#[cfg(not(target_os = "android"))]
async fn save_export(busy: &Busy, _root: PathBuf, settings: String) -> Result<bool> {
    use nsh_core::file_dialog::{FileFilter, SaveDialog};
    let destination = nsh_core::element::pick_save_path(
        SaveDialog::new()
            .file_name("Patina.patinabackup")
            .filter(FileFilter::new("Patina backup", ["patinabackup"])),
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
    thread_local! {
        static REQUEST: Cell<Option<Signal<Value>>> = const { Cell::new(None) };
        static ANSWER: RefCell<Option<(String, futures_channel::oneshot::Sender<Result<bool>>)>> = const { RefCell::new(None) };
    }
    pub fn layer() -> impl IntoElement {
        let request = signal(Value::Null);
        REQUEST.with(|slot| slot.set(Some(request)));
        native("tv.backup")
            .absolute()
            .w(px(0.))
            .h(px(0.))
            .prop("archivePath", move || request.get()["archivePath"].clone())
            .prop("requestId", move || request.get()["requestId"].clone())
            .prop("operation", move || request.get()["operation"].clone())
            .on_native_event(move |event: &NativeEvent| {
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
                            Ok(true)
                        } else if event.data["cancelled"].as_bool() == Some(true) {
                            Ok(false)
                        } else {
                            Err("Could not read or write the chosen backup document".into())
                        };
                        let _ = tx.send(value);
                        request.set(Value::Null);
                    }
                });
            })
    }
    async fn document(request_id: String, path: &Path, operation: &str) -> Result<bool> {
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
        let result = document(request_id, &path, "export").await;
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
        let file = ImportFile {
            path,
            temporary: true,
        };
        if document(request_id, &file.path, "import").await? {
            Ok(Some(file))
        } else {
            Ok(None)
        }
    }
}

#[cfg(target_os = "android")]
async fn save_export(busy: &Busy, root: PathBuf, settings: String) -> Result<bool> {
    android::save(busy, root, settings).await
}

pub(crate) fn service_layer() -> impl nsh_core::prelude::IntoElement {
    #[cfg(target_os = "android")]
    {
        android::layer()
    }
    #[cfg(not(target_os = "android"))]
    {
        use nsh_core::prelude::*;
        view().absolute().w(px(0.)).h(px(0.))
    }
}

struct ImportFile {
    path: PathBuf,
    temporary: bool,
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
        OpenDialog::files().filter(FileFilter::new("Patina backup", ["patinabackup"])),
    )
    .await
    {
        Ok(Some(paths)) => Ok(paths.into_iter().next().map(|path| ImportFile {
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
            .title("Restore scheduled")
            .description(RESTART_NOTE)
            .cancel("keep", "Keep app open")
            .action("close", "Close Patina")
            .show()
            .await;
        if answer.is_ok_and(|a| a.action.as_deref() == Some("close")) {
            android::close().await;
        }
    }
}
