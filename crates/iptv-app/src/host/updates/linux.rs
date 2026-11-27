//! A viewer-confirmed full AppImage upgrade. The current process keeps running.
use super::UpdatePrompts;
use iptv_core::message::key;
use iptv_runtime::Runtime;
use iptv_runtime::app_updates::ReleaseAsset;
use nsh_base::alert_dialog;
use nsh_core::{element::pick_save_path, file_dialog::SaveDialog};
use std::rc::Rc;

pub(super) async fn update(owner: &Rc<Runtime>, prompts: &impl UpdatePrompts, asset: ReleaseAsset) {
    let image = owner.running_appimage().await;
    if prompts.cancelled() || !prompts.is_open() {
        return;
    }
    let mut prompt = alert_dialog()
        .title(prompts.translate(key("Update AppImage")))
        .cancel("cancel", prompts.translate(key("Cancel")))
        .action("save", prompts.translate(key("Save as…")))
        .close_on_escape(true);
    if let Some(image) = &image {
        prompt = prompt
            .description(image.path().display().to_string())
            .action("replace", prompts.translate(key("Replace installed file")));
    }
    let answer = prompt.show().await.ok().and_then(|r| r.action);
    if prompts.cancelled() {
        return;
    }
    if answer.as_deref() == Some("save") {
        let Some(name) = asset.url().rsplit('/').next() else {
            return;
        };
        let Ok(Some(destination)) = pick_save_path(SaveDialog::new().file_name(name)).await else {
            return;
        };
        if prompts.cancelled() {
            return;
        }
        let Some(parent) = destination.parent() else {
            return;
        };
        let can_save = owner.can_save_appimage(destination.clone()).await;
        if prompts.cancelled() {
            return;
        }
        if !can_save {
            return prompts.notify(key("Choose a writable location and a new file name."));
        }
        let directory = owner.stage_appimage(parent.to_owned()).await;
        if prompts.cancelled() {
            return;
        }
        let Some(directory) = directory else {
            return prompts.notify(key("Choose a writable location and a new file name."));
        };
        prompts.notify(key("Downloading…"));
        let file = owner.download_appimage(asset, directory).await;
        if prompts.cancelled() {
            return;
        }
        let Some(file) = file else {
            return prompts.notify(key("Update download failed. Try again."));
        };
        let commit = owner.begin_app_update_file_commit();
        let saved = owner.save_appimage(destination, file).await;
        drop(commit);
        if saved {
            prompts.notify(key("AppImage saved."));
        } else {
            prompts.notify(key("Choose a writable location and a new file name."));
        }
        return;
    }
    if answer.as_deref() != Some("replace") {
        return;
    }
    let Some(image) = image else { return };
    if owner.recording_background_needed() {
        return prompts.notify(key("Finish recording before installing the update."));
    }
    let Some(parent) = image.path().parent() else {
        return;
    };
    let file = if let Some(file) = owner.take_appimage(&asset) {
        Some(file)
    } else {
        let Some(directory) = owner.stage_appimage(parent.to_owned()).await else {
            return prompts.notify(key("Could not replace the AppImage. Try Save as…"));
        };
        if prompts.cancelled() {
            return;
        }
        prompts.notify(key("Downloading…"));
        owner.download_appimage(asset.clone(), directory).await
    };
    let Some(file) = file else {
        if prompts.cancelled() {
            return;
        }
        return prompts.notify(key(
            "Download failed. Try Save as… to choose another location.",
        ));
    };
    if prompts.cancelled() {
        return;
    }
    if !prompts.is_open() {
        owner.retain_appimage(asset, file);
        return;
    }
    if owner.recording_background_needed() {
        owner.retain_appimage(asset, file);
        return prompts.notify(key("Finish recording before installing the update."));
    }
    let confirm = alert_dialog()
        .title(prompts.translate(key("Install update?")))
        .description(image.path().display().to_string())
        .cancel("close", prompts.translate(key("Close")))
        .action("install", prompts.translate(key("Install update")))
        .close_on_escape(true)
        .show()
        .await;
    if prompts.cancelled() {
        return;
    }
    if confirm.ok().and_then(|r| r.action).as_deref() != Some("install") {
        owner.retain_appimage(asset, file);
        return;
    }
    if owner.recording_background_needed() {
        owner.retain_appimage(asset, file);
        return prompts.notify(key("Finish recording before installing the update."));
    }
    let commit = owner.begin_app_update_file_commit();
    let previous = owner.replace_appimage(image, file).await;
    drop(commit);
    let Some(previous) = previous else {
        return prompts.notify(key("Could not replace the AppImage. Try Save as…"));
    };
    let _ = alert_dialog()
        .title(prompts.translate(key("Restart Patina to use the update.")))
        .description(format!(
            "{}\n{}",
            prompts.translate(key("Previous AppImage")),
            previous.display()
        ))
        .cancel("close", prompts.translate(key("Close")))
        .close_on_escape(true)
        .show()
        .await;
}
