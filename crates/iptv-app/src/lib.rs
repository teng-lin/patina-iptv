//! iptv-rs NSH shell (TV + tablet): remote (D-pad) and touch drive one
//! app-owned focus model (`guide.rs`, `browse.rs`); the layout follows the
//! reference UX's regions (header with the preview and the focused
//! program, navigation rail and groups beside the guide grid) over one
//! persistent video layer (`video.rs`). The guide shows one list at a time (`lists.rs`).

#[cfg(patina_airplay_probe)]
#[path = "../../../tools/apple-airplay-probe/view.rs"]
mod airplay_probe;
#[cfg(patina_airplay_probe)]
pub use airplay_probe::AirPlayProbe;

/// Guide values and navigation used by the shell.
pub mod guide;
mod host;
mod kit;
mod library_ui;
mod playback;
mod screens;
mod shell;
pub mod theme;
mod ui;
mod wiring;

#[allow(unused_imports)]
use iptv_runtime as runtime;
#[allow(unused_imports)]
use kit::i18n;
#[allow(unused_imports)]
use wiring::runtime_owner::{self, runtime};

pub use guide::{header::debug_overlay, timeline};
pub use host::set_tui_mode;
#[cfg(feature = "test-support")]
pub use kit::i18n::pseudo_fixed_for_test;
/// Take keys as an Android remote's raw Down/Repeat/Up phases (headless tests).
#[cfg(feature = "test-support")]
pub use kit::input::use_remote_key_phases;
pub use kit::{layout, startup};
// A browser host exports nothing: the glob is empty there.
#[allow(unused_imports)]
#[doc(inline)]
pub use host::exports::*;
/// The document pickers a device has, and the ones headless apps get.
#[cfg(feature = "test-support")]
pub use library_ui::backup::{Pickers, set_pickers_for_test};
#[doc(hidden)]
pub use playback::video::has_source as player_has_source;
#[doc(hidden)]
pub use playback::video::opens as player_opens;
pub use shell::app::App;
pub use shell::back::confirm_leaving;
/// Whether a keyboard binding fires on auto-repeats now (headless tests).
#[cfg(feature = "test-support")]
pub use shell::input::keyboard_repeats;
pub use time::clock;
pub use ui::{Input, Playback, SharedUi, Stats, Ui, ViewMode, channel_name, in_archive, tint};
pub use wiring::runtime_owner::schedule;

pub(crate) use shell::app::{guide_chrome_pointer_events, pointer_moved, toggle_panels};
pub(crate) use shell::back::{ask_before_leaving, go_back, leave};
pub(crate) use ui::{ChromeUi, GuideUi, PlaybackUi, ScreensUi, pointer_down, pointer_input};

/// The app's guide clock and its UI tick.
pub mod time {
    pub use crate::guide::clock::{clock, date_only, label, now_minute, time_only, wall_unix};
    pub use crate::guide::guide_clock::{start, tick};
}

/// iOS (an iPad): a tap beside the open columns or list closes them.
pub(crate) const IOS: bool = crate::host::IPAD;

use nsh_core::prelude::*;

#[native_module("iptv-rs/native")]
fn native_module(registry: &mut Registry) {
    registry.register::<App>();
}

/// Whether a stored source has a guide timer or an in-flight guide read,
/// for the headless tests of what keeps its periodic checks alive.
#[cfg(feature = "test-support")]
pub fn guide_check_pending(source: iptv_runtime::model::SourceId) -> bool {
    host::host_handle().is_some_and(|owner| owner.guide_check_registered(source))
}

#[cfg(feature = "test-support")]
pub fn refuse_work(count: usize) {
    host::host_handle()
        .expect("refuse_work needs a started app")
        .refuse_work(count);
}

#[cfg(feature = "test-support")]
pub fn hold_reports(held: bool) {
    host::host_handle()
        .expect("hold_reports needs a started app")
        .hold_reports(held);
}

#[cfg(feature = "test-support")]
pub fn reports_waiting() -> usize {
    host::host_handle().map_or(0, |owner| owner.reports_waiting())
}

#[cfg(feature = "test-support")]
pub fn wizard_reports_delivered() -> usize {
    host::host_handle().map_or(0, |owner| owner.wizard_reports_delivered())
}

#[doc(hidden)]
pub use iptv_runtime::execution::recovering_worker_spawn;

/// Keeps the store's writer busy, as a long catalog or guide publish does,
/// for the headless tests of what must not wait for it.
#[cfg(feature = "test-support")]
pub fn occupy_writer(duration: std::time::Duration) {
    // The headless tests drive the app from another thread: this reaches the
    // running root's owner, never one made for the occasion (a test must not
    // pass by occupying an unused store).
    host::host_handle()
        .expect("occupy_writer needs a started app")
        .occupy_writer(duration);
}

/// Reader-lane job count for the running synthetic app owner.
#[cfg(feature = "test-support")]
pub fn store_reads() -> u64 {
    host::host_handle()
        .expect("store_reads needs a started app")
        .store_reads()
}

/// Pauses queued writes; the gate acknowledges entry and releases on drop.
#[cfg(feature = "test-support")]
pub fn hold_writer() -> iptv_runtime::TestGate {
    host::host_handle()
        .and_then(|owner| owner.hold_writer())
        .expect("hold_writer needs a started app")
}

/// Hold the next source preparation for deterministic old-native-event tests.
#[cfg(feature = "test-support")]
pub fn hold_source_preparation() -> iptv_runtime::TestGate {
    host::host_handle()
        .and_then(|owner| owner.hold_source_preparation())
        .expect("source preparation gate needs a started app")
}

/// Pretends an external player is (`Some(true)`) or is not (`Some(false)`)
/// installed, or probes the host again (`None`), for the headless tests.
#[cfg(feature = "test-support")]
pub fn set_external_player_handler(handler: Option<bool>) {
    host::set_external_player_handler(handler);
}

/// Read-only synthetic-source observation keyed by the actually published URL.
#[cfg(feature = "test-support")]
pub fn original_player_source(url: &str) -> Option<iptv_core::playback::PlaySource> {
    host::host_handle().and_then(|owner| owner.original_player_source(url))
}

/// The guide header a desktop window `width` × `height` gets at the default
/// text size (for tests: the same policy the header and the video follow).
#[cfg(feature = "test-support")]
pub fn desktop_guide_header(width: f32, height: f32) -> crate::kit::layout::GuideHeader {
    crate::kit::layout::GuideHeader::of(
        width,
        height,
        kit::scale::scale_for(width, height, true),
        1.0,
    )
}

/// The guide header a phone's window `width` × `height` gets at the text
/// factor `text` (for tests: the same policy the header and the video
/// follow, at the phone's guide scale).
#[cfg(feature = "test-support")]
pub fn phone_guide_header(width: f32, height: f32, text: f32) -> crate::kit::layout::GuideHeader {
    crate::kit::layout::GuideHeader::of(width, height, kit::scale::PHONE, text)
}
