//! Where a resolved key goes: the platform's Back takes a short Back; a
//! Long Back acts here and its release takes no step back.

use std::time::Duration;

use iptv_nav::{Action, BackGuard, Press, RemoteKey};
use nsh_core::prelude::*;

use crate::Ui;

/// Where a resolved key goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// The app applies it.
    Apply,
    /// A short Back: the platform's Back handler (`on_back_press` →
    /// `go_back`) takes that step; applying it too would go back twice.
    Platform,
    /// A Long Back: the app applies it, and the platform's Back that
    /// follows on release is swallowed.
    ApplyAndSwallow,
}

fn route(key: RemoteKey, press: Press) -> Route {
    match (key, press) {
        (RemoteKey::Back, Press::Short) => Route::Platform,
        (RemoteKey::Back, Press::Long) => Route::ApplyAndSwallow,
        _ => Route::Apply,
    }
}

pub(super) fn act(ui: Ui, key: RemoteKey, press: Press, action: Action, repeat: bool) {
    match route(key, press) {
        // The platform's Back steps back; it is still a key, so the app
        // leaves touch or pointer mode (a TV hides its visible Back again).
        Route::Platform => ui.chrome.input.set(crate::Input::Keys),
        Route::Apply => super::apply(ui, key, press, action, repeat),
        Route::ApplyAndSwallow => {
            arm_back_guard(ui);
            super::apply(ui, key, press, action, repeat);
        }
    }
}

/// A Long Back acted: its release must not also go back (`lib.rs`
/// `bind_back`), unless no release comes in time.
pub(super) fn swallow_release(ui: Ui) {
    arm_back_guard(ui);
}

fn arm_back_guard(ui: Ui) {
    let now = ui.now_ms();
    ui.chrome.back_guard.update(|g| g.long_back(now));
    let wait = Duration::from_millis(BackGuard::TIMEOUT_MS);
    let _task = set_timeout(wait, move || {
        if ui.chrome.back_guard.with_untracked(BackGuard::armed) {
            let now = ui.now_ms();
            let mut expired = false;
            ui.chrome.back_guard.update(|g| expired = g.expire(now));
            if expired {
                log::info!("iptv long back: no release came");
            }
        }
    });
}

/// The platform's Back (`on_back_press`): `true` when it is the release of
/// a Long Back that acted, which takes no step back.
pub(crate) fn swallow_back(ui: Ui) -> bool {
    if !ui.chrome.back_guard.with_untracked(BackGuard::armed) {
        return false;
    }
    let now = ui.now_ms();
    let mut swallow = false;
    ui.chrome.back_guard.update(|g| swallow = g.take(now));
    if swallow {
        log::info!("iptv back: released after a long back; no step back");
    }
    swallow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_as_a_key_acts_only_when_long() {
        assert_eq!(route(RemoteKey::Back, Press::Short), Route::Platform);
        assert_eq!(route(RemoteKey::Back, Press::Long), Route::ApplyAndSwallow);
        assert_eq!(route(RemoteKey::Left, Press::Long), Route::Apply);
        assert_eq!(route(RemoteKey::Ok, Press::Short), Route::Apply);
    }
}
