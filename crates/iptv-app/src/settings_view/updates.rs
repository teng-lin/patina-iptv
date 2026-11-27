//! Application-owned release discovery, inline notes and native installation.
use crate::i18n::tr;
use nsh_core::prelude::*;

pub(super) use crate::host::updates::RELEASES;

#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "windows",
    target_os = "macos"
))]
mod native;

pub(super) fn label() -> &'static str {
    tr("Check for updates")
}

pub(super) fn run(ui: crate::Ui) {
    #[cfg(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    ))]
    native::open(ui);
    #[cfg(not(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    )))]
    let _ = ui;
}

pub(crate) fn bind(ui: crate::Ui) {
    #[cfg(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    ))]
    native::bind(ui);
    #[cfg(not(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    )))]
    let _ = ui;
}

pub(crate) fn is_open(ui: crate::Ui) -> bool {
    ui.update_dialog_open.get()
}

pub(crate) fn close(ui: crate::Ui) {
    ui.update_dialog_open.set(false);
}

pub(crate) fn key(ui: crate::Ui, action: iptv_nav::Action) {
    #[cfg(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    ))]
    native::key(ui, action);
    #[cfg(not(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    )))]
    let _ = (ui, action);
}

pub(crate) fn layer(ui: crate::Ui) -> View {
    #[cfg(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    ))]
    {
        native::layer(ui)
    }
    #[cfg(not(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    )))]
    {
        let _ = ui;
        view()
    }
}

pub(crate) fn notice_open(ui: crate::Ui) -> bool {
    #[cfg(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    ))]
    {
        native::notice_open(ui)
    }
    #[cfg(not(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    )))]
    {
        let _ = ui;
        false
    }
}

pub(crate) fn dismiss_notice(ui: crate::Ui) {
    #[cfg(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    ))]
    native::dismiss_notice(ui);
    #[cfg(not(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    )))]
    let _ = ui;
}

pub(crate) fn notice_key(ui: crate::Ui, action: iptv_nav::Action) {
    #[cfg(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    ))]
    native::notice_key(ui, action);
    #[cfg(not(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    )))]
    let _ = (ui, action);
}

pub(crate) async fn open(url: &str) {
    if !open_url(url).await {
        let _ = nsh_base::alert_dialog()
            .title(tr("Release notes and downloads"))
            .description(url.to_owned())
            .close_on_escape(true)
            .cancel("close", tr("Close"))
            .show()
            .await;
    }
}
