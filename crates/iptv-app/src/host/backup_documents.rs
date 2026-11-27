//! Native backup document transport, with caller-owned picker state and labels.
use std::{fs, path::PathBuf};

use iptv_runtime::backup::{Busy, error::Result};

#[cfg(target_os = "android")]
mod android {
    use super::*;
    use iptv_runtime::backup::error::Context;
    use iptv_runtime::backup::next_document_request;
    use nsh_core::prelude::*;
    use serde_json::{Value, json};
    use std::cell::{Cell, RefCell};
    use std::path::Path;

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
    pub fn layer(on_pickers: impl Fn(bool, bool) + 'static) -> impl IntoElement {
        let request = signal(Value::Null);
        REQUEST.with(|slot| slot.set(Some(request)));
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
                    on_pickers(
                        event.data["create"].as_bool() == Some(true),
                        event.data["open"].as_bool() == Some(true),
                    );
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
    pub async fn close(root: PathBuf) {
        let request_id = next_document_request();
        let _ = document(request_id, &root, "close").await;
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
pub(crate) use android::{close, layer, probe};

#[cfg(not(target_os = "android"))]
pub(crate) fn probe() {}

#[cfg(not(target_os = "android"))]
pub(crate) async fn close(_: PathBuf) {}

#[cfg(not(target_os = "android"))]
pub(crate) fn layer(_: impl Fn(bool, bool) + 'static) -> impl nsh_core::prelude::IntoElement {
    use nsh_core::prelude::*;
    view().absolute().w(px(0.)).h(px(0.))
}

#[cfg(not(target_os = "android"))]
pub(crate) async fn save_export(
    busy: &Busy,
    _root: PathBuf,
    settings: String,
    label: &'static str,
) -> Result<bool> {
    use nsh_core::file_dialog::{FileFilter, SaveDialog};
    let destination = nsh_core::element::pick_save_path(
        SaveDialog::new()
            .file_name("Patina.patinabackup")
            .filter(FileFilter::new(label, ["patinabackup"])),
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
pub(crate) async fn save_export(
    busy: &Busy,
    root: PathBuf,
    settings: String,
    _label: &'static str,
) -> Result<bool> {
    android::save(busy, root, settings).await
}

pub(crate) struct ImportFile {
    pub(crate) path: PathBuf,
    pub(crate) name: Option<String>,
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
pub(crate) async fn select_import(
    _root: PathBuf,
    label: &'static str,
) -> Result<Option<ImportFile>> {
    use nsh_core::file_dialog::{FileFilter, OpenDialog};
    match nsh_core::element::pick_paths(
        OpenDialog::files().filter(FileFilter::new(label, ["patinabackup"])),
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
pub(crate) async fn select_import(
    root: PathBuf,
    _label: &'static str,
) -> Result<Option<ImportFile>> {
    android::select(root).await
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
