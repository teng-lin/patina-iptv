//! OS execution hooks. Provider identities never cross this boundary.
#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub(crate) use android::{schedule, set_language};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::activate_existing;
#[cfg(not(target_os = "android"))]
pub(crate) fn schedule(next: i64, active: bool) -> iptv_core::recording::WakeCapability {
    // A desktop stays awake while a capture runs (#275).
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    super::keep_awake::set(active);
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let _ = active;
    #[cfg(windows)]
    return windows::schedule(next);
    #[cfg(not(windows))]
    {
        let _ = next;
        iptv_core::recording::WakeCapability::AwakeOnly
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod startup;
pub(crate) fn command(key: &str) {
    if key == "recordings.background_status" {
        let capability = crate::runtime().recording_capabilities();
        if let Some(message) = iptv_presentation::capabilities::recording_notice(capability) {
            crate::settings_view::notify(crate::i18n::tr(message));
            return;
        }
        crate::settings_view::notify(crate::i18n::tr(
            if crate::capabilities::host().alarm_permission() {
                "Android keeps capture in a foreground service. Exact alarms need Special app access. Force stop prevents recordings; Android may limit background recording to six hours."
            } else if !capability.background {
                "Keep Patina open and this device awake for recordings. Recordings use local app storage and are excluded from backups."
            } else {
                "Closing the window keeps accepted recordings running. Enable start at login to recover after restart. \
             Linux and macOS cannot wake a sleeping or powered-off computer; leave it awake. Quit stops the recorder."
            },
        ));
        return;
    }
    #[cfg(target_os = "android")]
    {
        android::request_alarm_access();
    }
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    {
        let directory = crate::runtime().data_dir().to_owned();
        let enabled = key == "recordings.enable_startup";
        let owner = crate::runtime();
        crate::data::spawn_in_app(async move {
            let result = owner
                .spawn_network(async move {
                    tokio::task::spawn_blocking(move || startup::install(&directory, enabled)).await
                })
                .await;
            if enabled && matches!(result, Ok(Ok(Ok(())))) {
                // Re-register existing intent on the native worker and surface
                // any denied capability through the normal recording notice.
                let _ = owner.refresh_recording_wake().await;
            }
            crate::settings_view::notify(crate::i18n::tr(if matches!(result, Ok(Ok(Ok(())))) {
                "Recording startup preference saved"
            } else {
                "Recording startup preference could not be saved"
            }));
        });
    }
}
