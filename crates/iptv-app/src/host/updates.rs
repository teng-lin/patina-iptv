//! Release information for the installed native package.
pub(crate) const RELEASES: &str = "https://github.com/teng-lin/patina-iptv/releases";

pub(crate) fn feed(android_build: Option<(u64, u64)>) -> Option<&'static str> {
    if cfg!(target_os = "android") && android_build.is_none() {
        return None;
    }
    option_env!("PATINA_UPDATE_FEED")
}

#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "windows",
    target_os = "macos"
))]
pub(crate) async fn check(android_build: Option<(u64, u64)>) -> (String, Option<String>) {
    use crate::i18n::tr;
    const PRODUCT: &str = "Patina";
    use iptv_runtime::app_updates::{InstalledPackage, UpdateResult};
    let version = env!("CARGO_PKG_VERSION");
    let Some(feed) = feed(android_build) else {
        return (
            format!("{PRODUCT} {version}\n{RELEASES}"),
            Some(RELEASES.to_owned()),
        );
    };
    crate::settings_view::notify(tr("Checking…"));
    let owner = crate::runtime_owner::runtime();
    let mut installed = InstalledPackage::native(version);
    if let Some((code, api)) = android_build {
        installed.format = Some("apk".into());
        installed.android_code = Some(code);
        installed.minimum = serde_json::json!({"androidApi": api});
    }
    let root = include_bytes!(concat!(env!("OUT_DIR"), "/update-root.json"));
    let result = owner.check_app_update(feed, root, installed).await;
    let (message, url) = match result {
        UpdateResult::Current => (tr("Patina is up to date.").to_owned(), None),
        UpdateResult::Unavailable => (
            tr("Update check failed. Try again.").to_owned(),
            Some(RELEASES.to_owned()),
        ),
        UpdateResult::RequirementsUnconfirmed => (
            tr("Check the system requirements in the release notes.").to_owned(),
            Some(RELEASES.to_owned()),
        ),
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
            (format!("{label} · {PRODUCT} {version}"), Some(notes))
        }
    };
    (format!("{PRODUCT} {version}\n{message}"), url)
}

#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "windows"
)))]
pub(crate) async fn check(_: Option<(u64, u64)>) -> (String, Option<String>) {
    (String::new(), None)
}
