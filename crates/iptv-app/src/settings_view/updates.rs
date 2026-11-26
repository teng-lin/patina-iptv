//! Native About action. The package embeds its feed URL and public trust root.
use crate::i18n::tr;
use nsh_base::alert_dialog;
use nsh_core::prelude::open_url;

pub(super) use crate::host::updates::RELEASES;

thread_local! {
    static ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Hold the action through its result dialog. Dropping a cancelled UI task
/// also makes the row available to the next app owner on this thread.
struct UpdateDialog;

impl Drop for UpdateDialog {
    fn drop(&mut self) {
        ACTIVE.set(false);
    }
}

pub(super) fn run(ui: crate::Ui) {
    if ACTIVE.replace(true) {
        return;
    }
    let dialog = UpdateDialog;
    crate::tasks::spawn_in_app(async move {
        let _dialog = dialog;
        show(ui).await;
    });
}

async fn show(ui: crate::Ui) {
    let (message, url) = crate::host::updates::check(crate::device::direct_android_build()).await;
    if super::state(ui, |screen| screen.page().key) != Some("about") {
        return;
    }
    let mut dialog = alert_dialog()
        .title(super::words::label("general.check_updates"))
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
