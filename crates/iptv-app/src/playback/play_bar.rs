//! The pointer's play bar at the bottom of full-screen video: on the
//! desktop the mouse moving over the video shows it, on a tablet a tap
//! (a click on the desktop) shows it and another hides it; it hides after
//! a few seconds without input, but not while the mouse rests on it nor
//! while a screen reader runs (`iptv_presentation::play_bar`). A TV opens the
//! same bar with OK or Menu and traverses its controls with the D-pad.
//!
//! Above the controls: a single compact line with the channel, program title,
//! stream quality and (on wider screens) what is on next. Sound and guide
//! navigation sit left of transport. Favorite, picture in picture, multiview
//! and display sit to its right. More holds the available actions that do not
//! fit, plus Record now on Android handhelds and multiview on Android phones.
//! Live playback shows a thin hairline progress indicator at the top of the
//! panel and omits the timeline row; catch-up and on-demand playback show the
//! interactive timeline below the controls.
//! The bar acts on the full-screen player (surface 0). While multiview's grid is open it
//! does not show (the tiles take the taps), and its state then times the
//! grid's pointer chrome instead (`multiview.rs`). On a multiview detour
//! (a tile watched full screen) its Multiview button goes back to the
//! grid.

use crate::chrome::i18n::tr;

mod info;
mod progress;

use std::time::Duration;

use iptv_core::tracks::Kind as Track;
use iptv_nav::{Action, RemoteKey};
use iptv_presentation::capabilities::RecordNow;
use iptv_presentation::channel_list::ListMode;
use iptv_presentation::menu::Origin;
use iptv_presentation::panels::QuickAction;
use iptv_presentation::play_bar::{Control, PlayBar};
use nsh_core::platform::Idiom;
use nsh_core::prelude::*;
use nsh_core::{Checked, TouchEvent};

use crate::chrome::ui_kit::icons::{self, Icon};
use crate::theme::{RADIUS_LARGE, RESTING_ALPHA, TEXT_DISABLED_ALPHA};
use crate::{Playback, Ui, ViewMode, tint};

const WHITE: u32 = 0xFFFFFF;
const PANEL_ALPHA: u8 = 0xE0;
/// Volume and window full screen are the app's on the desktop only
/// (Linux and macOS; Android keeps the system volume).
const DESKTOP: bool = crate::chrome::desktop::DESKTOP;

/// Whether the device has a pointer for the bar: anything but a TV (its
/// remote has the panels) and a terminal.
pub fn available() -> bool {
    idiom() != Idiom::Terminal
}
/// Pointer gestures retain their existing TV info-card behavior.
pub fn pointer_available() -> bool {
    !matches!(idiom(), Idiom::Tv | Idiom::Terminal)
}

/// A button's side: a finger's target on touch devices, slimmer under a
/// desktop mouse. The phone uses Apple's 44-point touch minimum.
fn target() -> f32 {
    if idiom() == Idiom::Desktop {
        34.0
    } else if crate::chrome::scale::phone() {
        44.0
    } else {
        48.0
    }
}

/// Whether the bar shows the full interactive timeline row: catch-up and
/// on-demand playback (including while a title is still preparing). Live TV
/// uses a slim hairline progress bar at the top of the panel instead.
fn show_timeline(ui: Ui) -> bool {
    matches!(ui.playback.playback.get(), Playback::CatchUp { .. })
        || crate::playback::on_demand::active(ui)
}

/// Whether the bar has the desktop's own controls (the volume slider, the
/// window's full screen): a desktop build, not a TV, and not a headless
/// phone (whose device has neither).
fn desktop_controls() -> bool {
    DESKTOP && idiom() != Idiom::Tv && !crate::chrome::scale::phone()
}

/// Keep enough room for the centered transport in tight windows.
fn bar_margin() -> f32 {
    let safe = safe_area();
    safe.left.0.max(safe.right.0)
        + if crate::chrome::scale::phone() || window().width.0 < 360.0 {
            PHONE_BAR_MARGIN
        } else {
            BAR_MARGIN
        }
}

/// Whether the bar is on screen.
pub fn shows(ui: Ui) -> bool {
    ui.chrome.mode.get() == ViewMode::Fullscreen
        && (idiom() == Idiom::Tv || !ui.keys_shown())
        && ui.playback.play_bar.with(|b| b.shown)
        && !crate::playback::multiview::is_open(ui)
        && available()
        // A chooser needs usable list rows on a short landscape screen.
        // The held bar returns with its original focus when the popup closes.
        && (window().height.0 - safe_area().top.0 - safe_area().bottom.0 >= 400.0
            || !(ui.screens.chooser.with(Option::is_some) || ui.screens.description.with(Option::is_some)))
}

/// Leaving full screen or a key (off a TV) hides the bar; it stays while
/// a popup over the video (a chooser, the program details) is open or a
/// screen reader runs.
pub fn bind(ui: Ui) {
    effect(move || {
        let away = ui.chrome.mode.get() != ViewMode::Fullscreen
            || (idiom() != Idiom::Tv && ui.keys_shown());
        if away && ui.playback.play_bar.with_untracked(|b| b.shown) {
            ui.playback.play_bar.update(PlayBar::hide);
        }
    });
    // A screen reader's user moves from control to control by swipes, a
    // switch user by switch, slower than the delay: the bar stays until
    // it is hidden.
    effect(move || {
        let reader = crate::wiring::device::assistive();
        if untracked(|| ui.playback.play_bar.with(|b| b.reader != reader)) {
            untracked(|| update(ui, |b, now| b.screen_reader(reader, now)));
        }
    });
    // Otherwise it waits at least as long as the system recommends for
    // controls (Android's accessibility timeout).
    effect(move || {
        let idle = iptv_presentation::play_bar::HIDE_AFTER_MS
            .max(crate::wiring::device::recommended().controls_ms);
        if untracked(|| ui.playback.play_bar.with(|b| b.idle_ms() != idle)) {
            untracked(|| update(ui, |b, now| b.set_idle_ms(idle, now)));
        }
    });
    let popup = computed(move || {
        ui.screens.chooser.with(Option::is_some)
            || ui.screens.description.with(Option::is_some)
            || ui.cast.picker_open.get()
    });
    effect(move || {
        let open = popup.get();
        if untracked(|| ui.playback.play_bar.with(|b| b.held != open)) {
            untracked(|| update(ui, |b, now| b.hold(open, now)));
        }
    });
}

/// The bar's height above the window's bottom (with its margin and the
/// safe area): popups over the video keep clear of it. Until the first
/// layout use an estimate.
pub fn height(ui: Ui) -> f32 {
    if let Some(size) = ui.playback.play_bar_size.get() {
        return size.height.0;
    }
    let row = target();
    let extra = if show_timeline(ui) {
        PANEL_GAP + progress::TIMELINE_HEIGHT
    } else if ui.playback.playing.get().is_some() {
        PANEL_GAP + progress::HAIRLINE_HEIGHT
    } else {
        0.0
    };
    PANEL_PAD_Y
        + info::ESTIMATED_HEIGHT
        + PANEL_GAP
        + row
        + extra
        + PANEL_PAD_Y
        + BAR_BOTTOM_MARGIN
        + safe_area().bottom.0
}

/// Changes the bar's state and (re)schedules its hiding.
fn update(ui: Ui, f: impl FnOnce(&mut PlayBar, u64)) {
    // An auxiliary control (Guide, for example) can close its window and
    // dispose this local presentation before the press handler finishes.
    if untracked(|| ui.playback.play_bar.try_get()).is_none() {
        return;
    }
    let now = ui.now_ms();
    ui.playback.play_bar.update(|b| f(b, now));
    let Some(at) = ui.playback.play_bar.with_untracked(PlayBar::hide_at_ms) else {
        return;
    };
    // The window owns the idle deadline even when a resize replaces a button.
    ui.owner.run(|| {
        let _task = set_timeout(Duration::from_millis(at.saturating_sub(now)), move || {
            let now = ui.now_ms();
            // Later input moved the deadline: then this does nothing.
            if untracked(|| ui.playback.play_bar.try_get())
                .is_some_and(|b| b.hide_at_ms().is_some_and(|at| now >= at))
            {
                ui.playback.play_bar.update(|b| {
                    b.tick(now);
                });
            }
        });
    });
}

/// The bar replaces the remote's panels while a pointer is used.
fn show(ui: Ui, f: impl FnOnce(&mut PlayBar, u64)) {
    if ui.playback.panels.with_untracked(|p| p.shown) != iptv_presentation::panels::Shown::Hidden {
        ui.playback.panels.update(|p| p.hide());
    }
    update(ui, f);
}

/// The mouse moved over full-screen video (a TV's air mouse leaves the
/// remote's panels as they are: no bar there).
pub fn pointer_moved(ui: Ui) {
    if !pointer_available() {
        return;
    }
    show(ui, PlayBar::pointer_moved);
}

/// A tap or a click on full-screen video.
pub fn tapped(ui: Ui) {
    show(ui, PlayBar::tapped);
}

/// A remote key held over the bar decides tap or hold at `at` (ms): the
/// bar stays until then.
pub(crate) fn keep_until(ui: Ui, at: u64) {
    update(ui, |b, _| b.keep_until(at));
}

/// That key's press was decided: the bar's own delay alone counts.
pub(crate) fn release_hold(ui: Ui) {
    update(ui, |b, _| b.release_hold());
}

/// A control of the bar (or of multiview's pointer chrome) acted.
pub(crate) fn used(ui: Ui) {
    update(ui, PlayBar::used);
}

/// The mouse came over the bar (or multiview's toolbar), or left it.
pub(crate) fn hover(ui: Ui, over: bool) {
    update(ui, |b, now| b.hover(over, now));
}

pub fn layer(ui: Ui, back_action: fn(Ui) -> bool) -> impl IntoElement {
    // Back belongs to the pointer chrome: keys and auto-hide hide both.
    let shown = computed(move || shows(ui));
    when(
        move || shown.get(),
        move || {
            view()
                .absolute()
                .inset_0()
                .test_id("play-bar-layer")
                .accessibility_elements_hidden(move || {
                    ui.overlay_open(|overlay| {
                        !matches!(
                            overlay,
                            crate::chrome::overlays::Overlay::Chooser
                                | crate::chrome::overlays::Overlay::Description
                        )
                    })
                })
                .pointer_events(PointerEvents::BoxNone)
                .child(when(
                    move || ui.back_controls(),
                    move || back(ui, back_action),
                ))
                .child(bar(ui))
        },
    )
}

/// A tablet's way back from full-screen video, where an iPad's players
/// keep it: a round ‹ top left, with the bar. It leaves at once for the
/// guide, a played title's details (Movies or Shows), Recordings, or
/// multiview's grid from a tile watched full screen (its
/// accessibility label says which; a remote's Back and an iPad's swipe
/// from the left edge hide the bar first).
fn back(ui: Ui, back_action: fn(Ui) -> bool) -> impl IntoElement {
    let window = nsh_core::element::desktop::current_window();
    let side = target();
    view()
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .pl_safe_area_plus(px(20.0))
        .pt_safe_area_plus(px(16.0))
        // Only the button takes presses: around it they reach the video.
        .pointer_events(PointerEvents::BoxNone)
        .child(
            crate::chrome::ui_kit::round_back(
                side,
                0.0,
                back_bg(ui),
                move || {
                    if crate::playback::multiview::detour(ui) {
                        tr("Multiview")
                    } else if ui.playback.player.recorded.with(Option::is_some) {
                        tr("Recordings")
                    } else if let Some(kind) = crate::screens::vod_view::returns_to(ui) {
                        if kind == iptv_core::vod::Kind::Movie {
                            tr("Movies")
                        } else {
                            tr("Shows")
                        }
                    } else {
                        tr("Guide")
                    }
                },
                move || {
                    if !window.is_main() {
                        window.close();
                        return;
                    }
                    ui.touched();
                    log::info!("iptv play bar: back");
                    ui.playback.panels.update(|p| p.hide());
                    ui.playback.play_bar.update(PlayBar::hide);
                    back_action(ui);
                },
            )
            .test_id("play-bar-back"),
        )
}

/// The round ‹'s color over video (the player's, the guide's over its
/// preview): the bar's panel.
pub(crate) fn back_bg(ui: Ui) -> impl Fn() -> Rgba + Copy + 'static {
    move || tint(ui.app.theme.with(|t| t.background.panel), PANEL_ALPHA)
}

/// The bar's margin at the window's sides and bottom, and the panel's padding
/// and vertical gap inside it (points).
const BAR_MARGIN: f32 = 24.0;
const PHONE_BAR_MARGIN: f32 = 12.0;
const BAR_BOTTOM_MARGIN: f32 = 10.0;
const PANEL_PADDING: f32 = 12.0;
const PANEL_PAD_Y: f32 = 6.0;
const PANEL_GAP: f32 = 3.0;
/// The gap between the bar's groups, and between the transport's buttons.
const GROUP_GAP: f32 = 8.0;
const TRANSPORT_GAP: f32 = 6.0;

fn group_gap() -> f32 {
    if window().width.0 < 360.0 {
        4.0
    } else {
        GROUP_GAP
    }
}

const LEFT: &[Control] = &[
    Control::Mute,
    Control::Volume,
    Control::Listen,
    Control::Tracks,
    Control::Channels,
    Control::Guide,
];
const CENTER: &[Control] = &[Control::Rewind, Control::PlayPause, Control::Forward];
const RIGHT: &[Control] = &[
    Control::Favorite,
    Control::Cast,
    Control::Pip,
    Control::Multiview,
    Control::Picture,
    Control::Record,
    Control::Details,
    Control::Fullscreen,
    Control::More,
];
const VOLUME_WIDTH: f32 = 96.0;
const BUTTON_GAP: f32 = 2.0;

#[derive(PartialEq, Clone)]
struct Layout {
    row: Vec<Control>,
    more: Vec<Control>,
}

fn group_width(group: &[Control], row: &[Control], size: f32) -> f32 {
    let gap = if group == CENTER {
        TRANSPORT_GAP
    } else {
        BUTTON_GAP
    };
    let mut width = 0.0;
    for control in group.iter().filter(|c| row.contains(c)) {
        if width > 0.0 {
            width += gap;
        }
        width += match control {
            Control::Volume => VOLUME_WIDTH,
            _ => size,
        };
    }
    width
}

fn layout(ui: Ui) -> Layout {
    let width = ui
        .playback
        .play_bar_size
        .get()
        .map_or_else(|| window().width.0, |s| s.width.0);
    layout_for_width(ui, width)
}

pub(crate) fn overflow(ui: Ui, width: f32) -> Vec<Control> {
    layout_for_width(ui, width).more
}

fn layout_for_width(ui: Ui, width: f32) -> Layout {
    use Control::*;
    let size = target();
    let available = width - 2.0 * (bar_margin() + PANEL_PADDING);
    // Record now's place follows the device (`capabilities::record_now`):
    // in the row, under More only, or nowhere.
    let record = crate::wiring::capabilities::record_now();
    let eligible = |c: &Control| match c {
        Pip => crate::playback::pip::supported(ui) && enabled(ui, Pip),
        Record => enabled(ui, Record),
        More => false,
        _ => shown(ui, *c),
    };
    let phone = phone_form();
    let under_more = |c: &Control| match c {
        Record => record != RecordNow::Direct,
        Cast | Multiview => phone,
        _ => false,
    };
    let candidates: Vec<_> = LEFT
        .iter()
        .chain(CENTER)
        .chain(RIGHT)
        .copied()
        .filter(eligible)
        .collect();
    let mut row: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|c| !under_more(c))
        .collect();
    let mut more: Vec<_> = candidates.iter().copied().filter(under_more).collect();
    let needed = |row: &[Control]| {
        group_width(LEFT, row, size)
            + group_width(CENTER, row, size)
            + group_width(RIGHT, row, size)
            + 2.0 * group_gap()
    };
    // What belongs under More alone still needs the button.
    if !more.is_empty() {
        row.push(More);
    }
    if needed(&row) > available {
        row.clear();
        more.clear();
        row.push(More);
        // Keep transport direct where it fits, including at widths below
        // the full replay transport. A wide slider can yield to a later icon.
        for control in [
            PlayPause, Cast, Rewind, Forward, Favorite, Guide, Pip, Mute, Multiview, Listen,
            Record, Channels, Tracks, Volume, Details, Picture, Fullscreen,
        ] {
            if candidates.contains(&control) && !under_more(&control) {
                row.push(control);
                if needed(&row) > available {
                    row.pop();
                }
            }
        }
        more.extend(candidates.iter().filter(|c| !row.contains(c)).copied());
        row = LEFT
            .iter()
            .chain(CENTER)
            .chain(RIGHT)
            .copied()
            .filter(|c| row.contains(c))
            .collect();
    }
    Layout { row, more }
}

/// The bar, made again when the screen turns from a phone's into a
/// tablet's or back (a foldable phone, unfolded): its buttons' side and
/// margins are read as it is made.
fn bar(ui: Ui) -> impl IntoElement {
    when(crate::chrome::scale::phone, move || bar_now(ui)).otherwise(move || bar_now(ui))
}

fn bar_now(ui: Ui) -> impl IntoElement {
    // Hidden, the bar is unmounted: forget its height, which a resize or a
    // text change meanwhile makes stale, until it lays out again.
    cleanup(move || {
        if untracked(|| ui.playback.play_bar_size.try_get()).is_some() {
            ui.playback.play_bar_size.set(None);
        }
    });
    let controls = computed(move || layout(ui));
    effect(move || {
        let focus = ui.playback.play_bar.with(|b| b.focus);
        let invalid = focus.is_some_and(|c| {
            if c == Control::Timeline {
                !show_timeline(ui) || !enabled(ui, Control::Timeline)
            } else {
                !controls.with(|l| l.row.contains(&c))
            }
        });
        if invalid {
            let next = controls.with(|l| {
                if l.row.contains(&Control::PlayPause) {
                    Control::PlayPause
                } else if l.row.contains(&Control::More) {
                    Control::More
                } else {
                    Control::PlayPause
                }
            });
            update(ui, |b, now| b.focus_control(next, now));
        }
    });
    let panel = move || tint(ui.app.theme.with(|t| t.background.panel), PANEL_ALPHA);
    view()
        .absolute()
        .left(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .px(move || px(bar_margin()))
        .pb_safe_area_plus(px(BAR_BOTTOM_MARGIN))
        .on_layout(move |b: &Bounds<Points>| {
            if ui.playback.play_bar_size.get_untracked() != Some(b.size) {
                ui.playback.play_bar_size.set(Some(b.size));
            }
        })
        // Only the panel takes presses: around it they reach the video.
        .pointer_events(PointerEvents::BoxNone)
        .child(
            view()
                .px(px(PANEL_PADDING))
                .pt(px(PANEL_PAD_Y))
                .pb(px(PANEL_PAD_Y))
                .gap(px(PANEL_GAP))
                .rounded(px(RADIUS_LARGE))
                .bg(panel)
                .on_mouse_enter(move |_: &MouseEvent| hover(ui, true))
                .on_mouse_leave(move |_: &MouseEvent| hover(ui, false))
                .child(when(
                    move || !show_timeline(ui) && ui.playback.playing.get().is_some(),
                    move || progress::live_hairline(ui),
                ))
                .child(info::header(ui))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(move || px(group_gap()))
                        .child(group(ui, controls, LEFT))
                        .child(group(ui, controls, CENTER))
                        .child(group(ui, controls, RIGHT)),
                )
                .child(when(move || show_timeline(ui), move || progress::line(ui))),
        )
}

/// An icon button shared by touch, pointer and D-pad. Admission and actions
/// are resolved from the same stable identity on every input path.
fn button(ui: Ui, control: Control, size: f32) -> View {
    if control == Control::Cast && crate::screens::airplay::direct() {
        return crate::screens::airplay::button(ui, size, control_id(control));
    }
    let target_window = matches!(control, Control::More | Control::Fullscreen)
        .then(nsh_core::element::desktop::current_window);
    let icon_window = target_window.clone();
    let button = view()
        .role(AccessibilityRole::Button)
        .accessible(true)
        .test_id(control_id(control))
        .disabled(move || !shown(ui, control))
        .accessibility_state(move || AccessibilityState {
            busy: control == Control::Cast && ui.cast.busy.get(),
            selected: focused(ui, control),
            has_popup: control == Control::More,
            expanded: (control == Control::More).then(|| {
                ui.screens.chooser.with(|c| {
                    c.as_ref()
                        .is_some_and(|c| c.kind == crate::chrome::chooser::Kind::More)
                })
            }),
            pressed: match control {
                Control::PlayPause => Some(if ui.playback.paused.get() {
                    Checked::False
                } else {
                    Checked::True
                }),
                Control::Record => Some(if crate::screens::recordings_view::recording_active(ui) {
                    Checked::True
                } else {
                    Checked::False
                }),
                _ => None,
            },
            ..Default::default()
        })
        .flex_shrink_0()
        .w(px(size))
        .h(px(size))
        .rounded(px(size / 2.0))
        .border(px(2.0))
        .border_color(move || {
            tint(
                ui.app.theme.with(|t| t.accent.light),
                if focused(ui, control) { 255 } else { 0 },
            )
        })
        .items_center()
        .justify_center()
        .label(move || label(ui, control))
        // Why a control drawn disabled cannot act, said on focus (#279).
        .optional_accessibility_description(move || {
            (control == Control::Multiview)
                .then(|| crate::playback::multiview::disabled_reason(ui))
                .flatten()
                .map(SharedString::from)
        })
        .hover(|s| s.bg(tint(WHITE, RESTING_ALPHA * 3)))
        .on_press(move || {
            ui.touched();
            activate(ui, control, target_window.clone());
        })
        .child(
            svg(move || icons::src(icon(ui, control, icon_window.as_ref())))
                .size(px(size * 0.55))
                .text_color(move || {
                    tint(
                        if control == Control::Record
                            && crate::screens::recordings_view::recording_active(ui)
                        {
                            crate::theme::RECORDING
                        } else {
                            WHITE
                        },
                        if enabled(ui, control) {
                            0xFF
                        } else {
                            TEXT_DISABLED_ALPHA
                        },
                    )
                }),
        );
    if control == Control::PlayPause {
        button.bg(tint(WHITE, RESTING_ALPHA * 2))
    } else {
        button
    }
}

fn control_label(control: Control) -> &'static str {
    match control {
        Control::Mute => crate::chrome::i18n::key("Mute"),
        Control::Listen => crate::chrome::i18n::key("Listen to this"),
        Control::More => crate::chrome::i18n::key("More options"),
        Control::Volume => crate::chrome::i18n::key("Volume"),
        Control::Rewind => crate::chrome::i18n::key("Rewind"),
        Control::PlayPause => crate::chrome::i18n::key("Play or pause"),
        Control::Forward => crate::chrome::i18n::key("Fast-forward"),
        Control::Record => crate::chrome::i18n::key("Record"),
        Control::Favorite => crate::chrome::i18n::key("Favorite"),
        Control::Tracks => crate::chrome::i18n::key("Subtitles and tracks"),
        Control::Details => crate::chrome::i18n::key("Program details"),
        Control::Picture => crate::chrome::i18n::key("Display"),
        Control::Channels => crate::chrome::i18n::key("Channel list"),
        Control::Multiview => crate::chrome::i18n::key("Multiview"),
        Control::Cast => crate::chrome::i18n::key("Cast to TV"),
        Control::Pip => crate::chrome::i18n::key("Picture in picture"),
        Control::Guide => crate::chrome::i18n::key("TV guide"),
        Control::Search => crate::chrome::i18n::key("Search"),
        Control::Fullscreen => crate::chrome::i18n::key("Full screen"),
        Control::Timeline => crate::chrome::i18n::key("Playback position"),
    }
}
fn control_id(control: Control) -> &'static str {
    match control {
        Control::Mute => "play-bar-mute",
        Control::Listen => "play-bar-listen",
        Control::Volume => "play-bar-volume",
        Control::More => "play-bar-more",
        Control::Rewind => "play-bar-rewind",
        Control::PlayPause => "play-bar-play-pause",
        Control::Forward => "play-bar-forward",
        Control::Record => "play-bar-record",
        Control::Favorite => "play-bar-favorite",
        Control::Tracks => "play-bar-tracks",
        Control::Details => "play-bar-details",
        Control::Picture => "play-bar-picture",
        Control::Channels => "play-bar-channels",
        Control::Multiview => "play-bar-multiview",
        Control::Cast => "play-bar-cast",
        Control::Pip => "play-bar-pip",
        Control::Guide => "play-bar-guide",
        Control::Search => "play-bar-search",
        Control::Fullscreen => "play-bar-fullscreen",
        Control::Timeline => "play-bar-seek",
    }
}

fn tracks(ui: Ui) -> Option<Track> {
    [Track::Text, Track::Audio, Track::Video]
        .into_iter()
        .find(|k| ui.playback.player.main().tracks.with(|t| t.has_choice(*k)))
}
/// The device is a phone (its form factor, not its window's size).
fn phone_form() -> bool {
    crate::wiring::device::host().form_factor == iptv_core::capabilities::FormFactor::Phone
}

/// A control drawn disabled that still takes the focus and a press, to
/// say why it cannot act: multiview on a TV at the account's limit
/// (#279). Every other control the bar cannot use is left out.
fn explains(ui: Ui, control: Control) -> bool {
    control == Control::Multiview
        && !enabled(ui, control)
        && matches!(
            crate::playback::multiview::offer(ui),
            iptv_presentation::capabilities::MultiviewOffer::Disabled(_)
        )
}

/// Whether the bar shows `control` (enabled, or explaining why not).
fn shown(ui: Ui, control: Control) -> bool {
    enabled(ui, control) || explains(ui, control)
}

fn enabled(ui: Ui, control: Control) -> bool {
    use iptv_core::cast::Command;
    match control {
        Control::Rewind | Control::Forward | Control::Timeline => {
            matches!(ui.playback.playback.get(), Playback::CatchUp { .. })
                || crate::playback::on_demand::seekable(ui)
        }
        Control::Record => {
            crate::playback::player_panels::enabled(ui, iptv_presentation::panels::Control::Record)
        }
        Control::Favorite | Control::Details => ui.playback.playing.get().is_some(),
        Control::Tracks => tracks(ui).is_some(),
        Control::Multiview => {
            !ui.cast.remote.get() && crate::playback::multiview::availability(ui).available()
        }
        Control::Cast => crate::screens::casting::available(ui),
        Control::Pip => !ui.cast.remote.get() && crate::playback::pip::can_enter(ui),
        Control::PlayPause => crate::screens::casting::supports(
            ui,
            if ui.playback.paused.get() {
                Command::Play
            } else {
                Command::Pause
            },
        ),
        Control::Mute => crate::screens::casting::supports(ui, Command::ToggleMute),
        Control::Volume => {
            desktop_controls() && crate::screens::casting::supports(ui, Command::Volume(0))
        }
        Control::Fullscreen => desktop_controls(),
        Control::Listen => crate::wiring::listen::choice(ui) && !crate::wiring::listen::heard(ui),
        _ => true,
    }
}
pub(crate) fn activate(
    ui: Ui,
    control: Control,
    target_window: Option<nsh_core::element::desktop::WindowRef>,
) {
    used(ui);
    if !enabled(ui, control) {
        if explains(ui, control)
            && let Some(reason) =
                untracked(|| crate::playback::multiview::availability(ui)).reason()
        {
            crate::chrome::toast::notify(crate::wiring::capabilities::reason(reason));
        }
        return;
    }
    log::info!("iptv play bar: {}", control_label(control));
    use crate::chrome::chooser::{self, Kind};
    // Keep the current reading or selection position when the user adjusts
    // playback beneath a popup. Opening another surface replaces that popup.
    if matches!(
        control,
        Control::More
            | Control::Tracks
            | Control::Details
            | Control::Picture
            | Control::Channels
            | Control::Pip
            | Control::Multiview
            | Control::Guide
            | Control::Search
            | Control::Volume
    ) {
        if ui.screens.description.with_untracked(Option::is_some) {
            crate::chrome::menu_view::close(ui);
            if control == Control::Details {
                return;
            }
        }
        if let Some(kind) = ui
            .screens
            .chooser
            .with_untracked(|c| c.as_ref().map(|c| c.kind))
        {
            chooser::close(ui);
            if matches!(
                (control, kind),
                (Control::More, Kind::More)
                    | (Control::Picture, Kind::Picture)
                    | (Control::Tracks, Kind::Track(_))
            ) {
                return;
            }
        }
    }
    match control {
        Control::Mute => crate::playback::media::toggle_mute(ui),
        Control::More => chooser::open_more(
            ui,
            layout(ui)
                .more
                .into_iter()
                .filter(|c| shown(ui, *c))
                .collect(),
            target_window,
        ),
        Control::Listen => crate::wiring::listen::listen(ui),
        Control::Volume => chooser::open(ui, Kind::Volume),
        Control::Rewind | Control::Forward => {
            let steps = crate::playback::seek::steps();
            crate::playback::seek::step(
                ui,
                if control == Control::Forward {
                    steps.forward
                } else {
                    -steps.rewind
                },
            );
        }
        Control::PlayPause => crate::playback::player::toggle_pause(ui),
        Control::Record => crate::screens::recordings_view::toggle_current(ui),
        Control::Favorite => {
            if let Some(ch) = ui.playback.playing.get_untracked() {
                crate::guide::favorites::toggle(ui, ch);
            }
        }
        Control::Tracks => {
            if let Some(kind) = untracked(|| tracks(ui)) {
                chooser::open(ui, Kind::Track(kind));
            }
        }
        Control::Details => details(ui),
        Control::Picture => chooser::open(ui, Kind::Picture),
        Control::Channels => {
            crate::screens::channels_view::open(ui, ListMode::Overlay);
            ui.playback.play_bar.update(PlayBar::hide);
        }
        Control::Multiview => {
            ui.playback.play_bar.update(PlayBar::hide);
            crate::playback::multiview::open(ui);
        }
        Control::Cast => crate::screens::casting::open(ui),
        Control::Pip => {
            ui.playback.play_bar.update(PlayBar::hide);
            crate::playback::pip::enter(ui);
        }
        Control::Guide => crate::playback::player_keys::press_quick(ui, QuickAction::Guide),
        Control::Search => crate::screens::search_view::open(ui),
        Control::Fullscreen => {
            let Some(window) = target_window else {
                return;
            };
            let full = window
                .state()
                .is_some_and(|s| s.mode == nsh_core::desktop::WindowMode::FullScreen);
            window.set_mode(if full {
                nsh_core::desktop::WindowMode::Normal
            } else {
                nsh_core::desktop::WindowMode::FullScreen
            });
        }
        Control::Timeline => {
            if crate::playback::seek::pending(ui) {
                crate::playback::seek::commit_now(ui);
            }
            ui.playback
                .play_bar
                .update(|b| b.focus_control(Control::PlayPause, ui.now_ms()));
        }
    }
    used(ui);
}

pub(crate) fn focused(ui: Ui, control: Control) -> bool {
    idiom() == Idiom::Tv
        && ui.keys_shown()
        && ui.playback.play_bar.with(|b| b.focus == Some(control))
}
/// Open the shared bar with Play/pause focused.
pub fn remote(ui: Ui) {
    show(ui, PlayBar::remote);
}
pub fn captures(ui: Ui) -> bool {
    idiom() == Idiom::Tv && shows(ui)
}
/// Returns true for D-pad actions owned by the visible shared bar.
pub fn key(ui: Ui, key: RemoteKey, action: Action, repeat: bool) -> bool {
    if !captures(ui) {
        return false;
    }
    // Context::Controls already maps arrows to moves. Use the physical arrows
    // too so player-specific zap/seek preferences cannot steal bar navigation.
    let direction = match key {
        RemoteKey::Left => Some(Action::MoveLeft),
        RemoteKey::Right => Some(Action::MoveRight),
        RemoteKey::Up => Some(Action::MoveUp),
        RemoteKey::Down => Some(Action::MoveDown),
        _ => None,
    };
    let action = direction.unwrap_or(action);
    if repeat
        && matches!(
            action,
            Action::Select | Action::ShowPanels | Action::ShowControls
        )
    {
        used(ui);
        return true;
    }
    match action {
        Action::MoveLeft | Action::MoveRight => {
            let forward = action == Action::MoveRight;
            if focused(ui, Control::Timeline) && enabled(ui, Control::Timeline) {
                seek_timeline(ui, forward);
            } else {
                let enabled = layout(ui)
                    .row
                    .into_iter()
                    .filter(|c| shown(ui, *c))
                    .collect::<Vec<_>>();
                update(ui, |b, now| {
                    b.move_focus(if forward { 1 } else { -1 }, &enabled, now)
                });
            }
        }
        Action::MoveDown => {
            if enabled(ui, Control::Timeline) {
                update(ui, |b, now| b.focus_control(Control::Timeline, now));
            } else {
                used(ui);
            }
        }
        Action::MoveUp => {
            update(ui, |b, now| b.focus_control(Control::PlayPause, now));
        }
        Action::Select | Action::ShowPanels => {
            if let Some(control) = ui.playback.play_bar.with_untracked(|b| b.focus) {
                activate(ui, control, None);
            }
        }
        Action::ShowControls if key == RemoteKey::Menu => {
            ui.playback.play_bar.update(PlayBar::hide)
        }
        Action::ShowControls => remote(ui),
        _ => return false,
    }
    true
}

/// D-pad and native adjustable actions use the same seek admission and step.
pub(crate) fn seek_timeline(ui: Ui, forward: bool) {
    if enabled(ui, Control::Timeline) {
        let step = crate::playback::seek::steps().timebar;
        crate::playback::seek::step(ui, if forward { step } else { -step });
        used(ui);
    }
}

fn group(ui: Ui, controls: Memo<Layout>, candidates: &'static [Control]) -> View {
    let group = view().flex_row().items_center();
    let group = if candidates == CENTER {
        group.flex_shrink_0().gap(px(TRANSPORT_GAP))
    } else {
        let side = group
            .flex_1()
            .min_w(move || px(controls.with(|l| group_width(candidates, &l.row, target()))))
            .gap(px(BUTTON_GAP));
        if candidates == RIGHT {
            side.justify_end()
        } else {
            side
        }
    };
    group.children(candidates.iter().copied().map(move |c| {
        when(
            move || controls.with(|l| l.row.contains(&c)),
            move || {
                if c == Control::Volume {
                    return volume(ui);
                }
                button(ui, c, target())
            },
        )
    }))
}

/// The center action reopens a completed on-demand title from the beginning.
fn play_again(ui: Ui) -> bool {
    crate::playback::on_demand::active(ui)
        && ui.playback.player.main().finished.get()
        && ui.playback.player.main().native_published()
}

fn icon(ui: Ui, c: Control, window: Option<&nsh_core::element::desktop::WindowRef>) -> Icon {
    match c {
        Control::Mute
            if crate::playback::media::muted(ui.playback)
                || ui.playback.media.volume.with(|v| v.level == 0) =>
        {
            Icon::Muted
        }
        Control::Mute | Control::Volume => Icon::Volume,
        Control::Listen => Icon::Listen,
        Control::Channels => Icon::ChannelList,
        Control::Guide => Icon::Guide,
        Control::Rewind => Icon::Rewind,
        Control::PlayPause if ui.playback.paused.get() || play_again(ui) => Icon::Play,
        Control::PlayPause => Icon::Pause,
        Control::Forward => Icon::Forward,
        Control::Favorite
            if ui
                .playback
                .playing
                .get()
                .is_some_and(|ch| ui.app.favorites.with(|f| f.contains(&ch))) =>
        {
            Icon::StarFilled
        }
        Control::Favorite => Icon::Star,
        Control::Cast => Icon::Cast,
        Control::Pip => Icon::PictureInPicture,
        Control::Multiview => Icon::Multiview,
        Control::Record => Icon::Record,
        Control::More => Icon::More,
        Control::Tracks => Icon::Subtitles,
        Control::Details => Icon::Info,
        Control::Picture => Icon::PictureSize,
        Control::Fullscreen
            if window
                .and_then(|w| w.state())
                .is_some_and(|s| s.mode == nsh_core::desktop::WindowMode::FullScreen) =>
        {
            Icon::ExitFullScreen
        }
        Control::Fullscreen => Icon::FullScreen,
        _ => unreachable!("only toolbar controls have icons"),
    }
}

pub(crate) fn label(ui: Ui, c: Control) -> &'static str {
    match c {
        Control::Record if crate::screens::recordings_view::recording_active(ui) => {
            tr("Stop recording")
        }
        Control::Multiview if crate::playback::multiview::detour(ui) => tr("Back to multiview"),
        Control::PlayPause if play_again(ui) => tr("Play again"),
        _ => tr(control_label(c)),
    }
}

/// The desktop's volume: press or drag along it.
fn volume(ui: Ui) -> View {
    let level = move || f32::from(crate::playback::media::audible_volume(ui.playback)) / 100.0;
    let set = move |e: &TouchEvent| {
        if let Some(p) = e.points.iter().find(|p| p.changed) {
            let at = (p.location.x.0 / VOLUME_WIDTH * 100.0).round() as i32;
            crate::playback::media::set_volume(ui, at);
            used(ui);
        }
    };
    view()
        .test_id(control_id(Control::Volume))
        .w(px(VOLUME_WIDTH))
        .h(px(24.0))
        .justify_center()
        .label(move || tr("Volume"))
        .child(
            view()
                .h(px(4.0))
                .rounded(px(2.0))
                .bg(tint(WHITE, RESTING_ALPHA * 2))
                .child(
                    view()
                        .h_full()
                        .rounded(px(2.0))
                        .w(move || percentage(level()))
                        .bg(rgb(WHITE)),
                ),
        )
        // On top, for touches' places along the whole bar.
        .child(view().absolute().inset_0().on_touch(set))
}

/// Open details for the programme currently on the bar.
fn details(ui: Ui) {
    let (channel, p) = untracked(|| progress::program(ui));
    crate::screens::recordings_view::remember(
        ui,
        Origin {
            channel,
            program_start: p.start,
        },
    );
    ui.screens.description.set(Some(Origin {
        channel,
        program_start: p.start,
    }));
}
