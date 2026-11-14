//! Bind runtime tasks, lifecycle, and presentation to the shell owner.
//! Import, library, combined-browser and guide work live in `iptv_runtime::data`.

mod library;
pub(crate) mod pages;
pub(crate) mod stored;
use crate::Ui;
pub use crate::runtime::data::{
    Activity, DEV_PLAYLIST, Job, Playlist, Purpose, Update, delete, edit_guides, refresh,
    refresh_all, rename, set_policy, set_user_agent, source_from_draft, start_import, switch_all,
    switch_to, try_details,
};
pub use crate::runtime::data::{assign, combined, organization};
pub use crate::runtime::guides;
pub use library::Library;
use nsh_core::prelude::*;
use std::{cell::Cell, future::Future, path::PathBuf, rc::Rc, time::Duration};
thread_local! {
    /// The app's root owner: import tasks outlive the view that started them.
    static APP_OWNER: Cell<Option<Owner>> = const { Cell::new(None) };
}

/// Where the app keeps its files: the Android app's files directory,
/// `~/Library/Application Support/iptv-rs` on macOS and in the iOS app's
/// container, or
/// `~/.local/share/iptv-rs` on the other desktops.
pub fn data_dir() -> PathBuf {
    crate::runtime().data_dir().to_owned()
}

/// Runs `future` on the UI thread under the app's root owner.
pub fn spawn_in_app(future: impl Future<Output = ()> + 'static) {
    let owner = APP_OWNER.with(Cell::get).unwrap_or_else(Owner::current);
    let future = crate::in_runtime(future);
    owner.run(move || {
        // Dropping the handle detaches the task; it lives with the owner.
        drop(spawn(future));
    });
}

/// Startup: browse enabled playlists together when there is more than one,
/// otherwise refresh the active source or open the wizard.
pub fn start(ui: Ui) {
    APP_OWNER.with(|o| o.set(Some(Owner::current())));
    crate::runtime().bind_tasks(spawn_in_app, |duration| Box::pin(timer(duration)));
    crate::runtime().bind(move |event| crate::runtime_events::apply(ui, event));
    crate::runtime().start_services();
    // NSH drops cleanup callbacks without invoking them when the entire Rust
    // runtime exits. Capture an RAII guard so both owner disposal and runtime
    // destruction drain accepted writes while the engine thread still exists.
    struct Drain(Rc<crate::runtime::Runtime>);
    impl Drop for Drain {
        fn drop(&mut self) {
            crate::with_runtime(self.0.clone(), || {
                if !crate::runtime().flush_files(Duration::from_secs(5)) {
                    log::error!("iptv persistence: queued file writes did not finish at shutdown");
                }
                if !crate::favorites::flush_on_shutdown() {
                    log::error!(
                        "iptv persistence: queued favourite changes could not be saved at shutdown"
                    );
                }
                if !crate::runtime().drain_services() {
                    log::error!("iptv persistence: store did not drain at shutdown");
                }
            });
        }
    }
    let drain = Drain(crate::runtime());
    nsh_core::prelude::cleanup(move || drop(drain));
    // In the background the system may end the process without a teardown:
    // a zap or a settings change made just before must already be on disk.
    // Usually nothing is queued and this returns at once. The Android host
    // also waits for them in `onStop` (`TvActivity.flushPendingWrites`).
    effect(|| {
        let background = lifecycle() == nsh_core::platform::Lifecycle::Background;
        crate::runtime().set_foreground(!background);
        if background && !crate::runtime().flush_files(Duration::from_secs(2)) {
            log::warn!("iptv persistence: queued file writes did not finish in the background");
        }
    });
    APP_OWNER.with(|o| o.set(Some(Owner::current())));
    stored::bind(ui);
    crate::search_view::load_saved_history(ui);
    let owner = crate::runtime();
    owner.bind_browser(move || crate::runtime::BrowserState {
        focused_key: crate::runtime_events::focused_channel(ui)
            .and_then(|channel| crate::schedule().channel_key(channel)),
        playing: ui.playing.get_untracked().is_some(),
        has_vod: ui.has_vod.get_untracked(),
    });
    crate::runtime::data::start(&crate::runtime());
}
