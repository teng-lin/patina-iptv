//! Wires application startup, runtime events and foreground lifecycle.

use std::{rc::Rc, time::Duration};

use nsh_core::prelude::*;

use crate::runtime_owner::{runtime, schedule, with_runtime};
use crate::{Ui, tasks};

/// Startup: browse enabled playlists together when there is more than one,
/// otherwise refresh the active source or open the wizard.
pub(crate) fn start(ui: Ui) {
    tasks::bind_owner();
    runtime().bind_tasks(tasks::spawn_in_app, |duration| Box::pin(timer(duration)));
    runtime().bind(move |event| crate::runtime_events::apply(ui, event));
    runtime().start_services();
    crate::recordings_view::bind(ui);
    // NSH drops cleanup callbacks without invoking them when the entire Rust
    // runtime exits. Capture an RAII guard so both owner disposal and runtime
    // destruction drain accepted writes while the engine thread still exists.
    struct Drain(Rc<crate::runtime::Runtime>);
    impl Drop for Drain {
        fn drop(&mut self) {
            with_runtime(self.0.clone(), || {
                if !runtime().flush_files(Duration::from_secs(5)) {
                    log::error!("iptv persistence: queued file writes did not finish at shutdown");
                }
                if !crate::favorites::flush_on_shutdown() {
                    log::error!(
                        "iptv persistence: queued favorite changes could not be saved at shutdown"
                    );
                }
                if !runtime().drain_services() {
                    log::error!("iptv persistence: store did not drain at shutdown");
                }
            });
        }
    }
    let drain = Drain(runtime());
    nsh_core::prelude::cleanup(move || drop(drain));
    // In the background the system may end the process without a teardown:
    // a zap or a settings change made just before must already be on disk.
    // Usually nothing is queued and this returns at once. The Android host
    // also waits for them in `onStop` (`TvActivity.flushPendingWrites`).
    let was_background = std::cell::Cell::new(false);
    effect(move || {
        let background = lifecycle() == nsh_core::platform::Lifecycle::Background;
        let target = crate::host::EXTERNAL_PLAYER
            .then(|| crate::external_player::handoff_target(ui))
            .flatten();
        runtime().set_foreground_for_external(!background, target.as_ref());
        if was_background.replace(background) && !background && crate::host::ANDROID {
            crate::external_player::returned(ui);
        }
        if background && !runtime().flush_files(Duration::from_secs(2)) {
            log::warn!("iptv persistence: queued file writes did not finish in the background");
        }
    });
    tasks::bind_owner();
    crate::data::stored::bind(ui);
    crate::search_view::load_saved_history(ui);
    let owner = runtime();
    owner.bind_browser(move || crate::runtime::BrowserState {
        focused_key: crate::runtime_events::focused_channel(ui)
            .and_then(|channel| schedule().channel_key(channel)),
        playing: ui.playing.get_untracked().is_some(),
    });
    tasks::spawn_in_app(async move {
        let owner = runtime();
        let _ = owner.parental_refresh().await;
        crate::runtime::data::start(&owner);
        crate::parental_view::bind(ui);
    });
}
