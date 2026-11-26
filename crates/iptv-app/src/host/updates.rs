//! Release information for the installed native package.
pub(crate) const RELEASES: &str = "https://github.com/teng-lin/patina-iptv/releases";

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "linux")]
mod linux;

pub(crate) enum PackageAction {
    #[cfg(target_os = "android")]
    Install(iptv_runtime::app_updates::ReleaseAsset),
    #[cfg(target_os = "linux")]
    AppImage(iptv_runtime::app_updates::ReleaseAsset),
    #[cfg(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android"
    ))]
    Download(String),
}

impl PackageAction {
    pub(crate) fn label(&self) -> &'static str {
        match *self {
            #[cfg(target_os = "linux")]
            Self::AppImage(_) => crate::i18n::tr("Update AppImage"),
            #[cfg(target_os = "android")]
            Self::Install(_) => crate::i18n::tr("Download and update"),
            #[cfg(any(
                target_os = "linux",
                target_os = "windows",
                target_os = "macos",
                target_os = "android"
            ))]
            Self::Download(_) => crate::i18n::tr("Download package"),
        }
    }

    pub(crate) async fn run(self, _ui: crate::Ui) {
        #[cfg(target_os = "android")]
        use crate::i18n::tr;
        match self {
            #[cfg(target_os = "linux")]
            Self::AppImage(asset) => linux::update(_ui, asset).await,
            #[cfg(any(
                target_os = "linux",
                target_os = "windows",
                target_os = "macos",
                target_os = "android"
            ))]
            Self::Download(url) => {
                crate::settings_view::updates::open(&url).await;
            }
            #[cfg(target_os = "android")]
            Self::Install(asset) => {
                let owner = crate::runtime_owner::runtime();
                let Some((code, directory)) = asset.android_code().zip(android::directory()) else {
                    return crate::settings_view::notify(tr("Update check failed. Try again."));
                };
                crate::settings_view::notify(tr("Downloading…"));
                let Some(path) = owner.download_update(asset, directory).await else {
                    return crate::settings_view::notify(tr("Update download failed. Try again."));
                };
                if !crate::settings_view::updates::is_open(_ui) {
                    return;
                }
                if owner.recording_background_needed() {
                    return crate::settings_view::notify(tr(
                        "Finish recording before installing the update.",
                    ));
                }
                let confirmed = nsh_base::alert_dialog()
                    .title(tr("Install update?"))
                    .description(tr("Android will ask you to confirm the installation."))
                    .cancel("close", tr("Close"))
                    .action("install", tr("Install update"))
                    .close_on_escape(true)
                    .show()
                    .await;
                if confirmed.ok().and_then(|r| r.action).as_deref() != Some("install") {
                    return;
                }
                if owner.recording_background_needed() {
                    return crate::settings_view::notify(tr(
                        "Finish recording before installing the update.",
                    ));
                }
                if let Ok(owned) = path.keep() {
                    if android::handoff(&owned, code) {
                        return;
                    }
                    let _ = std::fs::remove_file(owned);
                }
                crate::settings_view::notify(tr("Update download failed. Try again."));
            }
        }
    }
}

#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "windows",
    target_os = "macos"
))]
pub(crate) async fn check(
    android_build: Option<(u64, u64)>,
) -> (String, Option<String>, Option<PackageAction>) {
    use crate::i18n::tr;
    const PRODUCT: &str = "Patina";
    use iptv_runtime::app_updates::{InstalledPackage, UpdateResult};
    let version = env!("CARGO_PKG_VERSION");
    let Some(feed) = option_env!("PATINA_UPDATE_FEED") else {
        return (
            format!("{PRODUCT} {version}\n{RELEASES}"),
            Some(RELEASES.to_owned()),
            None,
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
    let mut action = None;
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
            asset,
        } => {
            if cfg!(target_os = "linux") {
                action = Some(PackageAction::Download(asset.url().to_owned()));
            }
            #[cfg(target_os = "linux")]
            if cfg!(target_arch = "x86_64")
                && option_env!("PATINA_APPIMAGE_HANDOFF") == Some("1")
                && asset.url().ends_with(".AppImage")
            {
                action = Some(PackageAction::AppImage(asset));
            }
            #[cfg(target_os = "android")]
            if option_env!("PATINA_APK_HANDOFF") == Some("1") && android_build.is_some() {
                action = Some(PackageAction::Install(asset));
            }
            let label = if preview {
                tr("Preview")
            } else {
                tr("Update available")
            };
            (format!("{label} · {PRODUCT} {version}"), Some(notes))
        }
    };
    (format!("{PRODUCT} {version}\n{message}"), url, action)
}

#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "windows"
)))]
pub(crate) async fn check(
    _: Option<(u64, u64)>,
) -> (String, Option<String>, Option<PackageAction>) {
    (String::new(), None, None)
}
