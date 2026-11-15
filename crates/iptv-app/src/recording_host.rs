//! OS execution hooks. Provider identities never cross this boundary.
#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub(crate) use android::schedule;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::activate_existing;
#[cfg(not(target_os = "android"))]
pub(crate) fn schedule(next: i64, _active: bool) {
    #[cfg(windows)]
    windows::schedule(next);
    #[cfg(not(windows))]
    let _ = next;
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod startup;
pub(crate) fn command(key: &str) {
    if key == "recordings.background_status" {
        crate::settings_view::notify(crate::i18n::tr(if cfg!(target_os = "android") {
            "Android keeps capture in a foreground service. Exact alarms need Special app access. Force stop prevents recordings; Android may limit background recording to six hours."
        } else {
            "Closing the window keeps accepted recordings running. Enable start at login to recover after restart. Linux and macOS cannot wake a sleeping or powered-off computer; leave it awake. Quit stops the recorder."
        }));
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
                // A schedule may predate login opt-in. Arm its wake trigger now
                // rather than waiting for an unrelated catalogue transition.
                let jobs = owner.recording_snapshot();
                let next = jobs
                    .iter()
                    .filter(|job| job.status == iptv_core::recording::Status::Scheduled)
                    .map(|job| job.request.start)
                    .min()
                    .unwrap_or(0);
                schedule(
                    next,
                    jobs.iter()
                        .any(|job| job.status == iptv_core::recording::Status::Recording),
                );
            }
            crate::settings_view::notify(crate::i18n::tr(if matches!(result, Ok(Ok(Ok(())))) {
                "Recording startup preference saved"
            } else {
                "Recording startup preference could not be saved"
            }));
        });
    }
}
