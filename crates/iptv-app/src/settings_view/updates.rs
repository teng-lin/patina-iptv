//! Native About action. The package embeds its feed URL and public trust root.
use crate::i18n::tr;
use nsh_base::alert_dialog;
use nsh_core::prelude::open_url;

pub(super) const RELEASES: &str = "https://github.com/teng-lin/patina-iptv/releases";

pub(super) fn run() {
    crate::tasks::spawn_in_app(show());
}

async fn show() {
    #[cfg(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    ))]
    let (message, url) = check().await;
    #[cfg(not(any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    )))]
    let (message, url) = (String::new(), None::<String>);

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

#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "windows",
    target_os = "macos"
))]
async fn check() -> (String, Option<String>) {
    use iptv_runtime::app_updates::{InstalledPackage, UpdateResult};
    let version = env!("CARGO_PKG_VERSION");
    let Some(feed) = option_env!("PATINA_UPDATE_FEED") else {
        return (
            format!("Patina {version}\n{RELEASES}"),
            Some(RELEASES.to_owned()),
        );
    };
    super::notify(tr("Checking…"));
    let owner = crate::runtime_owner::runtime();
    let installed = InstalledPackage::native(version);
    let root = include_bytes!(concat!(env!("OUT_DIR"), "/update-root.json"));
    let result = owner.check_app_update(feed, root, installed).await;
    let (message, url) = match result {
        UpdateResult::Current => (tr("Patina is up to date.").to_owned(), None),
        UpdateResult::Unavailable => (tr("Update check failed. Try again.").to_owned(), None),
        UpdateResult::NoCompatiblePackage => (
            tr("No compatible package found.").to_owned(),
            Some(RELEASES.to_owned()),
        ),
        UpdateResult::Available {
            version,
            notes,
            preview,
        } => {
            let label = if preview {
                tr("Preview")
            } else {
                tr("Update available")
            };
            (format!("{label} · Patina {version}"), Some(notes))
        }
    };
    (format!("Patina {version}\n{message}"), url)
}
