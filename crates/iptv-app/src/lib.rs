//! iptv-rs NSH shell (TV + tablet): remote (D-pad) and touch drive one
//! app-owned focus model (`guide.rs`, `browse.rs`); the layout follows the
//! reference UX's regions (header with the preview and the focused
//! programme, navigation rail and groups beside the guide grid) over one
//! persistent video layer (`video.rs`). The guide shows one list at a time (`lists.rs`).

use crate::i18n::tr;

pub use iptv_core::guide;
pub mod theme;

#[cfg(target_os = "android")]
mod android_transport;
mod backup;
mod browse_playback;
mod channels_view;
mod chooser;
mod content_image;
mod credential_key;
mod custom_groups;
mod data;
mod desktop;
mod epg_sources_view;
use runtime::executor;
mod fav_editor;
mod favorites;
mod fonts;
mod grid;
mod guide_assign_view;
mod header;
mod i18n;
mod input;
mod list_rows;
mod lists;
mod media;
mod menu_view;
use iptv_runtime as runtime;
pub(crate) use runtime::with_runtime;
mod runtime_events;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios", windows))]
use iptv_player_mpv as mpv;
mod multiview;
mod multiview_picker;
mod multiview_view;
mod numbers;
mod organization_view;
mod overlays;
mod panels;
mod pip;
mod platform_data;
mod play_bar;
mod player;
mod player_keys;
mod player_panels;
mod playlists_view;
mod rail;
mod remote_editor;
mod scale;
mod search_view;
mod seek;
mod settings;
mod settings_view;
pub mod startup;
pub mod time;
mod transport;
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
pub(crate) const IOS: bool = cfg!(target_os = "ios");

use std::time::{Duration, Instant};

use guide::{Airing, Focus, Pane, Program, Schedule};
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

#[cfg(windows)]
#[native_components(windows)]
pub fn components(components: &nsh_platform_windows::Components) {
    mpv::register(components, transport::policy);
}

/// The Linux desktop's native components: `tv.player` on libmpv (the
/// Android host registers its own in Kotlin).
#[cfg(target_os = "linux")]
#[native_components(linux)]
pub fn components(components: &nsh_platform_linux::Components) {
    mpv::register(components, transport::policy);
}

/// The macOS desktop's native components: `tv.player` on libmpv, as on
/// Linux.
#[cfg(target_os = "macos")]
#[native_components(macos)]
pub fn components(components: &nsh_platform_macos::Components) {
    mpv::register(components, transport::policy);
}

/// iOS and iPadOS's native components: `tv.player` on libmpv (MPVKit's,
/// OpenGL ES), as on the desktops.
#[cfg(target_os = "ios")]
#[native_components(ios)]
pub fn components(components: &nsh_platform_ios::Components) {
    mpv::register(components, transport::policy);
}

/// The last kind of input: the focus fill only shows for keys, as in
/// Android's touch mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Keys,
    Touch,
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
    /// An archived programme (its start in guide minutes), from `offset`
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

/// The app's state, shared by the regions (signals are `Copy`).
#[derive(Clone, Copy)]
pub struct Ui {
    /// The remote's focus; its `channel` is a row of `list`.
    pub focus: Signal<Focus>,
    pub browse_playback: Signal<iptv_core::browse_playback::BrowsePlayback>,
    pub pending_zap: Signal<Option<iptv_core::browse_playback::Target>>,
    pub guide_panning: Signal<bool>,
    /// The list the guide shows: catalog channels (`lists.rs`).
    pub list: Signal<Rc<ChannelList>>,
    /// The playing channel (catalog).
    pub playing: Signal<Option<usize>>,
    pub input: Signal<Input>,
    pub mode: Signal<ViewMode>,
    pub theme: Signal<Theme>,
    pub stats: Signal<Stats>,
    pub player_status: Signal<String>,
    /// The guide rows on screen, as the list reports them.
    pub visible_rows: Signal<Option<(usize, usize)>>,
    /// Player panels over full-screen video.
    pub panels: Signal<Panels>,
    /// The pointer's play bar over full-screen video (`play_bar.rs`).
    pub play_bar: Signal<iptv_core::play_bar::PlayBar>,
    pub paused: Signal<bool>,
    /// A catch-up seek being chosen: the target, in seconds into the
    /// programme, opened once the keys pause (`seek.rs`).
    pub seek_target: Signal<Option<i32>>,
    /// The app's monotonic clock origin (core state machines take ms), on
    /// the engine's clock: the one its timers run on.
    pub epoch: Instant,
    /// The open context menu, if any.
    pub menu: Signal<Option<Menu>>,
    /// The programme description popover, if open.
    pub description: Signal<Option<Origin>>,
    /// The favourite channels on screen (catalog channels); kept by key
    /// and changed through `favorites.rs`.
    pub favorites: Signal<BTreeSet<usize>>,
    /// The source has movies or shows (the rail lists them).
    pub has_vod: Signal<bool>,
    /// Live or catch-up for the playing channel.
    pub playback: Signal<Playback>,
    /// Bumped when the guide data changes (views re-read `schedule()`).
    pub data_version: Signal<u64>,
    /// Bumped when an older day of the guide was paged in (`data/pages.rs`):
    /// views showing programmes re-read them ([`Ui::guide_data`]).
    pub guide_pages: Signal<u64>,
    pub picture: Signal<PictureSettings>,
    pub sleep: Signal<SleepTimer>,
    /// The open chooser over the video (picture size, sleep timer).
    pub chooser: Signal<Option<chooser::Open>>,
    /// The favourites editor, when open.
    pub editor: Signal<Option<FavoritesEditor>>,
    pub organization: Signal<Option<organization_view::Open>>,
    pub custom_groups: Signal<Option<custom_groups::Open>>,
    pub remote_editor: Signal<Option<iptv_nav::remap::Editor>>,
    /// The search screen, while open (also while a result it chose plays).
    pub search: Signal<Option<search_view::Screen>>,
    /// The add-playlist wizard, when open.
    pub wizard: Signal<Option<wizard_view::Open>>,
    /// The playlists screen, when open.
    pub playlists: Signal<Option<playlists_view::Open>>,
    /// A playlist's guide sources screen, when open (over the playlists
    /// and settings screens).
    pub guide_sources: Signal<Option<epg_sources_view::Open>>,
    /// The guide data chooser of a channel, when open.
    pub guide_assign: Signal<Option<guide_assign_view::Open>>,
    /// The stored playlists: which one is shown, their refreshes (`data.rs`).
    pub library: data::Library,
    /// Bumped when a setting changes; read values with `settings::read`.
    pub settings: Signal<u64>,
    /// The settings screen, when open.
    pub settings_screen: Signal<Option<iptv_settings::screen::Screen>>,
    /// Each player surface's stream, health, position, size and tracks;
    /// the channel history (`player.rs`).
    pub player: player::Player,
    /// Volume, mute and audio delay (`media.rs`).
    pub media: media::Media,
    /// Several channels at once, while open (`multiview.rs`).
    pub multiview: Signal<Option<multiview::State>>,
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

impl Ui {
    /// Tracks the programmes on screen: a new schedule or a day paged in.
    /// The value changes with either.
    pub fn guide_data(&self) -> u64 {
        self.data_version.get() << 32 | (self.guide_pages.get() & 0xFFFF_FFFF)
    }

    /// OK or a tap on the focused programme: a live one plays (OK on the
    /// channel already playing goes full screen); ended and upcoming ones
    /// only show their details (no catch-up or reminders yet).
    pub fn watch(&self) {
        let view = lists::view(*self);
        let (row, program, now) = self.focus.with(|f| (f.channel, f.program(&view), f.now));
        let Some(channel) = view.channel(row) else {
            return;
        };
        match program.airing(now) {
            Airing::Live => {
                let live = self.playback.get() == Playback::Live;
                if self.playing.get() == Some(channel) && live {
                    self.mode.set(ViewMode::Fullscreen);
                    self.show_playing();
                } else {
                    browse_playback::select(*self);
                }
            }
            // A past programme plays from the archive when the channel has
            // one that reaches it; otherwise OK leaves the guide as it is.
            Airing::Ended if in_archive(channel, program, now) => {
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
            Airing::Ended | Airing::Upcoming => {}
        }
    }

    /// Starts `channel` live or from the archive.
    pub fn play(&self, channel: usize, playback: Playback) {
        self.pending_zap.set(None);
        self.browse_playback.set(Default::default());
        self.playback.set(playback);
        self.playing.set(Some(channel));
        self.paused.set(false);
        player::start(self, channel);
    }

    /// A touch on any control.
    pub fn touched(&self) {
        self.pending_zap.set(None);
        browse_playback::activity(*self);
        self.input.set(Input::Touch);
    }

    pub fn keys_shown(&self) -> bool {
        self.input.get() == Input::Keys
    }

    /// Visible ways back for pointer and touch users, including after a
    /// keyboard is used. TVs and terminals keep their key navigation.
    pub fn back_controls(&self) -> bool {
        !matches!(
            idiom(),
            nsh_core::platform::Idiom::Tv | nsh_core::platform::Idiom::Terminal
        )
    }

    /// A foreground UI surface colour. In guide mode the opaque colours are
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
            .saturating_duration_since(self.epoch)
            .as_millis() as u64
    }

    /// What just started playing over full-screen video: the banner for
    /// a live channel, the card with its seek bar for a replay.
    pub fn show_playing(&self) {
        let live = self.playback.get_untracked() == Playback::Live;
        self.update_panels(|p, now| {
            if live {
                p.show_banner(now);
            } else {
                p.show_info(now);
            }
        });
    }

    /// Changes the panels and (re)schedules their auto-hide.
    pub fn update_panels(&self, f: impl FnOnce(&mut Panels, u64)) {
        let now = self.now_ms();
        self.panels.update(|p| f(p, now));
        if let Some(at) = self.panels.with(|p| p.hide_at_ms()) {
            let ui = *self;
            let _task = set_timeout(Duration::from_millis(at.saturating_sub(now)), move || {
                let now = ui.now_ms();
                // Later input moved the deadline: this tick is a no-op then.
                if ui
                    .panels
                    .with(|p| p.hide_at_ms().is_some_and(|at| now >= at))
                {
                    let at = ui.panels.with(|p| p.hide_at_ms());
                    log::info!("iptv panels auto-hide now={now} at={at:?}");
                    ui.panels.update(|p| {
                        p.tick(now);
                    });
                }
            });
        }
    }

    pub(crate) fn toggle(&self, mode: ViewMode) {
        self.mode
            .update(|m| *m = if *m == mode { ViewMode::Guide } else { mode });
    }
}

thread_local! {
    static RUNTIME: RefCell<Option<Rc<runtime::Runtime>>> = const { RefCell::new(None) };
}

/// The shell owns one application handle; all library state belongs to it.
pub(crate) fn runtime() -> Rc<runtime::Runtime> {
    runtime::try_current().unwrap_or_else(|| {
        let owner = RUNTIME.with(|runtime| {
            runtime
                .borrow_mut()
                .get_or_insert_with(|| {
                    let directory = platform_data::resolve_data_dir()
                        .expect("private storage must be resolved before using runtime services");
                    Rc::new(new_runtime(directory))
                })
                .clone()
        });
        runtime::set_current(&owner);
        owner
    })
}

fn new_runtime(directory: std::path::PathBuf) -> runtime::Runtime {
    let mut ports = runtime::Ports::new(directory, credential_key::database_key);
    ports.exclude_from_backup = platform_data::exclude_from_backup;
    ports.startup_mark = |phase, detail| startup::mark_with(phase, detail);
    runtime::Runtime::new(ports)
}

pub(crate) fn in_runtime(
    future: impl std::future::Future<Output = ()> + 'static,
) -> impl std::future::Future<Output = ()> {
    let owner = runtime();
    runtime::with_runtime(owner, || runtime::in_runtime(future))
}

/// The guide data in use (synthetic until a playlist is imported).
pub fn schedule() -> Rc<dyn Schedule> {
    runtime().schedule()
}

/// Swaps the guide data (after an import) and tells the views to re-read.
/// The guide keeps its list and focused channel by identity, at the new
/// schedule's now; channel indices into the old data (open menus) that no
/// longer exist are dropped, the playing channel and favourites are found
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
    ui.data_version.update(|v| *v += 1);
}

/// Whether `program` on `channel` is inside the channel's catch-up window
/// at guide minute `now`.
pub fn in_archive(channel: usize, program: Program, now: i32) -> bool {
    let days = i32::from(schedule().catchup_days(channel));
    days > 0 && program.end <= now && program.start >= now - days * 24 * 60
}

/// `0xRRGGBB` with alpha, as an NSH colour.
pub fn tint(rgb_hex: u32, alpha: u8) -> Rgba {
    rgba(theme::with_alpha(rgb_hex, alpha))
}

pub fn channel_name(ch: usize) -> String {
    schedule().channel_name(ch)
}

#[component]
pub fn App() -> impl IntoElement {
    startup::mark("rust root");
    startup::log_library_base();
    fonts::register();
    let directory = match platform_data::initialize() {
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
    let owner = Rc::new(new_runtime(directory));
    runtime::set_current(&owner);
    owner.activate();
    RUNTIME.set(Some(owner));
    if let Err(error) = backup::lock() {
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
    let started = std::thread::Builder::new()
        .name("iptv-bootstrap".into())
        .spawn(runtime().scope_worker(move || {
            let result = match backup::startup() {
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
        }));
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
    view().flex_1().child(dynamic(move || {
        let result = outcome.get();
        // Only the one-shot outcome controls this branch. Reads while building
        // ReadyApp must not subscribe the branch and recreate its video surface.
        untracked(move || match result {
            None => view()
                .flex_1()
                .items_center()
                .justify_center()
                .child(text(i18n::tr("Opening saved library…")))
                .into_element(),
            Some(Err(message)) => view()
                .flex_1()
                .items_center()
                .justify_center()
                .child(text(i18n::tr("Saved data could not be recovered")))
                .child(text(i18n::tr(message)))
                .child(text(i18n::tr("Close Patina and try opening it again.")))
                .into_element(),
            Some(Ok(())) => ready_app().into_element(),
        })
    }))
}

/// Constructed once after recovery; all player surfaces belong to this owner.
fn ready_app() -> impl IntoElement {
    settings::load();
    startup::mark("settings loaded");
    let ui = Ui {
        focus: signal(Focus::default()),
        browse_playback: signal(Default::default()),
        pending_zap: signal(None),
        guide_panning: signal(false),
        list: signal(Rc::new(ChannelList::build(
            iptv_core::lists::ALL,
            &*schedule(),
            &[],
            &[],
        ))),
        playing: signal(None),
        // An iPad is touched first: no remote hints until a key is used.
        input: signal(if cfg!(target_os = "ios") {
            Input::Touch
        } else {
            Input::Keys
        }),
        mode: signal(ViewMode::Guide),
        theme: signal(Theme::default()),
        stats: signal(Stats::default()),
        player_status: signal("idle".to_owned()),
        visible_rows: signal(None),
        panels: signal(Panels::default()),
        play_bar: signal(Default::default()),
        paused: signal(false),
        seek_target: signal(None),
        epoch: nsh_core::time::now(),
        menu: signal(None),
        description: signal(None),
        favorites: signal(BTreeSet::new()),
        has_vod: signal(false),
        playback: signal(Playback::Live),
        data_version: signal(0),
        guide_pages: signal(0),
        picture: signal(PictureSettings::default()),
        sleep: signal(SleepTimer::default()),
        chooser: signal(None),
        editor: signal(None),
        organization: signal(None),
        custom_groups: signal(None),
        remote_editor: signal(None),
        search: signal(None),
        wizard: signal(None),
        playlists: signal(None),
        guide_sources: signal(None),
        guide_assign: signal(None),
        library: data::Library::new(),
        settings: signal(0),
        settings_screen: signal(None),
        player: player::Player::new(),
        media: media::Media::new(),
        multiview: signal(None),
        channels: signal(None),
        channel_picker: signal(None),
        pip: signal(Default::default()),
        back_guard: signal(iptv_nav::BackGuard::default()),
    };
    settings::bind(ui);
    browse_playback::bind(ui);
    desktop::bind();
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
    input::bind(ui);
    data::start(ui);
    time::start(ui);
    player::bind(ui);
    data::pages::bind(ui);
    play_bar::bind(ui);
    multiview::bind(ui);
    pip::bind(ui);
    bind_back(ui);

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
        .bg(move || rgb(ui.theme.with(|t| t.background.window)))
        // A moving mouse over full-screen video shows the player panels.
        .on_mouse_move(move |e: &MouseEvent| pointer_moved(ui, e))
        // Layers, bottom to top: opaque backdrop, video (multiview's other
        // surfaces while their tiles play), UI, full-screen tap catcher.
        // The picture-in-picture window shows only the video (`pip.rs`).
        .child(backup::service_layer())
        .child(panels::backdrop(ui))
        .child(video::layer(ui, 0))
        .children((1..player::SURFACES).map(move |s| {
            when(
                move || multiview::mounted(ui, s),
                move || video::layer(ui, s),
            )
        }))
        .child(ui_layers(ui, groups, rows))
        // The desktops' picture-in-picture window, while it is open.
        .child(desktop::pip_window::window(ui))
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
                .accessibility_elements_hidden(move || fullscreen() || ui.any_overlay_open())
                .opacity(move || if fullscreen() { 0.0_f32 } else { 1.0 })
                .pointer_events(move || {
                    if fullscreen() {
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
        // Over the popups it opens (choosers, programme details), so its
        // buttons stay usable and toggle them.
        .child(play_bar::layer(ui))
        .child(fav_editor::layer(ui))
        .child(settings_view::layer(ui))
        .child(guide_assign_view::layer(ui))
        // The playlists screen opens over settings, a guide sources screen
        // over both.
        .child(playlists_view::layer(ui))
        .child(remote_editor::layer(ui))
        .child(custom_groups::layer(ui))
        .child(organization_view::layer(ui))
        .child(epg_sources_view::layer(ui))
        .child(wizard_view::layer(ui))
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
        if play_bar::available() {
            play_bar::tapped(ui);
        }
        return;
    }
    if play_bar::available() {
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

/// The mouse moved: over full-screen video with nothing open above the
/// panels, it shows the play bar (multiview's pointer chrome over its
/// grid) and keeps it while it moves.
fn pointer_moved(ui: Ui, e: &MouseEvent) {
    let open = untracked(|| ui.overlay_open(overlays::Overlay::keeps_pointer_from_play_bar));
    let (x, y) = (e.page_position.x.0, e.page_position.y.0);
    if ui.mode.get_untracked() != ViewMode::Fullscreen
        || open
        || !desktop::pointer_moved(x, y, ui.now_ms())
    {
        return;
    }
    if ui.input.get_untracked() != Input::Touch {
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
            _ if top == Some(Overlay::Wizard) => wizard_view::back(ui),
            _ if top == Some(Overlay::GuideSources) => epg_sources_view::back(ui),
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
            _ if desktop::fullscreen() => {
                desktop::set_fullscreen(false);
                true
            }
            // A dialog closes, else the active tile goes full screen.
            _ if multiview::is_open(ui) => multiview::back(ui),
            // A tile watched full screen: back to the grid as it was.
            _ if multiview::detour(ui) => multiview::resume(ui),
            (ViewMode::Fullscreen, _, true) => {
                ui.panels.update(|p| p.hide());
                ui.play_bar.update(iptv_core::play_bar::PlayBar::hide);
                true
            }
            // Hint, past programme → now, guide → groups → rail → leave
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

/// Explicit synthetic-key injection for the headless test binary only.
#[cfg(feature = "test-support")]
pub use credential_key::install_test_key;
/// Whether a stored source has a guide timer or an in-flight guide read,
/// for the headless tests of what keeps its periodic checks alive.
#[cfg(feature = "test-support")]
pub use runtime::guide_check_registered as guide_check_pending;
/// Keeps the store's writer busy, as a long catalog or guide publish does,
/// for the headless tests of what must not wait for it.
#[cfg(feature = "test-support")]
pub use runtime::occupy_writer;
/// The refresh workers refuse work as when their queue is full, and an
/// import waits just before it reports its catalog, for the headless
/// tests of what such moments must not lose.
#[cfg(feature = "test-support")]
pub use runtime::{hold_reports, refuse_work, reports_waiting};
