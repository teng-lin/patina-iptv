//! A viewer-confirmed full AppImage upgrade. The current process keeps running.
use crate::{
    Ui,
    i18n::tr,
    settings_view::{notify, updates::is_open},
};
use iptv_runtime::app_updates::ReleaseAsset;
use nsh_base::alert_dialog;
use nsh_core::{element::pick_save_path, file_dialog::SaveDialog};

pub(super) async fn update(ui: Ui, asset: ReleaseAsset) {
    let owner = crate::runtime_owner::runtime();
    let image = owner.running_appimage().await;
    if !is_open(ui) {
        return;
    }
    let mut prompt = alert_dialog()
        .title(tr("Update AppImage"))
        .cancel("cancel", tr("Cancel"))
        .action("save", tr("Save as…"))
        .close_on_escape(true);
    if let Some(image) = &image {
        prompt = prompt
            .description(image.path().display().to_string())
            .action("replace", tr("Replace installed file"));
    }
    let answer = prompt.show().await.ok().and_then(|r| r.action);
    if answer.as_deref() == Some("save") {
        let Some(name) = asset.url().rsplit('/').next() else {
            return;
        };
        let Ok(Some(destination)) = pick_save_path(SaveDialog::new().file_name(name)).await else {
            return;
        };
        let Some(parent) = destination.parent() else {
            return;
        };
        if !owner.can_save_appimage(destination.clone()).await {
            return notify(tr("Choose a writable location and a new file name."));
        }
        let Some(directory) = owner.stage_appimage(parent.to_owned()).await else {
            return notify(tr("Choose a writable location and a new file name."));
        };
        notify(tr("Downloading…"));
        let Some(file) = owner.download_appimage(asset, directory).await else {
            return notify(tr("Update download failed. Try again."));
        };
        if owner.save_appimage(destination, file).await {
            notify(tr("AppImage saved."));
        } else {
            notify(tr("Choose a writable location and a new file name."));
        }
        return;
    }
    if answer.as_deref() != Some("replace") {
        return;
    }
    let Some(image) = image else { return };
    if owner.recording_background_needed() {
        return notify(tr("Finish recording before installing the update."));
    }
    let Some(parent) = image.path().parent() else {
        return;
    };
    let Some(directory) = owner.stage_appimage(parent.to_owned()).await else {
        return notify(tr("Could not replace the AppImage. Try Save as…"));
    };
    notify(tr("Downloading…"));
    let Some(file) = owner.download_appimage(asset, directory).await else {
        return notify(tr(
            "Download failed. Try Save as… to choose another location.",
        ));
    };
    if !is_open(ui) {
        return;
    }
    if owner.recording_background_needed() {
        return notify(tr("Finish recording before installing the update."));
    }
    let confirm = alert_dialog()
        .title(tr("Install update?"))
        .description(image.path().display().to_string())
        .cancel("close", tr("Close"))
        .action("install", tr("Install update"))
        .close_on_escape(true)
        .show()
        .await;
    if confirm.ok().and_then(|r| r.action).as_deref() != Some("install") {
        return;
    }
    if owner.recording_background_needed() {
        return notify(tr("Finish recording before installing the update."));
    }
    let Some(previous) = owner.replace_appimage(image, file).await else {
        return notify(tr("Could not replace the AppImage. Try Save as…"));
    };
    let _ = alert_dialog()
        .title(tr("Restart Patina to use the update."))
        .description(format!(
            "{}\n{}",
            tr("Previous AppImage"),
            previous.display()
        ))
        .cancel("close", tr("Close"))
        .close_on_escape(true)
        .show()
        .await;
}
