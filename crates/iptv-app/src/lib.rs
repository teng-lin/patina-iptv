//! iptv-rs NSH shell (TV + tablet): remote (D-pad) and touch drive one
//! app-owned focus model (`guide.rs`, `browse.rs`); the layout follows the
//! reference UX's regions (header with the preview and the focused
//! program, navigation rail and groups beside the guide grid) over one
//! persistent video layer (`video.rs`). The guide shows one list at a time (`lists.rs`).

use crate::i18n::tr;

static TUI_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Enables TUI-host layout adjustments (such as bypassing desktop horizontal
/// `StickyCells` wrappers that `nsh-platform-tui` does not implement).
pub fn set_tui_mode(enabled: bool) {
    TUI_MODE.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn is_tui_mode() -> bool {
    TUI_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

pub use iptv_core::guide;
#[cfg(all(
    feature = "test-support",
    any(target_os = "linux", target_os = "macos")
))]
mod guide_window_walkthrough;
pub mod theme;

mod backup;
mod browse_playback;
mod capabilities;
mod channels_view;
mod chooser;
mod content_image;
mod custom_groups;
mod data;
mod desktop;
mod epg_sources_view;
mod fav_editor;
mod favorites;
mod fonts;
mod grid;
mod guide_assign_view;
mod guide_inspect_view;
mod guide_repair_view;
mod header;
mod host;
pub mod layout;
pub mod timeline;
// A browser host exports nothing: the glob is empty there.
#[allow(unused_imports)]
#[doc(inline)]
pub use host::exports::*;
mod i18n;
#[cfg(feature = "test-support")]
pub use i18n::pseudo_fixed_for_test;
mod input;
/// Whether a keyboard binding fires on auto-repeats now (headless tests).
#[cfg(feature = "test-support")]
pub use input::keyboard_repeats;
/// Take keys as an Android remote's raw Down/Repeat/Up phases (headless tests).
#[cfg(feature = "test-support")]
pub use input::use_remote_key_phases;
mod interruption;
mod keyboard;
mod list_rows;
mod listen;
mod lists;
mod media;
mod menu_view;
pub(crate) mod parental_view;
#[cfg(feature = "test-support")]
mod performance_probe;
mod recordings_view;
mod vod_view;
use iptv_runtime as runtime;
mod multiview;
mod multiview_picker;
mod multiview_view;
mod numbers;
mod on_demand;
mod organization_view;
mod overlays;
mod panels;
mod pip;
mod play_bar;
mod player;
mod player_keys;
mod player_panels;
mod playlists_view;
mod rail;
mod remote_editor;
mod runtime_events;
mod scale;
mod search_view;
mod seek;
mod settings;
mod settings_view;
mod setup_view;
pub mod startup;
pub mod time;
mod ui_kit;
mod video;
mod wizard_view;

pub use header::debug_overlay;
pub use time::clock;
#[doc(hidden)]
pub use video::has_source as player_has_source;
#[doc(hidden)]
pub use video::opens as player_opens;

/// iOS (an iPad): a tap beside the open columns or list closes them.
pub(crate) const IOS: bool = crate::host::IPAD;

use nsh_core::time::Instant;
use std::time::Duration;

use guide::{Focus, Pane, Program, ProgramSelection, Schedule};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use iptv_core::editor::FavoritesEditor;
use iptv_core::lists::ChannelList;
use iptv_core::menu::{Menu, Origin};
use iptv_core::panels::Panels;
use iptv_core::player_options::{PictureSettings, SleepTimer};
use nsh_core::prelude::*;
use theme::Theme;

#[native_module("iptv-rs/native")]
fn native_module(registry: &mut Registry) {
    registry.register::<App>();
}

/// The last kind of input (`iptv_core::layout`): the focus fill and key
/// hints only show for keys, as in Android's touch mode.
pub use iptv_core::layout::InputMode as Input;

/// A pointer went down: a finger, or a mouse, trackpad or pen. Recorded at
/// the root as the touch starts, before the pressed control hears the press.
pub(crate) fn pointer_down(ui: Ui, e: &nsh_core::TouchEvent) {
    if e.phase != nsh_core::TouchPhase::Start {
        return;
    }
    if let Some(p) = e.points.iter().find(|p| p.changed) {
        let finger = p.pointer_type == PointerType::Touch;
        ui.last_press.set(Input::of_press(finger));
    }
}

/// How the UI sits over the video layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    /// Opaque guide; video in the preview rectangle.
    Guide,
    /// Full-screen video under a translucent guide.
    Overlay,
    /// Full-screen video, no UI.
    Fullscreen,
}

/// What the player is showing for the playing channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Playback {
    Live,
    /// An archived program (its start in guide minutes), from `offset`
    /// seconds into it.
    CatchUp {
        start: i32,
        offset: i32,
    },
}

/// Keys handled and handler time, for the status line and logcat.
#[derive(Clone, Copy, PartialEq, Default)]
pub struct Stats {
    pub keys: u64,
    pub last_us: u128,
    pub max_us: u128,
}

/// Application-wide signals deliberately shared by every presentation window.
#[derive(Clone, Copy)]
pub struct SharedUi {
    /// Ordered favorites/recent lists changed, including while a popup owns its list.
    pub list_version: Signal<u64>,
    pub theme: Signal<Theme>,
    /// The app's monotonic clock origin (core state machines take ms), on
    /// the engine's clock: the one its timers run on.
    pub epoch: Instant,
    /// The favorite channels on screen (catalog channels); kept by key
    /// and changed through `favorites.rs`.
    pub favorites: Signal<BTreeSet<usize>>,
    /// The source has movies or shows (the rail lists them).
    pub has_vod: Signal<bool>,
    /// Some enabled playlist has movies or shows: the rail's cross-source
    /// Continue watching and Recently added entries.
    pub any_vod: Signal<bool>,
    /// Bumped when the guide data changes (views re-read `schedule()`).
    pub data_version: Signal<u64>,
    /// Bumped when an older day of the guide was paged in (`data/pages.rs`):
    /// views showing programs re-read them ([`Ui::guide_data`]).
    pub guide_pages: Signal<u64>,
    pub picture: Signal<PictureSettings>,
    /// The stored playlists: which one is shown, their refreshes (`data.rs`).
    pub library: data::Library,
    /// Bumped when a setting changes; read values with `settings::read`.
    pub settings: Signal<u64>,
}

/// One window's presentation. Copying handles is only for regions of that window;
/// a new window must allocate fresh state through `Ui::new`.
#[derive(Clone, Copy)]
pub struct Ui {
    pub app: SharedUi,
    /// The remote's focus; its `channel` is a row of `list`.
    pub focus: Signal<Focus>,
    pub browse_playback: Signal<iptv_core::browse_playback::BrowsePlayback>,
    /// Guide autoplay held after the main window closed with recordings
    /// running: the channel key focused then (`browse_playback::hold`).
    pub autoplay_held: Signal<Option<Option<String>>>,
    pub pending_zap: Signal<Option<iptv_core::browse_playback::Target>>,
    /// A timeline pan's hold on the channel whose program on air it
    /// started from: browse playback keeps that channel while the pan
    /// moves the focus along the row (`grid/pan.rs`, `browse_playback.rs`).
    pub pan_anchor: Signal<Option<iptv_core::browse_playback::PanAnchor>>,
    /// The host's audio interruptions' policy (`interruption.rs`).
    pub interruptions: Signal<iptv_core::interruption::Interruptions>,
    /// The list the guide shows: catalog channels (`lists.rs`).
    pub list: Signal<Rc<ChannelList>>,
    /// The playing channel (catalog).
    pub playing: Signal<Option<usize>>,
    pub input: Signal<Input>,
    /// What the last pointer press was made with ([`Ui::touched`] reads it).
    pub last_press: Signal<Input>,
    pub mode: Signal<ViewMode>,
    pub stats: Signal<Stats>,
    #[cfg(feature = "test-support")]
    pub(crate) performance: Signal<Option<performance_probe::Probe>>,
    pub player_status: Signal<String>,
    /// The guide rows on screen, as the list reports them.
    pub visible_rows: Signal<Option<(usize, usize)>>,
    /// Player panels over full-screen video.
    pub panels: Signal<Panels>,
    /// The pointer's play bar over full-screen video (`play_bar.rs`).
    pub play_bar: Signal<iptv_core::play_bar::PlayBar>,
    /// The play bar's laid-out height above the window's bottom (0 until
    /// it first shows): its rows wrap in a narrow window.
    pub play_bar_height: Signal<f32>,
    pub paused: Signal<bool>,
    /// A catch-up seek being chosen: the target, in seconds into the
    /// program, opened once the keys pause (`seek.rs`).
    pub seek_target: Signal<Option<i32>>,
    /// The open context menu, if any.
    pub menu: Signal<Option<Menu>>,
    /// The program description popover, if open.
    pub description: Signal<Option<Origin>>,
    pub recordings: Signal<iptv_core::recording::Screen>,
    pub vod: Signal<iptv_core::vod::Screen>,
    pub recordings_open: Memo<bool>,
    /// Live or catch-up for the playing channel.
    pub playback: Signal<Playback>,
    pub channel_number: Signal<iptv_core::channel_number::ChannelNumber<iptv_search::ChannelId>>,
    pub sleep: Signal<SleepTimer>,
    /// The open chooser over the video (picture size, sleep timer).
    pub chooser: Signal<Option<chooser::Open>>,
    /// The favorites editor, when open.
    pub editor: Signal<Option<FavoritesEditor>>,
    pub organization: Signal<Option<organization_view::Open>>,
    organization_generation: Signal<u64>,
    pub custom_groups: Signal<Option<custom_groups::Open>>,
    pub remote_editor: Signal<Option<iptv_nav::remap::Editor>>,
    /// The search screen, while open (also while a result it chose plays).
    pub search: Signal<Option<search_view::Screen>>,
    pub search_history_revision: Signal<u64>,
    /// The add-playlist wizard, when open.
    pub wizard: Signal<Option<wizard_view::Open>>,
    restore_review: Signal<Option<backup::RestoreReview>>,
    store_startup_failure: Signal<Option<startup::StoreFailure>>,
    store_failure_scroll: ui_kit::FocusScroll,
    /// The playlists screen, when open.
    pub playlists: Signal<Option<playlists_view::Open>>,
    /// A playlist's guide sources screen, when open (over the playlists
    /// and settings screens).
    pub guide_sources: Signal<Option<epg_sources_view::Open>>,
    pub guide_inspect: Signal<Option<iptv_library::guide_inspect::Screen>>,
    pub guide_repair: Signal<Option<iptv_library::guide_repair_screen::Screen>>,
    pub guide_repair_cancel: Stored<Option<iptv_import::CancelToken>>,
    pub guide_inspect_cancel: Stored<Option<iptv_import::CancelToken>>,
    /// The guide data chooser of a channel, when open.
    pub guide_assign: Signal<Option<guide_assign_view::Open>>,
    /// The settings screen, when open.
    pub parental: Signal<Option<parental_view::Open>>,
    pub settings_screen: Signal<Option<iptv_settings::screen::Screen>>,
    /// Each player surface's stream, health, position, size and tracks;
    /// the channel history (`player.rs`).
    pub player: player::Player,
    pub player_surface: usize,
    /// Volume, mute and audio delay (`media.rs`).
    pub media: media::Media,
    /// Several channels at once, while open (`multiview.rs`).
    pub multiview: Signal<Option<multiview::State>>,
    pub player_windows: Signal<iptv_core::playback_windows::Workspace<String>>,
    /// The channel list over full-screen video, while open.
    pub channels: Signal<Option<iptv_core::channel_list::ChannelList>>,
    /// Group and search selection while adding or replacing a multiview tile.
    pub channel_picker: Signal<Option<multiview_picker::Screen>>,
    /// The picture-in-picture window, as the host reports it (`pip.rs`).
    pub pip: Signal<iptv_core::pip::Pip>,
    /// A Long Back acted: the platform's Back on its release is swallowed
    /// (`input.rs`).
    pub back_guard: Signal<iptv_nav::BackGuard>,
}

impl SharedUi {
    fn new() -> Self {
        Self {
            list_version: signal(0),
            theme: signal(Theme::default()),
            epoch: nsh_core::time::now(),
            favorites: signal(BTreeSet::new()),
            has_vod: signal(false),
            any_vod: signal(false),
            data_version: signal(0),
            guide_pages: signal(0),
            picture: signal(PictureSettings::default()),
            library: data::Library::new(),
            settings: signal(0),
        }
    }
}

impl Ui {
    /// Allocate all presentation state afresh; shared signals and native stream
    /// ownership are supplied explicitly, never inherited from another window.
    pub(crate) fn new(app: SharedUi, player: player::Player) -> Self {
        let recordings = signal(iptv_core::recording::Screen::default());
        let recordings_open = computed(move || recordings.with(|s| s.open));
        let ui = Self {
            app,
            focus: signal(Focus::default()),
            browse_playback: signal(Default::default()),
            autoplay_held: signal(None),
            pending_zap: signal(None),
            pan_anchor: signal(None),
            interruptions: signal(Default::default()),
            list: signal(Rc::new(ChannelList::build(
                iptv_core::lists::ALL,
                &*schedule(),
                &[],
                &[],
            ))),
            playing: signal(None),
            // Until a press reports its pointer: a finger on phones and
            // tablets, the mouse on desktops.
            last_press: signal(if crate::host::ANDROID || crate::host::IPAD {
                Input::Touch
            } else {
                Input::Pointer
            }),
            // A phone or tablet is touched first: no remote hints, and its
            // visible controls show, until a key is used. TVs, desktops and
            // terminals start with their keys.
            input: signal(
                if matches!(
                    idiom(),
                    nsh_core::platform::Idiom::Phone | nsh_core::platform::Idiom::Pad
                ) {
                    Input::Touch
                } else {
                    Input::Keys
                },
            ),
            mode: signal(ViewMode::Guide),
            stats: signal(Stats::default()),
            #[cfg(feature = "test-support")]
            performance: signal(None),
            player_status: signal("idle".to_owned()),
            visible_rows: signal(None),
            panels: signal(Panels::default()),
            play_bar: signal(Default::default()),
            play_bar_height: signal(0.0),
            paused: signal(false),
            seek_target: signal(None),
            menu: signal(None),
            description: signal(None),
            recordings,
            vod: signal(iptv_core::vod::Screen::default()),
            recordings_open,
            playback: signal(Playback::Live),
            channel_number: signal(Default::default()),
            sleep: signal(SleepTimer::default()),
            chooser: signal(None),
            editor: signal(None),
            organization: signal(None),
            organization_generation: signal(0),
            custom_groups: signal(None),
            remote_editor: signal(None),
            search: signal(None),
            search_history_revision: signal(0),
            wizard: signal(None),
            restore_review: nsh_core::element::signal_with(
                None,
                nsh_core::element::SignalEquals::Never,
            ),
            store_startup_failure: signal(None),
            store_failure_scroll: ui_kit::FocusScroll::new(),
            playlists: signal(None),
            guide_sources: signal(None),
            guide_inspect: signal(None),
            guide_repair: signal(None),
            guide_repair_cancel: stored(None),
            guide_inspect_cancel: stored(None),
            guide_assign: signal(None),
            parental: signal(None),
            settings_screen: signal(None),
            player,
            player_surface: 0,
            media: media::Media::new(),
            multiview: signal(None),
            player_windows: signal(Default::default()),
            channels: signal(None),
            channel_picker: signal(None),
            pip: signal(Default::default()),
            back_guard: signal(iptv_nav::BackGuard::default()),
        };
        player_panels::bind_auto_hide(ui);
        ui
    }
}

impl Ui {
    /// Tracks the programs on screen: a new schedule or a day paged in.
    /// The value changes with either.
    pub fn guide_data(&self) -> u64 {
        self.app.data_version.get() << 32 | (self.app.guide_pages.get() & 0xFFFF_FFFF)
    }

    /// OK or a tap on the focused program: a live one plays (OK on the
    /// channel already playing goes full screen). An available replay plays
    /// from the archive. Other listings open their details.
    pub fn watch(&self) {
        let view = lists::view(*self);
        let (row, program, now) = self.focus.with(|f| (f.channel, f.program(&view), f.now));
        let Some(channel) = view.channel(row) else {
            return;
        };
        let live = (self.playback.get() == Playback::Live)
            .then(|| self.playing.get())
            .flatten();
        let selection =
            ProgramSelection::for_program(&program, now, view.catchup_days(row), channel, live);
        match selection {
            ProgramSelection::AlreadyLive => {
                self.mode.set(ViewMode::Fullscreen);
                self.show_playing();
            }
            ProgramSelection::Live => browse_playback::select(*self),
            ProgramSelection::Replay => {
                if !player::allow_guide_channel(channel) {
                    return;
                }
                self.play(
                    channel,
                    Playback::CatchUp {
                        start: program.start,
                        offset: 0,
                    },
                );
                self.mode.set(ViewMode::Fullscreen);
                self.show_playing();
            }
            ProgramSelection::Upcoming | ProgramSelection::Unavailable => {
                let origin = Origin {
                    channel,
                    program_start: program.start,
                };
                if selection == ProgramSelection::Upcoming {
                    recordings_view::remember(*self, origin);
                }
                self.description.set(Some(origin));
            }
        }
    }

    /// Starts `channel` live or from the archive.
    pub fn play(&self, channel: usize, playback: Playback) {
        if !player::allow_guide_channel(channel) {
            return;
        }
        if self.player_surface == 0
            && self
                .playing
                .get_untracked()
                .is_some_and(|old| old != channel)
            && let Some(old) = self
                .playing
                .get_untracked()
                .and_then(|old| parental_view::live_target(*self, old))
        {
            crate::runtime().parental_content_left(&old);
        }
        if !parental_view::allow_play(*self, channel, playback) {
            return;
        }
        if self.player_surface != 0 {
            self.pending_zap.set(None);
            if player::start_surface(*self, self.player_surface, channel) {
                // Accepted: the viewer's playback now (`interruption.rs`).
                crate::interruption::viewer_acted(*self);
                self.playback.set(Playback::Live);
                self.paused.set(false);
            }
            return;
        }
        if !recordings_view::reserve_viewing(*self, 0, channel, matches!(playback, Playback::Live))
        {
            return;
        }
        // Accepted: the viewer's playback now (`interruption.rs`).
        crate::interruption::viewer_acted(*self);
        crate::on_demand::flush(*self, false);
        self.player.vod.set(None);
        self.player.recorded.set(None);
        crate::runtime().close_recording_playback();
        self.pending_zap.set(None);
        self.browse_playback.set(Default::default());
        self.playback.set(playback);
        self.playing.set(Some(channel));
        self.paused.set(false);
        player::start(self, channel);
    }

    /// A touch or a click on any control: touch or pointer mode, whichever
    /// made the press.
    pub fn touched(&self) {
        self.pending_zap.set(None);
        browse_playback::activity(*self);
        self.input.set(self.last_press.get_untracked());
    }

    pub fn keys_shown(&self) -> bool {
        self.input.get().key_hints()
    }

    /// Whether lists are edited by dragging with visible Save and Cancel
    /// (tablets and desktops) rather than by OK, the arrows and Back asking
    /// to save (TVs and terminals). A property of the host, not of the last
    /// input: an editing session keeps one model however it is driven.
    pub fn drag_editing(&self) -> bool {
        !matches!(
            idiom(),
            nsh_core::platform::Idiom::Tv | nsh_core::platform::Idiom::Terminal
        )
    }

    /// Visible ways back (`iptv_core::layout::InputMode::back_controls`):
    /// always on tablets and desktops, including after a keyboard is used;
    /// on TVs and terminals only once touch or a pointer is used.
    pub fn back_controls(&self) -> bool {
        let remote_back = matches!(
            idiom(),
            nsh_core::platform::Idiom::Tv | nsh_core::platform::Idiom::Terminal
        );
        self.input.get().back_controls(remote_back)
    }

    /// A foreground UI surface color. In guide mode the opaque colors are
    /// drawn by the backdrop *under* the video layer (`panels::backdrop`), so
    /// the foreground is clear and the preview rectangle shows the video; in
    /// overlay mode it tints the full-screen video.
    pub fn surface(&self, rgb_hex: u32) -> Rgba {
        let alpha = match self.mode.get() {
            // `appearance.ui_transparency`
            ViewMode::Overlay => settings::overlay_alpha(&settings::read(*self)),
            ViewMode::Guide | ViewMode::Fullscreen => 0x00,
        };
        tint(rgb_hex, alpha)
    }

    /// Milliseconds since the app started, on the engine's clock (a
    /// test's manual clock moves it with the timers).
    pub fn now_ms(&self) -> u64 {
        nsh_core::time::now()
            .saturating_duration_since(self.app.epoch)
            .as_millis() as u64
    }

    /// What just started playing over full-screen video: the banner for
    /// a live channel, the card with its seek bar for a replay.
    pub fn show_playing(&self) {
        let live =
            self.playback.get_untracked() == Playback::Live && !crate::on_demand::active(*self);
        self.update_panels(|p, now| {
            if live {
                p.show_banner(now);
            } else {
                p.show_info(now);
            }
        });
    }

    /// Changes the panels at the current point in the app's clock.
    pub fn update_panels(&self, f: impl FnOnce(&mut Panels, u64)) {
        let now = self.now_ms();
        self.panels.update(|p| f(p, now));
    }

    pub(crate) fn toggle(&self, mode: ViewMode) {
        self.mode
            .update(|m| *m = if *m == mode { ViewMode::Guide } else { mode });
    }
}

thread_local! {
    /// The application owner this thread's shell code is working for: the
    /// root's, or, while a task accepted by an earlier root is polled, that
    /// task's ([`in_runtime`]). The runtime crate keeps no such handle of its
    /// own; every call into it is given this one.
    static RUNTIME: RefCell<Option<Rc<runtime::Runtime>>> = const { RefCell::new(None) };
}

/// The shell owns one application handle; all library state belongs to it.
///
/// Application-thread code only. A player, JNI or worker thread holds no
/// handle and must be given what it needs; once a root exists, asking for
/// the handle elsewhere is a bug, not a reason to build a second owner.
pub(crate) fn runtime() -> Rc<runtime::Runtime> {
    RUNTIME.with(|runtime| {
        runtime
            .borrow_mut()
            .get_or_insert_with(|| {
                // Pre-root shell tests may construct an owner. Once a root
                // exists, callbacks must dispatch to its shell thread; a second
                // dormant owner would silently fork state in release builds.
                assert!(
                    !ROOT_STARTED.load(std::sync::atomic::Ordering::Acquire),
                    "the application handle was requested off the application thread"
                );
                let directory = host::resolve_data_dir()
                    .expect("private storage must be resolved before using runtime services");
                Rc::new(new_runtime(directory))
            })
            .clone()
    })
}

/// An application root installed its owner in this process. Only a guard
/// against asking for the handle on another thread; it selects no owner.
static ROOT_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether an application owner exists (none before storage is resolved).
pub(crate) fn has_runtime() -> bool {
    RUNTIME.with(|runtime| runtime.borrow().is_some())
}

/// Runs `run` with `owner` as the shell's handle, then puts the previous
/// one back (also when `run` unwinds).
pub(crate) fn with_runtime<R>(owner: Rc<runtime::Runtime>, run: impl FnOnce() -> R) -> R {
    struct Restore(Option<Rc<runtime::Runtime>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            RUNTIME.set(self.0.take());
        }
    }
    let _restore = Restore(RUNTIME.replace(Some(owner)));
    run()
}

pub(crate) fn new_runtime(directory: std::path::PathBuf) -> runtime::Runtime {
    runtime::Runtime::new(host::ports(directory))
}

/// A local task always polls with the owner that accepted it, also after
/// the root (and with it the shell's handle) was recreated.
pub(crate) fn in_runtime(
    future: impl std::future::Future<Output = ()> + 'static,
) -> impl std::future::Future<Output = ()> {
    let owner = runtime();
    let mut future = Box::pin(future);
    std::future::poll_fn(move |context| {
        with_runtime(owner.clone(), || future.as_mut().poll(context))
    })
}

/// The guide data in use (synthetic until a playlist is imported).
pub fn schedule() -> Rc<dyn Schedule> {
    runtime().schedule()
}

/// Swaps the guide data (after an import) and tells the views to re-read.
/// The guide keeps its list and focused channel by identity, at the new
/// schedule's now; channel indices into the old data (open menus) that no
/// longer exist are dropped, the playing channel and favorites are found
/// again by key.
pub fn set_schedule(ui: &Ui, schedule: Rc<dyn Schedule>) {
    let kept = lists::keep(*ui);
    runtime().set_schedule(schedule.clone());
    time::mark_shown();
    let n = schedule.channel_count();
    if ui.playing.get_untracked().is_some_and(|ch| ch >= n) {
        player::row_dropped(*ui);
    }
    favorites::catalog_changed(*ui, &*schedule);
    lists::restore(*ui, kept);
    let n = ui.list.with_untracked(|l| l.len());
    let rows = ui.visible_rows.get_untracked();
    let clamped = rows.and_then(|(a, b)| (n > 0).then(|| (a.min(n - 1), b.min(n - 1))));
    if clamped != rows {
        ui.visible_rows.set(clamped);
    }
    ui.menu.set(None);
    ui.description.set(None);
    ui.editor.set(None);
    ui.app.data_version.update(|v| *v += 1);
}

/// Whether `program` on `channel` is inside the channel's catch-up window
/// at guide minute `now`.
pub fn in_archive(channel: usize, program: Program, now: i32) -> bool {
    program.end <= now && program.can_replay(now, schedule().catchup_days(channel))
}

/// `0xRRGGBB` with alpha, as an NSH color.
pub fn tint(rgb_hex: u32, alpha: u8) -> Rgba {
    rgba(theme::with_alpha(rgb_hex, alpha))
}

pub fn channel_name(ch: usize) -> String {
    schedule().channel_name(ch)
}

#[component]
pub fn App() -> impl IntoElement {
    startup::mark("rust root");
    host::log_library_base();
    fonts::register();
    let directory = match host::initialize() {
        Ok(directory) => directory,
        Err(error) => {
            log::error!("iptv storage initialization failed: {error}");
            return view()
                .flex_1()
                .items_center()
                .justify_center()
                .child(text(i18n::tr("Private app storage is unavailable")))
                .child(text(i18n::tr(
                    "Check your home directory and file permissions, then reopen Patina.",
                )));
        }
    };
    // This root's owner becomes the shell's handle, and the one the host's
    // stop callback flushes. Only a root does this: constructing a runtime
    // elsewhere changes neither.
    let owner = Rc::new(new_runtime(directory));
    host::flush_on_host_stop(owner.host_handle());
    RUNTIME.set(Some(owner));
    ROOT_STARTED.store(true, std::sync::atomic::Ordering::Release);
    if let Err(error) = runtime().lock_storage() {
        let message = error.message();
        return view()
            .flex_1()
            .items_center()
            .justify_center()
            .child(text(i18n::tr(message)));
    }
    // Restore validation can decrypt/migrate SQLite and copied documents. It
    // must finish off the UI thread before settings or the live store open.
    let outcome = signal(None::<Result<(), &'static str>>);
    let (tx, rx) = futures_channel::oneshot::channel();
    let started = host::background("iptv-bootstrap", {
        let activate = runtime().restore_activation();
        move || {
            let result = match activate() {
                Ok(()) => Ok(()),
                Err(error) => {
                    log::warn!("iptv backup: {}", error.error);
                    if error.fatal {
                        Err(error.error.message())
                    } else {
                        Ok(())
                    }
                }
            };
            let _ = tx.send(result);
        }
    });
    if started.is_err() {
        outcome.set(Some(Err(i18n::key(
            "The startup task could not be started",
        ))));
    } else {
        // Detach under this root owner; disposing the root cancels the reply
        // task, while recovery itself safely completes on its worker.
        drop(spawn(async move {
            let stopped = Err(i18n::key("The startup task stopped"));
            outcome.set(Some(rx.await.unwrap_or(stopped)));
        }));
    }
    // Keep startup branding above the app while its library and player start.
    // Removing this sibling overlay never remounts or hides a player surface.
    let minimum_elapsed = signal(false);
    let _splash_timer = set_timeout(Duration::from_millis(1500), move || {
        minimum_elapsed.set(true);
    });
    let splash = move || match outcome.get() {
        None => true,
        Some(Ok(())) => !minimum_elapsed.get(),
        Some(Err(_)) => false,
    };
    let input = stored(None);
    input::window(view().flex_1(), input)
        .child(
            view()
                .flex_1()
                .accessibility_elements_hidden(splash)
                .child(dynamic(move || {
                    let result = outcome.get();
                    // Only the one-shot outcome controls this branch. Reads while building
                    // ReadyApp must not subscribe the branch and recreate its video surface.
                    untracked(move || match result {
                        None => view()
                            .flex_1()
                            .items_center()
                            .justify_center()
                            .into_element(),
                        Some(Err(message)) => view()
                            .flex_1()
                            .items_center()
                            .justify_center()
                            .child(text(i18n::tr("Saved data could not be recovered")))
                            .child(text(i18n::tr(message)))
                            .child(text(i18n::tr("Close Patina and try opening it again.")))
                            .into_element(),
                        Some(Ok(())) => ready_app(input).into_element(),
                    })
                })),
        )
        .child(when(splash, startup::splash))
}

/// Constructed once after recovery; all player surfaces belong to this owner.
fn ready_app(input: Stored<Option<input::InputScope>>) -> impl IntoElement {
    settings::load();
    startup::mark("settings loaded");
    let ui = Ui::new(SharedUi::new(), player::Player::new());
    settings::bind(ui);
    browse_playback::bind(ui);
    interruption::bind(ui);
    desktop::bind(ui);
    desktop::menus(ui);
    // Leaving full screen closes the panels.
    effect(move || {
        if ui.mode.get() != ViewMode::Fullscreen {
            log::info!("iptv panels hidden: left full screen");
            ui.panels.update(|p| p.hide());
        }
    });
    favorites::reset();
    favorites::catalog_changed(ui, &*schedule());
    input.replace(Some(input::bind(ui)));
    #[cfg(feature = "test-support")]
    performance_probe::bind(ui);
    data::start(ui);
    time::start(ui);
    player::bind(ui);
    player::watch_time::bind(ui);
    data::pages::bind(ui);
    play_bar::bind(ui);
    lists::bind(ui);
    multiview::bind(ui);
    pip::bind(ui);
    desktop::player_windows::bind(ui);
    bind_back(ui);
    #[cfg(all(
        feature = "test-support",
        any(target_os = "linux", target_os = "macos")
    ))]
    guide_window_walkthrough::bind(ui);

    let rows = element_ref();
    let groups = element_ref();
    let channel = computed(move || ui.focus.with(|f| f.channel));
    let group = computed(move || ui.focus.with(|f| f.group));
    let shown = computed(move || lists::columns_shown(ui));
    // Keys scroll only when the focus comes within EDGE rows of the
    // viewport's edge (touch scrolls on its own).
    const EDGE: usize = 2;
    effect(move || {
        let ch = channel.get();
        if !ui.keys_shown() {
            return;
        }
        let Some((first, last)) = ui.visible_rows.get_untracked() else {
            return;
        };
        if ch < first + EDGE {
            rows.scroll_to_index(
                ch.saturating_sub(EDGE) as u32,
                percentage(0.0),
                px(0.0),
                false,
            );
        } else if ch + EDGE > last {
            let len = ui.list.with_untracked(|l| l.len());
            let target = (ch + EDGE).min(len.saturating_sub(1));
            rows.scroll_to_index(target as u32, percentage(1.0), px(0.0), false);
        }
    });
    // The groups column shows the selected group when it opens or moves.
    effect(move || {
        if shown.get() {
            groups.scroll_to_index(group.get() as u32, percentage(0.5), px(0.0), false);
        }
    });

    view()
        .flex_1()
        .bg(move || {
            if video::backdrop_clear(ui) {
                rgba(0)
            } else {
                rgb(ui.app.theme.with(|t| t.background.window))
            }
        })
        // A moving mouse over full-screen video shows the player panels.
        .on_mouse_move(move |e: &MouseEvent| pointer_moved(ui, e))
        .on_mouse_enter(move |e: &MouseEvent| {
            desktop::pointer_entered(ui.player_surface, e.page_position.x.0, e.page_position.y.0)
        })
        // Which pointer presses the controls: touch or pointer mode.
        .on_touch(move |e: &nsh_core::TouchEvent| pointer_down(ui, e))
        // Layers, bottom to top: opaque backdrop, video (multiview's other
        // surfaces while their tiles play), UI, full-screen tap catcher.
        // The picture-in-picture window shows only the video (`pip.rs`).
        .child(backup::service_layer())
        .child(keyboard::bridge())
        .child(panels::backdrop(ui))
        .child(video::layer(ui, 0))
        .children((1..player::SURFACES).map(move |s| {
            when(
                move || multiview::mounted(ui, s) || desktop::player_windows::mounted(ui, s),
                move || video::layer(ui, s),
            )
        }))
        .child(ui_layers(ui, groups, rows))
        // The desktops' picture-in-picture window, while it is open.
        .child(desktop::pip_window::window(ui))
        .child(desktop::player_windows::windows(ui))
}

/// Everything over the video: the guide, the full-screen tap catcher,
/// the player panels and every screen and popup. None of it shows in the
/// picture-in-picture window.
fn ui_layers(ui: Ui, groups: ElementRef, rows: ElementRef) -> View {
    let fullscreen = move || ui.mode.get() == ViewMode::Fullscreen;
    let shown = move || pip::ui_shown(ui);
    view()
        .test_id("app-ui")
        .absolute()
        .inset_0()
        .accessibility_elements_hidden(move || !shown())
        .opacity(move || if shown() { 1.0_f32 } else { 0.0 })
        .pointer_events(move || {
            if shown() {
                PointerEvents::BoxNone
            } else {
                PointerEvents::None
            }
        })
        .child(
            view()
                .test_id("guide-ui")
                .flex_1()
                .accessibility_elements_hidden(move || {
                    fullscreen()
                        || (ui.any_overlay_open() && !organization_view::guide_order_accessible(ui))
                })
                .opacity(move || if fullscreen() { 0.0_f32 } else { 1.0 })
                .pointer_events(move || {
                    if fullscreen() || organization_view::is_open(ui) {
                        PointerEvents::None
                    } else {
                        PointerEvents::Auto
                    }
                })
                .child(header::header(ui))
                .child(
                    view()
                        .flex_1()
                        .flex_row()
                        .child(rail::column(ui))
                        .child(panels::group_column(ui, groups))
                        .child(grid::guide(ui, rows)),
                ),
        )
        .child(
            // Full screen: a tap toggles the play bar (the panels where
            // there is no pointer).
            view()
                .test_id("video")
                .accessibility_elements_hidden(move || {
                    !fullscreen() || ui.any_overlay_open() || multiview::is_open(ui)
                })
                .absolute()
                .inset_0()
                .pointer_events(move || {
                    if fullscreen() {
                        PointerEvents::Auto
                    } else {
                        PointerEvents::None
                    }
                })
                .label(move || tr("Video"))
                .on_press(move |e: &PressEvent| {
                    ui.touched();
                    toggle_panels(ui);
                    // A double click on the desktop: window full screen (the
                    // first click's panels go again).
                    if e.clicks == 2 && desktop::DESKTOP {
                        desktop::toggle_fullscreen();
                    }
                }),
        )
        // Where the video was while the desktops' picture-in-picture
        // window shows it.
        .child(desktop::pip_window::placeholder(ui))
        .child(player_panels::status(ui))
        .child(player_panels::layer(ui))
        .child(multiview_view::layer(ui))
        .child(channels_view::layer(ui))
        .child(multiview_picker::layer(ui))
        .child(search_view::layer(ui))
        .child(menu_view::layer(ui))
        .child(chooser::layer(ui))
        // Over the popups it opens (choosers, program details), so its
        // buttons stay usable and toggle them.
        .child(play_bar::layer(ui))
        .child(fav_editor::layer(ui))
        .child(settings_view::layer(ui))
        .child(vod_view::layer(ui))
        .child(recordings_view::layer(ui))
        .child(guide_assign_view::layer(ui))
        // The playlists screen opens over settings, a guide sources screen
        // over both.
        .child(playlists_view::layer(ui))
        .child(remote_editor::layer(ui))
        .child(custom_groups::layer(ui))
        .child(organization_view::layer(ui))
        .child(epg_sources_view::layer(ui))
        .child(guide_inspect_view::layer(ui))
        .child(guide_repair_view::layer(ui))
        .child(wizard_view::layer(ui))
        .child(backup::layer(ui))
        .child(parental_view::layer(ui))
        .child(startup::store_failure_layer(ui))
        .child(settings_view::notice_layer())
}

/// A tap on full-screen video: the play bar on a device with a pointer,
/// else the card and its buttons; beside multiview's tiles, its pointer
/// chrome (the tiles take their own taps).
fn toggle_panels(ui: Ui) {
    // A screen of its own covers the video: a tap that fell through its
    // background is not one on the picture.
    if untracked(|| ui.overlay_open(overlays::Overlay::covers_video)) {
        return;
    }
    // An iPad's tap on the video beside the channel list closes it.
    if IOS && untracked(|| channels_view::is_open(ui)) {
        channels_view::close(ui);
        return;
    }
    if untracked(|| multiview::is_open(ui)) {
        // Beside the tiles: the menu closes first.
        if multiview::dismiss(ui) {
            return;
        }
        if play_bar::pointer_available() {
            play_bar::tapped(ui);
        }
        return;
    }
    if play_bar::pointer_available() {
        play_bar::tapped(ui);
        return;
    }
    if ui.panels.with(|p| p.shown) == iptv_core::panels::Shown::Hidden {
        ui.update_panels(|p, now| p.show_card(now));
    } else {
        log::info!("iptv panels hidden by a tap");
        ui.panels.update(|p| p.hide());
    }
}

/// The pointer is what the viewer uses now: Pointer mode, without the
/// side effects of a press ([`Ui::touched`]).
pub(crate) fn pointer_input(ui: Ui) {
    if ui.input.get_untracked() != Input::Pointer {
        ui.last_press.set(Input::Pointer);
        ui.input.set(Input::Pointer);
    }
}

/// The mouse moved: over full-screen video with nothing open above the
/// panels, it shows the play bar (multiview's pointer chrome over its
/// grid) and keeps it while it moves.
fn pointer_moved(ui: Ui, e: &MouseEvent) {
    let open = untracked(|| ui.overlay_open(overlays::Overlay::keeps_pointer_from_play_bar));
    let (x, y) = (e.page_position.x.0, e.page_position.y.0);
    // Anywhere else (the guide, lists, Recordings), a mouse that really
    // moved after keys switches to Pointer, so the ⋯ controls show: no
    // focus moves and no playback is scheduled or canceled.
    if desktop::pointer_traveled(ui.player_surface, x, y) {
        pointer_input(ui);
    }
    if ui.mode.get_untracked() != ViewMode::Fullscreen
        || open
        || !desktop::pointer_moved(x, y, ui.now_ms())
    {
        return;
    }
    if ui.input.get_untracked() != Input::Pointer {
        ui.last_press.set(Input::Pointer);
        ui.touched();
    }
    play_bar::pointer_moved(ui);
}

/// Back: dialogs close; a full screen window becomes a window again (the
/// desktop's Escape); player panels or the play bar → hidden; full screen/overlay → guide;
/// then the guide screen's chain (`browse.rs`): hint card → guide → groups →
/// rail → leave the app (no handler, so the platform's Back leaves; with
/// `general.confirm_exit` the first press only asks for a second). The
/// release of a Long Back that acted takes no step (`input::swallow_back`),
/// so a handler stays while one is pending, even where Back would leave.
fn bind_back(ui: Ui) {
    let back_target = computed(move || {
        let panels =
            ui.panels.with(|p| p.shown) != iptv_core::panels::Shown::Hidden || play_bar::shows(ui);
        let menu = ui.menu.with(Option::is_some)
            || ui.description.with(Option::is_some)
            || ui.chooser.with(Option::is_some)
            || ui.editor.with(Option::is_some)
            || ui.focus.with(|f| f.hint)
            || channels_view::is_open(ui)
            || multiview_picker::is_open(ui);
        (ui.mode.get(), ui.focus.with(|f| f.pane), panels || menu)
    });
    effect(move || {
        let leaves = matches!(back_target.get(), (ViewMode::Guide, Pane::Rail, false))
            && !ui.any_overlay_open()
            && !desktop::fullscreen()
            && !ui.back_guard.with(iptv_nav::BackGuard::armed);
        let confirm = settings::read(ui).bool(
            iptv_settings::keys::CONFIRM_EXIT,
            &iptv_settings::Target::Global,
        );
        if !leaves || confirm {
            on_back_press(move || input::swallow_back(ui) || go_back(ui));
        }
    });
}

/// One step back (the platform's Back, or a key standing in for it);
/// `false` where Back leaves the app.
pub(crate) fn go_back(ui: Ui) -> bool {
    if ui.channel_number.with(|entry| entry.active()) {
        ui.channel_number.update(|entry| {
            entry.take();
        });
        return true;
    }
    ui.pending_zap.set(None);
    browse_playback::activity(ui);
    untracked(|| {
        use overlays::Overlay;
        let top = ui.top_overlay();
        let panels =
            ui.panels.with(|p| p.shown) != iptv_core::panels::Shown::Hidden || play_bar::shows(ui);
        let menu = ui.menu.with(Option::is_some)
            || ui.description.with(Option::is_some)
            || ui.chooser.with(Option::is_some)
            || ui.editor.with(Option::is_some);
        match (ui.mode.get(), ui.focus.with(|f| f.pane), panels || menu) {
            _ if top == Some(Overlay::StoreStartupFailure) => startup::store_failure_back(ui),
            _ if top == Some(Overlay::Parental) => {
                parental_view::close(ui);
                true
            }
            _ if top == Some(Overlay::RestoreReview) => backup::back(ui),
            _ if top == Some(Overlay::Wizard) => wizard_view::back(ui),
            _ if top == Some(Overlay::Vod) => vod_view::back(ui),
            _ if top == Some(Overlay::Recordings) => recordings_view::back(ui),
            _ if top == Some(Overlay::GuideSources) => epg_sources_view::back(ui),
            _ if top == Some(Overlay::GuideRepair) => guide_repair_view::back(ui),
            _ if top == Some(Overlay::GuideInspection) => guide_inspect_view::back(ui),
            _ if top == Some(Overlay::RemoteEditor) => remote_editor::back(ui),
            _ if top == Some(Overlay::Organization) => organization_view::back(ui),
            _ if top == Some(Overlay::CustomGroups) => custom_groups::back(ui),
            _ if top == Some(Overlay::Playlists) => playlists_view::back(ui),
            _ if top == Some(Overlay::Settings) => settings_view::back(ui),
            _ if top == Some(Overlay::GuideAssign) => guide_assign_view::back(ui),
            _ if top == Some(Overlay::FavoritesEditor) => {
                fav_editor::close(ui);
                true
            }
            _ if top == Some(Overlay::Chooser) => {
                chooser::close(ui);
                true
            }
            _ if matches!(top, Some(Overlay::Menu | Overlay::Description)) => {
                menu_view::close(ui);
                true
            }
            _ if top == Some(Overlay::Search) => {
                search_view::apply(ui, iptv_search::SearchAction::Back);
                true
            }
            _ if top == Some(Overlay::ChannelList) => {
                channels_view::key(ui, iptv_nav::Action::Back);
                true
            }
            _ if top == Some(Overlay::ChannelPicker) => {
                multiview_picker::key(ui, iptv_nav::Action::Back);
                true
            }
            // The operating system's full screen is restored first; the
            // next Back leaves the video for where it was chosen.
            _ if ui.player_surface == 0 && desktop::fullscreen() => {
                desktop::set_fullscreen(false);
                true
            }
            (ViewMode::Fullscreen, _, false) if ui.player.recorded.with(Option::is_some) => {
                recordings_view::open(ui);
                true
            }
            // A movie or an episode: back to its details (#147).
            (ViewMode::Fullscreen, _, false) if vod_view::return_from_player(ui) => true,
            // A dialog closes, else the active tile goes full screen.
            _ if multiview::is_open(ui) => multiview::back(ui),
            // A tile watched full screen: back to the grid as it was.
            _ if multiview::detour(ui) => multiview::resume(ui),
            (ViewMode::Fullscreen, _, true) => {
                ui.panels.update(|p| p.hide());
                ui.play_bar.update(iptv_core::play_bar::PlayBar::hide);
                true
            }
            // Hint, past program → now, guide → groups → rail → leave
            // (`general.confirm_exit`: only after a second press).
            (ViewMode::Overlay, _, _) if ui.playing.get_untracked().is_some() => {
                ui.mode.set(ViewMode::Fullscreen);
                true
            }
            (ViewMode::Guide | ViewMode::Overlay, _, _) => {
                lists::back(ui) || !settings_view::may_leave()
            }
            _ => {
                ui.mode.set(crate::settings::browse_mode());
                true
            }
        }
    })
}

/// Whether a stored source has a guide timer or an in-flight guide read,
/// for the headless tests of what keeps its periodic checks alive.
#[cfg(feature = "test-support")]
pub fn guide_check_pending(source: runtime::model::SourceId) -> bool {
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
/// Keeps the store's writer busy, as a long catalog or guide publish does,
/// for the headless tests of what must not wait for it.
#[cfg(feature = "test-support")]
pub fn occupy_writer(duration: Duration) {
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
pub fn hold_writer() -> runtime::TestGate {
    host::host_handle()
        .and_then(|owner| owner.hold_writer())
        .expect("hold_writer needs a started app")
}

/// Hold the next source preparation for deterministic old-native-event tests.
#[cfg(feature = "test-support")]
pub fn hold_source_preparation() -> runtime::TestGate {
    host::host_handle()
        .and_then(|owner| owner.hold_source_preparation())
        .expect("source preparation gate needs a started app")
}

/// Read-only synthetic-source observation keyed by the actually published URL.
#[cfg(feature = "test-support")]
pub fn original_player_source(url: &str) -> Option<iptv_core::playback::PlaySource> {
    host::host_handle().and_then(|owner| owner.original_player_source(url))
}

/// The guide header a desktop window `width` × `height` gets at the default
/// text size (for tests: the same policy the header and the video follow).
#[cfg(feature = "test-support")]
pub fn desktop_guide_header(width: f32, height: f32) -> crate::layout::GuideHeader {
    crate::layout::GuideHeader::of(width, height, scale::scale_for(width, height, true), 1.0)
}

/// The guide header a phone's window `width` × `height` gets at the text
/// factor `text` (for tests: the same policy the header and the video
/// follow, at the phone's guide scale).
#[cfg(feature = "test-support")]
pub fn phone_guide_header(width: f32, height: f32, text: f32) -> crate::layout::GuideHeader {
    crate::layout::GuideHeader::of(width, height, scale::PHONE, text)
}
