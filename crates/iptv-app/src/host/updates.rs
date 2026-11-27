//! Release information for the installed native package.
pub(crate) const RELEASES: &str = "https://github.com/teng-lin/patina-iptv/releases";

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

pub(crate) enum PackageAction {
    #[cfg(target_os = "macos")]
    MacOS(String),
    #[cfg(target_os = "android")]
    Install(iptv_runtime::app_updates::ReleaseAsset),
    #[cfg(target_os = "linux")]
    AppImage(iptv_runtime::app_updates::ReleaseAsset),
    #[cfg(windows)]
    Windows(iptv_runtime::app_updates::ReleaseAsset, &'static str),
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
            #[cfg(target_os = "macos")]
            Self::MacOS(_) => crate::i18n::tr("Download and update"),
            #[cfg(target_os = "linux")]
            Self::AppImage(_) => crate::i18n::tr("Update AppImage"),
            #[cfg(windows)]
            Self::Windows(..) => crate::i18n::tr("Download and update"),
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
            #[cfg(target_os = "macos")]
            Self::MacOS(version) => macos::update(_ui, version).await,
            #[cfg(target_os = "linux")]
            Self::AppImage(asset) => linux::update(_ui, asset).await,
            #[cfg(windows)]
            Self::Windows(asset, publisher) => {
                use crate::i18n::tr;
                let owner = crate::runtime_owner::runtime();
                if owner.recording_background_needed() {
                    return crate::settings_view::notify(tr(
                        "Finish recording before installing the update.",
                    ));
                }
                crate::settings_view::notify(tr("Downloading…"));
                let Some(stage) = owner
                    .download_update(asset.clone(), std::env::temp_dir())
                    .await
                else {
                    return crate::settings_view::notify(tr("Update download failed. Try again."));
                };
                if !crate::settings_view::updates::is_open(_ui) {
                    return;
                }
                crate::settings_view::notify(tr("Verifying installer…"));
                let Some(mut setup) = owner
                    .prepare_windows_setup(
                        stage,
                        asset,
                        publisher,
                        tr("The installer could not be verified or opened."),
                    )
                    .await
                else {
                    return crate::settings_view::notify(tr(
                        "The installer could not be verified or opened.",
                    ));
                };
                if !crate::settings_view::updates::is_open(_ui) {
                    return;
                }
                let confirmed = nsh_base::alert_dialog()
                    .title(tr("Install update?"))
                    .description(tr("Patina will close before the installer opens."))
                    .action("install", tr("Install update"))
                    .cancel("close", tr("Cancel"))
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
                if !owner.shutdown_recording_host() {
                    return crate::settings_view::notify(tr(
                        "The recording host is still closing. Try updating again.",
                    ));
                }
                if owner.recording_background_needed() {
                    owner.start_recordings();
                    return crate::settings_view::notify(tr(
                        "Finish recording before installing the update.",
                    ));
                }
                if setup.install_after_exit() {
                    crate::desktop::close_after_shutdown(_ui);
                } else {
                    owner.start_recordings();
                    crate::settings_view::notify(tr(
                        "The installer could not be verified or opened.",
                    ));
                }
            }

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
                if owner.recording_background_needed() {
                    return crate::settings_view::notify(tr(
                        "Finish recording before installing the update.",
                    ));
                }
                let progress = nsh_base::alert_dialog()
                    .title(tr("Downloading…"))
                    .cancel("cancel", tr("Cancel"))
                    .close_on_escape(true)
                    .show();
                let closer = progress.closer();
                let path = tokio::select! {
                    biased;
                    _ = progress => return,
                    path = owner.download_update(asset, directory) => path,
                };
                if let Some(closer) = closer {
                    closer.close();
                }
                let Some(path) = path else {
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
                crate::settings_view::notify(if owner.recording_background_needed() {
                    tr("Finish recording before installing the update.")
                } else {
                    tr("Android could not open the installer. Try again from Settings.")
                });
            }
        }
    }
}

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
pub(crate) fn installed() -> iptv_runtime::app_updates::InstalledPackage {
    let mut installed =
        iptv_runtime::app_updates::InstalledPackage::native(env!("CARGO_PKG_VERSION"));
    if let Some((code, api)) = crate::device::direct_android_build() {
        installed.format = Some("apk".into());
        installed.android_code = Some(code);
        installed.minimum = serde_json::json!({"androidApi": api});
    }
    installed
}

/// Discovery stays usable without installation credentials. Only authenticated
/// metadata (or Sparkle's signed appcast) can authorize an in-app installation.
#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "windows",
    target_os = "macos"
))]
pub(crate) async fn action_for(
    check: &iptv_runtime::app_updates::github::ReleaseCheck,
) -> Result<Option<PackageAction>, &'static str> {
    use iptv_runtime::app_updates::{UpdateResult, github::ReleaseStatus};
    if check.status != ReleaseStatus::UpdateAvailable {
        return Ok(None);
    }
    let Some(download) = check.release.download.as_ref() else {
        return Ok(None);
    };
    let manual = || Ok(Some(PackageAction::Download(download.url.clone())));
    #[cfg(target_os = "macos")]
    if macos::supported() {
        return Ok(Some(PackageAction::MacOS(check.release.version.clone())));
    }
    let Some(feed) = feed(crate::device::direct_android_build()) else {
        return manual();
    };
    let root = include_bytes!(concat!(env!("OUT_DIR"), "/update-root.json"));
    let verified = crate::runtime_owner::runtime()
        .check_app_update(feed, root, installed())
        .await;
    let UpdateResult::Available {
        version,
        preview: _preview,
        asset,
        ..
    } = verified
    else {
        // Verification failure never falls back to an unverified installer.
        return Err(crate::i18n::tr(
            "The update could not be verified. Try again.",
        ));
    };
    if version != check.release.version || asset.url() != download.url {
        return Err(crate::i18n::tr(
            "The update could not be verified. Try again.",
        ));
    }
    #[cfg(target_os = "linux")]
    if cfg!(target_arch = "x86_64")
        && option_env!("PATINA_APPIMAGE_HANDOFF") == Some("1")
        && asset.url().ends_with(".AppImage")
    {
        return Ok(Some(PackageAction::AppImage(asset)));
    }
    #[cfg(windows)]
    if !_preview
        && let Some(publisher) = option_env!("PATINA_WINDOWS_PUBLISHER").map(str::trim)
        && !publisher.is_empty()
    {
        return Ok(Some(PackageAction::Windows(asset, publisher)));
    }
    #[cfg(target_os = "android")]
    if option_env!("PATINA_APK_HANDOFF") == Some("1")
        && crate::device::direct_android_build().is_some()
    {
        return Ok(Some(PackageAction::Install(asset)));
    }
    manual()
}

#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "windows",
    target_os = "macos"
))]
pub(crate) fn automatic_install_supported() -> bool {
    #[cfg(target_os = "macos")]
    if macos::supported() {
        return true;
    }
    if feed(crate::device::direct_android_build()).is_none() {
        return false;
    }
    if cfg!(target_os = "android") {
        return option_env!("PATINA_APK_HANDOFF") == Some("1");
    }
    if cfg!(target_os = "linux") {
        return option_env!("PATINA_APPIMAGE_HANDOFF") == Some("1");
    }
    cfg!(windows) && option_env!("PATINA_WINDOWS_PUBLISHER").is_some_and(|v| !v.trim().is_empty())
}
