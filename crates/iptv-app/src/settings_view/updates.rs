//! Native About action. The package embeds its feed URL and public trust root.
use crate::i18n::tr;
use nsh_base::alert_dialog;
use nsh_core::prelude::open_url;

pub(super) use crate::host::updates::RELEASES;

pub(super) fn run(ui: crate::Ui) {
    crate::tasks::spawn_in_app(show(ui));
}

pub(crate) fn is_open(ui: crate::Ui) -> bool {
    super::state(ui, |screen| screen.page().key) == Some("about")
}

async fn show(ui: crate::Ui) {
    let (message, url, action) =
        crate::host::updates::check(crate::device::direct_android_build()).await;
    if !is_open(ui) {
        return;
    }
    let mut dialog = alert_dialog()
        .title(tr("Look for a new version"))
        .description(message)
        .close_on_escape(true)
        .cancel("close", tr("Close"));
    if url.is_some() {
        dialog = dialog.action("releases", tr("Release notes and downloads"));
    }
    if let Some(action) = &action {
        dialog = dialog.action("package", action.label());
    }
    match dialog.show().await.ok().and_then(|r| r.action).as_deref() {
        Some("releases") => {
            if let Some(url) = url {
                open(&url).await;
            }
        }
        Some("package") => {
            if let Some(action) = action {
                action.run(ui).await;
            }
        }
        _ => {}
    }
}

pub(crate) async fn open(url: &str) {
    if !open_url(url).await {
        // TVs can keep the address on screen for as long as the viewer needs.
        let _ = alert_dialog()
            .title(tr("Release notes and downloads"))
            .description(url.to_owned())
            .close_on_escape(true)
            .cancel("close", tr("Close"))
            .show()
            .await;
    }
}
