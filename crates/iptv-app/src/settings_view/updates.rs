//! Native About action. The package embeds its feed URL and public trust root.
use crate::i18n::tr;
use nsh_base::alert_dialog;
use nsh_core::prelude::open_url;

pub(super) use crate::host::updates::RELEASES;

pub(super) fn run(ui: crate::Ui) {
    crate::tasks::spawn_in_app(show(ui));
}

async fn show(ui: crate::Ui) {
    let (message, url) = crate::host::updates::check(crate::device::direct_android_build()).await;
    if super::state(ui, |screen| screen.page().key) != Some("about") {
        return;
    }
    let mut dialog = alert_dialog()
        .title(tr("Check for updates"))
        .description(message)
        .close_on_escape(true)
        .cancel("close", tr("Close"));
    if url.is_some() {
        dialog = dialog.action("releases", tr("Release notes and downloads"));
    }
    let reply = dialog.show().await;
    if let Some(url) = url
        && reply.ok().and_then(|r| r.action).as_deref() == Some("releases")
        && !open_url(&url).await
    {
        // TVs can keep the address on screen for as long as the viewer needs.
        let _ = alert_dialog()
            .title(tr("Release notes and downloads"))
            .description(url)
            .close_on_escape(true)
            .cancel("close", tr("Close"))
            .show()
            .await;
    }
}
