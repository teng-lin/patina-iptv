//! Window presentation handles (`Ui`, `SharedUi`, and per-domain sub-handles)
//! and small view helpers shared across the app.

use std::collections::BTreeSet;
use std::rc::Rc;

use iptv_presentation::editor::FavoritesEditor;
use iptv_presentation::lists::ChannelList;
use iptv_presentation::menu::{Menu, Origin};
use iptv_presentation::panels::Panels;
use iptv_presentation::player_options::{PictureSettings, SleepTimer};
use nsh_core::prelude::*;
use nsh_core::time::Instant;

use crate::guide::{Focus, Program};
use crate::kit::{mode, overlay::Overlay, ui_kit};
use crate::library_ui::{
    backup, custom_groups, epg_sources_view, guide_assign_view, organization_view, playlists_view,
    wizard_view,
};
use crate::playback::{
    browse_playback, external_player, media, multiview, multiview_picker, player, player_panels,
    touch_zap,
};
use crate::screens::{casting, parental_view, search_view, settings_view};
use crate::shell::{chooser, desktop, startup_view};
use crate::theme::{self, Theme};
#[cfg(feature = "test-support")]
use crate::wiring::performance_probe;
use crate::wiring::runtime_owner::schedule;
use crate::wiring::{data, request_ui, settings_wiring};

/// The last kind of input (`iptv_presentation::input_mode`): the focus fill and key
/// hints only show for keys, as in Android's touch mode.
pub use iptv_presentation::input_mode::InputMode as Input;

/// A pointer went down: a finger, or a mouse, trackpad or pen. Recorded at
/// the root as the touch starts, before the pressed control hears the press.
pub(crate) fn pointer_down(chrome: ChromeUi, e: &nsh_core::TouchEvent) {
    if e.phase != nsh_core::TouchPhase::Start {
        return;
    }
    if let Some(p) = e.points.iter().find(|p| p.changed) {
        let finger = p.pointer_type == PointerType::Touch;
        chrome.last_press.set(Input::of_press(finger));
    }
}

/// The pointer is what the viewer uses now: Pointer mode, without the
/// side effects of a press ([`Ui::touched`]).
pub(crate) fn pointer_input(chrome: ChromeUi) {
    if chrome.input.get_untracked() != Input::Pointer {
        chrome.last_press.set(Input::Pointer);
        chrome.input.set(Input::Pointer);
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
    /// Bumped when a setting changes; read values with `settings_wiring::read`.
    pub settings: Signal<u64>,
    /// The streams the shown source's account allows at once, when it
    /// says (the runtime's, `multiview::account_limit`, as a signal for
    /// the gates that follow it).
    pub account_streams: Signal<Option<u32>>,
}

/// One window's presentation. Copying handles is only for regions of that window;
/// a new window must allocate fresh state through `Ui::new`.
#[derive(Clone, Copy)]
pub struct Ui {
    pub app: SharedUi,
    /// Owns work that must survive changes to controls inside this window.
    pub(crate) owner: Owner,
    pub(crate) cast: casting::CastUi,
    pub(crate) guide: GuideUi,
    pub(crate) playback: PlaybackUi,
    pub(crate) screens: ScreensUi,
    pub(crate) chrome: ChromeUi,
}

#[derive(Clone, Copy)]
pub(crate) struct GuideUi {
    /// The remote's focus; its `channel` is a row of `list`.
    pub focus: Signal<Focus>,
    pub browse_playback: Signal<iptv_presentation::browse_playback::BrowsePlayback>,
    /// Guide autoplay held after the main window closed with recordings
    /// running: the channel key focused then (`browse_playback::hold`).
    pub autoplay_held: Signal<Option<Option<String>>>,
    /// A timeline pan's hold on the channel whose program on air it
    /// started from: browse playback keeps that channel while the pan
    /// moves the focus along the row (`grid/pan.rs`, `browse_playback.rs`).
    pub pan_anchor: Signal<Option<iptv_presentation::browse_playback::PanAnchor>>,
    /// The list the guide shows: catalog channels (`lists.rs`).
    pub list: Signal<Rc<ChannelList>>,
    /// The guide rows on screen, as the list reports them.
    pub visible_rows: Signal<Option<(usize, usize)>>,
}

#[derive(Clone, Copy)]
pub(crate) struct PlaybackUi {
    pub pending_zap: Signal<Option<iptv_presentation::browse_playback::Target>>,
    /// The pending zap is a touch's (a swipe, the play bar's channel
    /// buttons, a screen reader's action): its banner shows when it tunes,
    /// whatever `player.info_on_switch` says (`touch_zap.rs`).
    pub(crate) touch_zapped: Signal<bool>,
    /// A swipe over full-screen video under way: the banner of the channel
    /// it would change to, following the finger (`touch_zap.rs`).
    pub(crate) swipe: Signal<Option<touch_zap::Preview>>,
    /// The host's audio interruptions' policy (`interruption.rs`).
    pub interruptions: Signal<iptv_presentation::interruption::Interruptions>,
    /// The playing channel (catalog).
    pub playing: Signal<Option<usize>>,
    pub player_status: Signal<String>,
    /// Player panels over full-screen video.
    pub panels: Signal<Panels>,
    /// The pointer's play bar over full-screen video (`play_bar.rs`).
    pub play_bar: Signal<iptv_presentation::play_bar::PlayBar>,
    /// Outer bounds of this player window's bar, including margins and safe area.
    pub play_bar_size: Signal<Option<nsh_core::Size<Points>>>,
    pub paused: Signal<bool>,
    /// A catch-up seek being chosen: the target, in seconds into the
    /// program, opened once the keys pause (`seek.rs`).
    pub seek_target: Signal<Option<i32>>,
    /// Live or catch-up for the playing channel.
    pub playback: Signal<Playback>,
    pub channel_number:
        Signal<iptv_presentation::channel_number::ChannelNumber<iptv_search::ChannelId>>,
    pub sleep: Signal<SleepTimer>,
    /// Each player surface's stream, health, position, size and tracks;
    /// the channel history (`player.rs`).
    pub player: player::Player,
    pub player_surface: usize,
    /// Volume, mute and audio delay (`media.rs`).
    pub media: media::Media,
    /// Several channels at once, while open (`multiview.rs`).
    pub multiview: Signal<Option<multiview::State>>,
    pub player_windows: Signal<iptv_presentation::playback_windows::Workspace<String>>,
    /// The channel list over full-screen video, while open.
    pub channels: Signal<Option<iptv_presentation::channel_list::ChannelList>>,
    /// Group and search selection while adding or replacing a multiview tile.
    pub channel_picker: Signal<Option<multiview_picker::Screen>>,
    /// The picture-in-picture window, as the host reports it (`pip.rs`).
    pub pip: Signal<iptv_presentation::pip::Pip>,
}

impl PlaybackUi {
    /// Bind a fresh window to an already-mounted auxiliary player surface.
    pub(crate) fn for_secondary_window(self, parent: Self, surface: usize) -> Self {
        Self {
            player_surface: surface,
            // Every window observes and changes the same audible player.
            player_windows: parent.player_windows,
            playing: parent.player.stream(surface).channel,
            ..self
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ScreensUi {
    /// The open context menu, if any.
    pub menu: Signal<Option<Menu>>,
    /// The program description popover, if open.
    pub description: Signal<Option<Origin>>,
    pub(crate) description_scroll: ui_kit::FocusScroll,
    pub recordings: Signal<iptv_presentation::recording::Screen>,
    pub vod: Signal<iptv_presentation::vod::Screen>,
    pub recordings_open: Memo<bool>,
    /// The open chooser over the video (picture size, sleep timer).
    pub chooser: Signal<Option<chooser::Open>>,
    /// The favorites editor, when open.
    pub editor: Signal<Option<FavoritesEditor>>,
    pub(crate) organization: request_ui::RequestUi<organization_view::Open>,
    pub(crate) custom_groups: request_ui::RequestUi<custom_groups::Open>,
    pub remote_editor: Signal<Option<iptv_nav::remap::Editor>>,
    /// The search screen, while open (also while a result it chose plays).
    pub search: Signal<Option<search_view::Screen>>,
    pub search_history_revision: Signal<u64>,
    /// The add-playlist wizard, when open.
    pub wizard: Signal<Option<wizard_view::Open>>,
    pub(crate) restore_review: Signal<Option<backup::RestoreReview>>,
    /// The playlists screen, when open.
    pub playlists: Signal<Option<playlists_view::Open>>,
    /// A playlist's guide sources screen, when open (over the playlists
    /// and settings screens).
    pub guide_sources: Signal<Option<epg_sources_view::Open>>,
    pub guide_inspect: Signal<Option<iptv_presentation::library::guide_inspect::Screen>>,
    pub guide_repair: Signal<Option<iptv_presentation::library::guide_repair_screen::Screen>>,
    pub guide_repair_cancel: Stored<Option<iptv_import::CancelToken>>,
    pub guide_inspect_cancel: Stored<Option<iptv_import::CancelToken>>,
    /// The guide data chooser of a channel, when open.
    pub(crate) guide_assign: request_ui::RequestUi<guide_assign_view::Open>,
    /// The settings screen, when open.
    pub parental: Signal<Option<parental_view::Open>>,
    pub settings_screen: Signal<Option<iptv_presentation::settings::screen::Screen>>,
    pub(crate) update_dialog_open: Signal<bool>,
    pub(crate) updates: Option<settings_view::updates::UpdateUi>,
}

#[derive(Clone, Copy)]
pub(crate) struct ChromeUi {
    pub input: Signal<Input>,
    /// What the last pointer press was made with ([`Ui::touched`] reads it).
    pub last_press: Signal<Input>,
    pub mode: mode::Mode,
    pub stats: Signal<Stats>,
    #[cfg(feature = "test-support")]
    pub(crate) performance: Signal<Option<performance_probe::Probe>>,
    /// The question before quitting during a capture, with its focused
    /// answer (`desktop::quit_confirm`).
    pub(crate) quit_confirm: Signal<Option<desktop::quit_confirm::Choice>>,
    pub(crate) store_startup_failure: Signal<Option<startup_view::StoreFailure>>,
    pub(crate) store_failure_scroll: ui_kit::FocusScroll,
    /// A Long Back acted: the platform's Back on its release is swallowed
    /// (`input.rs`).
    pub back_guard: Signal<iptv_nav::BackGuard>,
    pub(crate) exit_deadline: Signal<Option<u64>>,
    /// Side effects wired by `Ui::new` when any control is touched or clicked.
    on_touch: Stored<Option<Rc<dyn Fn()>>>,
}

impl SharedUi {
    pub(crate) fn new() -> Self {
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
            account_streams: signal(None),
        }
    }

    /// Tracks the programs on screen: a new schedule or a day paged in.
    /// The value changes with either.
    pub fn guide_data(&self) -> u64 {
        self.data_version.get() << 32 | (self.guide_pages.get() & 0xFFFF_FFFF)
    }

    /// Milliseconds since the app started, on the engine's clock (a
    /// test's manual clock moves it with the timers).
    pub fn now_ms(&self) -> u64 {
        nsh_core::time::now()
            .saturating_duration_since(self.epoch)
            .as_millis() as u64
    }
}

impl ChromeUi {
    pub(crate) fn keys_shown(&self) -> bool {
        self.input.get().key_hints()
    }

    /// Visible ways back (`iptv_presentation::input_mode::InputMode::back_controls`):
    /// always on tablets and desktops, including after a keyboard is used;
    /// on TVs and terminals only once touch or a pointer is used.
    pub(crate) fn back_controls(&self) -> bool {
        let remote_back = matches!(
            idiom(),
            nsh_core::platform::Idiom::Tv | nsh_core::platform::Idiom::Terminal
        );
        self.input.get().back_controls(remote_back)
    }

    /// A touch or a click on any control: touch or pointer mode, whichever
    /// made the press.
    pub(crate) fn touched(&self) {
        self.exit_deadline.set(None);
        if let Some(hook) = self.on_touch.with(|slot| slot.clone()) {
            hook();
        }
        self.input.set(self.last_press.get_untracked());
    }
}

impl Ui {
    /// Allocate all presentation state afresh; shared signals and native stream
    /// ownership are supplied explicitly, never inherited from another window.
    pub(crate) fn new(app: SharedUi, player: player::Player) -> Self {
        let recordings = signal(iptv_presentation::recording::Screen::default());
        let recordings_open = computed(move || recordings.with(|s| s.open));
        let ui = Self {
            app,
            owner: Owner::current(),
            cast: casting::CastUi::new(),
            guide: GuideUi {
                focus: signal(Focus::default()),
                browse_playback: signal(Default::default()),
                autoplay_held: signal(None),
                pan_anchor: signal(None),
                list: signal(Rc::new(ChannelList::build(
                    iptv_presentation::lists::ALL,
                    &*schedule(),
                    &[],
                    &[],
                ))),
                visible_rows: signal(None),
            },
            playback: PlaybackUi {
                pending_zap: signal(None),
                touch_zapped: signal(false),
                swipe: signal(None),
                interruptions: signal(Default::default()),
                playing: signal(None),
                player_status: signal("idle".to_owned()),
                panels: signal(Panels::default()),
                play_bar: signal(Default::default()),
                play_bar_size: signal(None),
                paused: signal(false),
                seek_target: signal(None),
                playback: signal(Playback::Live),
                channel_number: signal(Default::default()),
                sleep: signal(SleepTimer::default()),
                player,
                player_surface: 0,
                media: media::Media::new(),
                multiview: signal(None),
                player_windows: signal(Default::default()),
                channels: signal(None),
                channel_picker: signal(None),
                pip: signal(Default::default()),
            },
            screens: ScreensUi {
                menu: signal(None),
                description: signal(None),
                description_scroll: ui_kit::FocusScroll::new(),
                recordings,
                vod: signal(iptv_presentation::vod::Screen::default()),
                recordings_open,
                chooser: signal(None),
                editor: signal(None),
                organization: request_ui::RequestUi::new(),
                custom_groups: request_ui::RequestUi::new(),
                remote_editor: signal(None),
                search: signal(None),
                search_history_revision: signal(0),
                wizard: signal(None),
                restore_review: nsh_core::element::signal_with(
                    None,
                    nsh_core::element::SignalEquals::Never,
                ),
                playlists: signal(None),
                guide_sources: signal(None),
                guide_inspect: signal(None),
                guide_repair: signal(None),
                guide_repair_cancel: stored(None),
                guide_inspect_cancel: stored(None),
                guide_assign: request_ui::RequestUi::new(),
                parental: signal(None),
                settings_screen: signal(None),
                update_dialog_open: signal(false),
                updates: crate::host::updates::AVAILABLE
                    .then(settings_view::updates::UpdateUi::new),
            },
            chrome: ChromeUi {
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
                mode: mode::Mode::new(app.settings),
                stats: signal(Stats::default()),
                #[cfg(feature = "test-support")]
                performance: signal(None),
                quit_confirm: signal(None),
                store_startup_failure: signal(None),
                store_failure_scroll: ui_kit::FocusScroll::new(),
                back_guard: signal(iptv_nav::BackGuard::default()),
                exit_deadline: signal(None),
                on_touch: stored(None),
            },
        };
        ui.chrome.on_touch.with(|slot| {
            *slot = Some(Rc::new(move || {
                // PIN entry continues the choice that opened the prompt. Dismissing
                // the prompt cancels it in `parental_view::abort`.
                if ui.top_overlay() != Some(Overlay::Parental) {
                    external_player::cancel_pending(ui.cast);
                }
                ui.playback.pending_zap.set(None);
                browse_playback::activity(ui);
            }));
        });
        player_panels::bind_auto_hide(app, ui.playback);
        ui
    }

    /// Tracks the programs on screen: a new schedule or a day paged in.
    /// The value changes with either.
    pub fn guide_data(&self) -> u64 {
        self.app.guide_data()
    }

    pub fn keys_shown(&self) -> bool {
        self.chrome.keys_shown()
    }

    /// A touch or a click on any control: touch or pointer mode, whichever
    /// made the press.
    pub fn touched(&self) {
        self.chrome.touched();
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

    /// Visible ways back (`iptv_presentation::input_mode::InputMode::back_controls`):
    /// always on tablets and desktops, including after a keyboard is used;
    /// on TVs and terminals only once touch or a pointer is used.
    pub fn back_controls(&self) -> bool {
        self.chrome.back_controls()
    }

    /// A foreground UI surface color. In guide mode the opaque colors are
    /// drawn by the backdrop *under* the video layer (`panels::backdrop`), so
    /// the foreground is clear and the preview rectangle shows the video; in
    /// overlay mode it tints the full-screen video.
    pub fn surface(&self, rgb_hex: u32) -> Rgba {
        let alpha = match self.chrome.mode.get() {
            // `appearance.ui_transparency`
            ViewMode::Overlay => theme::overlay_alpha(&settings_wiring::read(self.app)),
            ViewMode::Guide | ViewMode::Fullscreen => 0x00,
        };
        tint(rgb_hex, alpha)
    }

    /// Milliseconds since the app started, on the engine's clock (a
    /// test's manual clock moves it with the timers).
    pub fn now_ms(&self) -> u64 {
        self.app.now_ms()
    }

    /// Changes the panels at the current point in the app's clock.
    pub fn update_panels(&self, f: impl FnOnce(&mut Panels, u64)) {
        let now = self.now_ms();
        self.playback.panels.update(|p| f(p, now));
    }
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
