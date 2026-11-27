//! Optional confirmation for Android TV's Back from the navigation rail.

use std::time::Duration;

use iptv_core::capabilities::{FormFactor, Host, Platform};
use iptv_settings::{Target, keys};
use nsh_core::prelude::*;

use crate::{Ui, i18n::tr};

const WINDOW_MS: u64 = 2_000;
fn prompt() -> &'static str {
    tr("Press Back again to exit")
}

pub(crate) fn offered(host: Host) -> bool {
    host.platform == Platform::Android && host.form_factor == FormFactor::Tv
}

pub(crate) fn enabled(ui: Ui) -> bool {
    offered(crate::device::host())
        && crate::settings_wiring::read(ui).bool(keys::DOUBLE_BACK_TO_EXIT, &Target::Global)
}

/// Consume the first Back. A second Back in the window falls through the
/// platform handler so Android returns to the launcher normally.
pub(crate) fn press(ui: Ui) -> bool {
    let now = ui.now_ms();
    if ui
        .chrome
        .exit_deadline
        .with_untracked(|at| at.is_some_and(|at| now < at))
    {
        ui.chrome.exit_deadline.set(None);
        return false;
    }

    let recommended = crate::device::recommended();
    let window = WINDOW_MS
        .max(recommended.controls_ms)
        .max(recommended.text_ms);
    let deadline = now.saturating_add(window);
    ui.chrome.exit_deadline.set(Some(deadline));
    announce_for_accessibility(prompt());
    ui.owner.run(|| {
        let _timer = set_timeout(Duration::from_millis(window), move || {
            if ui.chrome.exit_deadline.get_untracked() == Some(deadline) {
                ui.chrome.exit_deadline.set(None);
            }
        });
    });
    true
}

pub(crate) fn layer(ui: Ui) -> impl IntoElement {
    crate::settings_view::notice("exit-confirmation", move || {
        ui.chrome
            .exit_deadline
            .with(Option::is_some)
            .then(|| prompt().to_owned())
    })
}
