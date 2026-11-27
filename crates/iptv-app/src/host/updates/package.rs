//! Native package authorization, selection and installation.
use std::rc::Rc;

use iptv_core::{capabilities::Platform, message::key};
use iptv_runtime::Runtime;

#[cfg(windows)]
use super::until_update_cancelled;
use super::{Metadata, UpdatePrompts};

pub(crate) enum PackageAction {
    #[cfg(target_os = "macos")]
    MacOS(String),
    #[cfg(target_os = "android")]
    Install(iptv_runtime::app_updates::ReleaseAsset),
    #[cfg(target_os = "linux")]
    AppImage(iptv_runtime::app_updates::ReleaseAsset),
    #[cfg(windows)]
    Windows(iptv_runtime::app_updates::ReleaseAsset, &'static str),
    Download(String),
}

impl PackageAction {
    pub(crate) fn label(&self) -> &'static str {
        match *self {
            #[cfg(target_os = "macos")]
            Self::MacOS(_) => key("Download and update"),
            #[cfg(target_os = "linux")]
            Self::AppImage(_) => key("Update AppImage"),
            #[cfg(windows)]
            Self::Windows(..) => key("Download and update"),
            #[cfg(target_os = "android")]
            Self::Install(_) => key("Download and update"),
            Self::Download(_) => key("Download package"),
        }
    }

    pub(crate) async fn run(self, _owner: &Rc<Runtime>, prompts: &impl UpdatePrompts) {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOS(version) => super::macos::update(_owner, prompts, version).await,
            #[cfg(target_os = "linux")]
            Self::AppImage(asset) => super::linux::update(_owner, prompts, asset).await,
            #[cfg(windows)]
            Self::Windows(asset, publisher) => {
                let owner = _owner;
                if owner.recording_background_needed() {
                    return prompts.notify(key("Finish recording before installing the update."));
                }
                let mut setup = if let Some(setup) = owner.take_windows_setup(&asset) {
                    setup
                } else {
                    prompts.notify(key("Downloading…"));
                    let stage = if let Some(stage) = owner.take_update_download(&asset) {
                        Some(stage)
                    } else {
                        owner
                            .download_update(asset.clone(), std::env::temp_dir())
                            .await
                    };
                    let Some(stage) = stage else {
                        if prompts.cancelled() {
                            return;
                        }
                        return prompts.notify(key("Update download failed. Try again."));
                    };
                    if prompts.cancelled() {
                        return;
                    }
                    if !prompts.is_open() {
                        owner.retain_update_download(asset, stage);
                        return;
                    }
                    prompts.notify(key("Verifying installer…"));
                    let Some(result) = until_update_cancelled(
                        prompts,
                        owner.prepare_windows_setup(
                            stage,
                            asset.clone(),
                            publisher,
                            prompts
                                .translate(key("The installer could not be verified or opened.")),
                        ),
                    )
                    .await
                    else {
                        return;
                    };
                    let Some(setup) = result else {
                        if prompts.cancelled() {
                            return;
                        }
                        return prompts
                            .notify(key("The installer could not be verified or opened."));
                    };
                    setup
                };
                if prompts.cancelled() {
                    return;
                }
                if !prompts.is_open() {
                    owner.retain_windows_setup(asset, setup);
                    return;
                }
                let confirmed = nsh_base::alert_dialog()
                    .title(prompts.translate(key("Install update?")))
                    .description(
                        prompts.translate(key("Patina will close before the installer opens.")),
                    )
                    .action("install", prompts.translate(key("Install update")))
                    .cancel("close", prompts.translate(key("Cancel")))
                    .close_on_escape(true)
                    .show()
                    .await;
                if prompts.cancelled() {
                    return;
                }
                if confirmed.ok().and_then(|r| r.action).as_deref() != Some("install") {
                    owner.retain_windows_setup(asset, setup);
                    return;
                }
                if owner.recording_background_needed() {
                    owner.retain_windows_setup(asset, setup);
                    return prompts.notify(key("Finish recording before installing the update."));
                }
                if !owner.shutdown_recording_host() {
                    owner.retain_windows_setup(asset, setup);
                    return prompts.notify(key(
                        "The recording host is still closing. Try updating again.",
                    ));
                }
                if owner.recording_background_needed() {
                    owner.start_recordings();
                    owner.retain_windows_setup(asset, setup);
                    return prompts.notify(key("Finish recording before installing the update."));
                }
                if setup.install_after_exit() {
                    prompts.close_after_shutdown();
                } else {
                    owner.start_recordings();
                    prompts.notify(key("The installer could not be verified or opened."));
                }
            }

            Self::Download(url) => {
                prompts.open_url(&url).await;
            }
            #[cfg(target_os = "android")]
            Self::Install(asset) => {
                let owner = _owner;
                let Some((code, directory)) = asset.android_code().zip(super::android::directory())
                else {
                    return prompts.notify(key("Update check failed. Try again."));
                };
                if owner.recording_background_needed() {
                    return prompts.notify(key("Finish recording before installing the update."));
                }
                let path = if let Some(path) = owner.take_update_download(&asset) {
                    Some(path)
                } else {
                    owner.download_update(asset.clone(), directory).await
                };
                let Some(path) = path else {
                    if !prompts.cancelled() {
                        prompts.notify(key("Update download failed. Try again."));
                    }
                    return;
                };
                if prompts.cancelled() {
                    return;
                }
                if !prompts.is_open() {
                    owner.retain_update_download(asset, path);
                    return;
                }
                if owner.recording_background_needed() {
                    owner.retain_update_download(asset, path);
                    return prompts.notify(key("Finish recording before installing the update."));
                }
                let confirmed = nsh_base::alert_dialog()
                    .title(prompts.translate(key("Install update?")))
                    .description(
                        prompts.translate(key("Android will ask you to confirm the installation.")),
                    )
                    .cancel("close", prompts.translate(key("Close")))
                    .action("install", prompts.translate(key("Install update")))
                    .close_on_escape(true)
                    .show()
                    .await;
                if prompts.cancelled() {
                    return;
                }
                if confirmed.ok().and_then(|r| r.action).as_deref() != Some("install") {
                    owner.retain_update_download(asset, path);
                    return;
                }
                if owner.recording_background_needed() {
                    owner.retain_update_download(asset, path);
                    return prompts.notify(key("Finish recording before installing the update."));
                }
                if let Ok(owned) = path.keep() {
                    if super::android::handoff(&owned, code) {
                        return;
                    }
                    let _ = std::fs::remove_file(owned);
                }
                prompts.notify(if owner.recording_background_needed() {
                    key("Finish recording before installing the update.")
                } else {
                    key("Android could not open the installer. Try again from Settings.")
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

pub(super) fn installed(metadata: Metadata) -> iptv_runtime::app_updates::InstalledPackage {
    let mut installed =
        iptv_runtime::app_updates::InstalledPackage::native(env!("CARGO_PKG_VERSION"));
    // The device host also drives the existing synthetic Android app scenarios.
    if metadata.platform == Platform::Android {
        installed.platform = "android";
        installed.architecture = match std::env::consts::ARCH {
            "aarch64" => "arm64-v8a",
            "arm" => "armeabi-v7a",
            architecture => architecture,
        };
        #[cfg(feature = "test-support")]
        if std::env::var("IPTV_TEST_PLATFORM").as_deref() == Ok("android") {
            // The synthetic device represents the same supported Android ABI
            // on x86 CI hosts as on an ARM Mac.
            installed.architecture = "arm64-v8a";
        }
        installed.format = None;
        installed.minimum = serde_json::json!({"androidApi": 26});
    }
    if let Some((code, api)) = metadata.android_build {
        installed.format = Some("apk".into());
        installed.android_code = Some(code);
        installed.minimum = serde_json::json!({"androidApi": api});
    }
    installed
}

/// Discovery stays usable without installation credentials. Only authenticated
/// metadata (or Sparkle's signed appcast) can authorize an in-app installation.
pub(crate) async fn action_for(
    owner: &Rc<Runtime>,
    metadata: Metadata,
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
    if super::macos::supported() {
        return Ok(Some(PackageAction::MacOS(check.release.version.clone())));
    }
    if !automatic_install_supported(metadata) {
        return manual();
    }
    let Some(feed) = feed(metadata.android_build) else {
        return manual();
    };
    let root = include_bytes!(concat!(env!("OUT_DIR"), "/update-root.json"));
    let verified = owner
        .check_app_update(feed, root, installed(metadata))
        .await;
    let UpdateResult::Available {
        version,
        preview: _preview,
        asset,
        ..
    } = verified
    else {
        // Verification failure never falls back to an unverified installer.
        return Err(key("The update could not be verified. Try again."));
    };
    if version != check.release.version || asset.url() != download.url {
        return Err(key("The update could not be verified. Try again."));
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
    if option_env!("PATINA_APK_HANDOFF") == Some("1") && metadata.android_build.is_some() {
        return Ok(Some(PackageAction::Install(asset)));
    }
    manual()
}

pub(crate) fn automatic_install_supported(metadata: Metadata) -> bool {
    #[cfg(target_os = "macos")]
    if super::macos::supported() {
        return true;
    }
    if feed(metadata.android_build).is_none() {
        return false;
    }
    if cfg!(target_os = "android") {
        return option_env!("PATINA_APK_HANDOFF") == Some("1");
    }
    #[cfg(target_os = "linux")]
    {
        let format = std::env::current_exe()
            .ok()
            .and_then(|exe| std::fs::read_to_string(exe.parent()?.join("patina-package")).ok());
        appimage_install_enabled(option_env!("PATINA_APPIMAGE_HANDOFF"), format.as_deref())
    }
    #[cfg(not(target_os = "linux"))]
    {
        cfg!(windows)
            && option_env!("PATINA_WINDOWS_PUBLISHER").is_some_and(|v| !v.trim().is_empty())
    }
}

#[cfg(any(
    target_os = "linux",
    all(
        test,
        any(target_os = "android", target_os = "windows", target_os = "macos")
    )
))]
fn appimage_install_enabled(handoff: Option<&str>, format: Option<&str>) -> bool {
    handoff == Some("1") && format.is_some_and(|format| format.trim() == "appimage")
}

/// Whether the active native installer still offers genuine cancellation.
pub(crate) fn can_cancel_install(_owner: &Runtime) -> bool {
    #[cfg(target_os = "macos")]
    return super::macos::cancelable();
    #[cfg(target_os = "linux")]
    return !_owner.app_update_file_commit_active();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    true
}

/// Accept cancellation before callers change UI state or persist a skip.
pub(crate) fn try_cancel_current(_owner: &Runtime) -> bool {
    #[cfg(target_os = "macos")]
    return super::macos::try_cancel_current();
    #[cfg(target_os = "linux")]
    return !_owner.app_update_file_commit_active();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    true
}

#[cfg(all(
    test,
    any(
        target_os = "android",
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    )
))]
mod tests {
    use super::*;

    #[test]
    fn android_discovery_waits_for_the_channel_and_direct_package_identity() {
        let mut metadata = Metadata {
            platform: Platform::Android,
            direct_apk: None,
            android_build: None,
        };
        assert!(!metadata.ready());
        assert!(
            Metadata {
                platform: Platform::Macos,
                ..metadata
            }
            .ready()
        );
        metadata.direct_apk = Some(true);
        assert!(!metadata.ready());
        metadata.android_build = Some((42, 36));
        assert!(metadata.ready());
        metadata.direct_apk = Some(false);
        metadata.android_build = None;
        assert!(metadata.ready());
    }

    #[test]
    fn a_shared_linux_binary_only_installs_from_an_appimage_package() {
        assert!(appimage_install_enabled(Some("1"), Some("appimage\n")));
        for format in [None, Some("deb"), Some("rpm"), Some("pacman")] {
            assert!(!appimage_install_enabled(Some("1"), format));
        }
        assert!(!appimage_install_enabled(None, Some("appimage")));
        assert!(!appimage_install_enabled(Some("0"), Some("appimage")));
    }
}
