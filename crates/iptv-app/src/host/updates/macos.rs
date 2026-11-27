//! Sparkle owns authenticated replacement and relaunch of signed macOS bundles.
//! Discovery and scheduling remain runtime-owned; preview builds use the DMG link.
use crate::{Ui, i18n::tr, settings_view::notify};
use std::{ffi::CString, time::Duration};

unsafe extern "C" {
    fn patina_sparkle_supported() -> bool;
    fn patina_sparkle_start(
        version: *const std::ffi::c_char,
        failure: *const std::ffi::c_char,
    ) -> u64;
    fn patina_sparkle_status(session: u64) -> u8;
    fn patina_sparkle_can_cancel() -> bool;
    fn patina_sparkle_try_cancel() -> bool;
    fn patina_sparkle_allow_restart(session: u64);
    fn patina_sparkle_cancel(session: u64);
}

struct Session(u64);

impl Drop for Session {
    fn drop(&mut self) {
        // Root disposal revokes pending restart authorization synchronously.
        // The native side dismisses its UI on AppKit's main queue.
        // SAFETY: the native mailbox accepts generation values from any thread.
        unsafe { patina_sparkle_cancel(self.0) };
    }
}

pub(super) fn supported() -> bool {
    // SAFETY: this is a thread-safe bundle/signing capability query. AppKit
    // updater objects are created separately, asynchronously on its main queue.
    unsafe { patina_sparkle_supported() }
}

pub(super) fn cancelable() -> bool {
    // SAFETY: this queries the synchronized stage mailbox only.
    unsafe { patina_sparkle_can_cancel() }
}

pub(super) fn try_cancel_current() -> bool {
    // SAFETY: cancellation is accepted atomically before extraction and the
    // public Sparkle cancellation callback runs on AppKit's main queue.
    unsafe { patina_sparkle_try_cancel() }
}

pub(super) async fn update(ui: Ui, version: String) {
    let owner = crate::runtime_owner::runtime();
    if owner.recording_background_needed() {
        return notify(tr("Finish recording before installing the update."));
    }
    let Ok(version) = CString::new(version) else {
        return notify(tr("The installer could not be verified or opened."));
    };
    let Ok(failure) = CString::new(tr("The installer could not be verified or opened.")) else {
        return;
    };
    // SAFETY: the bridge copies the version before returning. All following
    // operations use a generation token, never a pointer to Ui or the runtime.
    let token = unsafe { patina_sparkle_start(version.as_ptr(), failure.as_ptr()) };
    if token == 0 {
        return notify(tr("The installer could not be verified or opened."));
    }
    let session = Session(token);
    let mut host_stopped = false;
    let outcome = loop {
        if !ui.owner.is_live() {
            return;
        }
        if ui.screens.update_cancel_requested.get_untracked() && try_cancel_current() {
            break 4;
        }
        // This future belongs to the app root, so owner/Ui access remains on
        // its shell thread. AppKit callbacks publish only mailbox status.
        // SAFETY: status reads the synchronized generation mailbox only.
        match unsafe { patina_sparkle_status(session.0) } {
            0 | 4 => break 4,
            5 => break 5,
            8 => break 8,
            3 if !owner.recording_background_needed() && owner.shutdown_recording_host() => {
                if owner.recording_background_needed() {
                    owner.start_recordings();
                } else {
                    host_stopped = true;
                    crate::player::watch_time::flush(ui);
                    // SAFETY: the native generation fence accepts approval only for this live session.
                    unsafe { patina_sparkle_allow_restart(session.0) };
                }
            }
            _ => {}
        }
        let (send, reply) = futures_channel::oneshot::channel();
        let _timer = nsh_core::prelude::set_timeout(Duration::from_millis(100), move || {
            let _ = send.send(());
        });
        let _ = reply.await;
    };
    // Native revocation/cleanup must precede restoring the recording host.
    drop(session);
    // A failed or dismissed installation may leave this process running after
    // its recording host was stopped for relaunch. Restore normal operation.
    if host_stopped {
        owner.start_recordings();
    }
    if outcome == 5 {
        notify(tr("The installer could not be verified or opened."));
    } else if outcome == 8 {
        notify(tr(
            "The update is prepared. Quit Patina to finish installing it, or check again to retry.",
        ));
    }
}
