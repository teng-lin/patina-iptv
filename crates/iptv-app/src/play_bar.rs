//! The pointer's play bar at the bottom of full-screen video: on the
//! desktop the mouse moving over the video shows it, on a tablet a tap
//! (a click on the desktop) shows it and another hides it; it hides after
//! a few seconds without input, but not while the mouse rests on it
//! (`iptv_core::play_bar`). A TV opens this same bar with long OK or Menu
//! and traverses its controls with the D-pad. Short OK keeps the info card.
//!
//! Left: mute and (desktop) volume, the stream's quality, the channel.
//! Centre: rewind, play/pause, fast-forward (catch-up only; `seek.rs`).
//! Right: favourite, subtitles and tracks, programme details, picture size,
//! channel list, multiview, guide, (desktop) window full screen, frame rate. Under
//! it the programme's progress: where the archive plays (click or drag to
//! seek) or how far the live programme is, with its times. It acts on
//! the full-screen player (surface 0); while multiview's grid is open it
//! does not show (the tiles take the taps), and its state then times the
//! grid's pointer chrome instead (`multiview.rs`). On a multiview detour
//! (a tile watched full screen) its Multiview button goes back to the
//! grid.

use crate::i18n::tr;

pub(crate) mod icons;
mod progress;

use std::time::Duration;

use iptv_core::channel_list::ListMode;
use iptv_core::menu::Origin;
use iptv_core::panels::QuickAction;
use iptv_core::play_bar::{Control, PlayBar};
use iptv_core::tracks::{Kind as Track, fps_label, resolution_class};
use iptv_nav::{Action, RemoteKey};
use nsh_core::platform::Idiom;
use nsh_core::prelude::*;
use nsh_core::{Checked, TouchEvent};

use crate::theme::{RADIUS_LARGE, RESTING_ALPHA, TEXT_DISABLED_ALPHA};
use crate::theme::{TEXT_SECONDARY_ALPHA, TEXT_SIZE};
use crate::{Playback, Ui, ViewMode, channel_name, tint};
use icons::Icon;

const WHITE: u32 = 0xFFFFFF;
const PANEL_ALPHA: u8 = 0xE0;
/// The frame rate's room at the right end ("120 fps").
const FPS_W: f32 = 48.0;
/// Volume and window full screen are the app's on the desktop only
/// (Linux and macOS; Android keeps the system volume).
const DESKTOP: bool = crate::desktop::DESKTOP;

/// Whether the device has a pointer for the bar: anything but a TV (its
/// remote has the panels) and a terminal.
pub fn available() -> bool {
    idiom() != Idiom::Terminal
}
/// Pointer gestures retain their existing TV info-card behavior.
pub fn pointer_available() -> bool {
    !matches!(idiom(), Idiom::Tv | Idiom::Terminal)
}

/// A button's side: a finger's target on touch devices, smaller under a
/// mouse.
fn target() -> f32 {
    if idiom() == Idiom::Desktop {
        40.0
    } else {
        48.0
    }
}

/// Whether the bar is on screen.
pub fn shows(ui: Ui) -> bool {
    ui.mode.get() == ViewMode::Fullscreen
        && (idiom() == Idiom::Tv || !ui.keys_shown())
        && ui.play_bar.with(|b| b.shown)
        && !crate::multiview::is_open(ui)
        && available()
}

/// Leaving full screen or a key hides the bar; it stays while a popup
/// over the video (a chooser, the programme details) is open.
pub fn bind(ui: Ui) {
    effect(move || {
        let away =
            ui.mode.get() != ViewMode::Fullscreen || (idiom() != Idiom::Tv && ui.keys_shown());
        if away && ui.play_bar.with_untracked(|b| b.shown) {
            ui.play_bar.update(PlayBar::hide);
        }
    });
    let popup =
        computed(move || ui.chooser.with(Option::is_some) || ui.description.with(Option::is_some));
    effect(move || {
        let open = popup.get();
        if untracked(|| ui.play_bar.with(|b| b.held != open)) {
            untracked(|| update(ui, |b, now| b.hold(open, now)));
        }
    });
}

/// The bar's height above the window's bottom (with its margin): popups
/// over the video keep clear of it.
pub fn height() -> f32 {
    let row = target() + 8.0;
    // Panel padding, the row, the gap, the progress line, the margin.
    6.0 + row + 4.0 + 20.0 + 10.0 + 16.0
}

/// Changes the bar's state and (re)schedules its hiding.
fn update(ui: Ui, f: impl FnOnce(&mut PlayBar, u64)) {
    // An auxiliary control (Guide, for example) can close its window and
    // dispose this local presentation before the press handler finishes.
    if untracked(|| ui.play_bar.try_get()).is_none() {
        return;
    }
    let now = ui.now_ms();
    ui.play_bar.update(|b| f(b, now));
    let Some(at) = ui.play_bar.with_untracked(PlayBar::hide_at_ms) else {
        return;
    };
    let _task = set_timeout(Duration::from_millis(at.saturating_sub(now)), move || {
        let now = ui.now_ms();
        // Later input moved the deadline: then this does nothing.
        if untracked(|| ui.play_bar.try_get())
            .is_some_and(|b| b.hide_at_ms().is_some_and(|at| now >= at))
        {
            ui.play_bar.update(|b| {
                b.tick(now);
            });
        }
    });
}

/// The bar replaces the remote's panels while a pointer is used.
fn show(ui: Ui, f: impl FnOnce(&mut PlayBar, u64)) {
    if ui.panels.with_untracked(|p| p.shown) != iptv_core::panels::Shown::Hidden {
        ui.panels.update(|p| p.hide());
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

/// A control of the bar (or of multiview's pointer chrome) acted.
pub(crate) fn used(ui: Ui) {
    update(ui, PlayBar::used);
}

/// The mouse came over the bar (or multiview's toolbar), or left it.
pub(crate) fn hover(ui: Ui, over: bool) {
    update(ui, |b, now| b.hover(over, now));
}

pub fn layer(ui: Ui) -> impl IntoElement {
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
                            crate::overlays::Overlay::Chooser
                                | crate::overlays::Overlay::Description
                        )
                    })
                })
                .pointer_events(PointerEvents::BoxNone)
                .child(when(move || ui.back_controls(), move || back(ui)))
                .child(bar(ui))
        },
    )
}

/// A tablet's way back from full-screen video, where an iPad's players
/// keep it: a round ‹ top left, with the bar. It leaves at once for the
/// guide, or multiview's grid from a tile watched full screen (its
/// accessibility label says which; a remote's Back and an iPad's swipe
/// from the left edge hide the bar first).
fn back(ui: Ui) -> impl IntoElement {
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
            crate::ui_kit::round_back(
                side,
                0.0,
                back_bg(ui),
                move || {
                    if crate::multiview::detour(ui) {
                        tr("Multiview")
                    } else if ui.player.recorded.with(Option::is_some) {
                        tr("Recordings")
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
                    ui.panels.update(|p| p.hide());
                    ui.play_bar.update(PlayBar::hide);
                    crate::go_back(ui);
                },
            )
            .test_id("play-bar-back"),
        )
}

/// The round ‹'s colour over video (the player's, the guide's over its
/// preview): the bar's panel.
pub(crate) fn back_bg(ui: Ui) -> impl Fn() -> Rgba + Copy + 'static {
    move || tint(ui.theme.with(|t| t.background.panel), PANEL_ALPHA)
}

fn bar(ui: Ui) -> impl IntoElement {
    let panel = move || tint(ui.theme.with(|t| t.background.panel), PANEL_ALPHA);
    view()
        .absolute()
        .left(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .px(px(24.0))
        .pb_safe_area_plus(px(16.0))
        // Only the panel takes presses: around it they reach the video.
        .pointer_events(PointerEvents::BoxNone)
        .child(
            view()
                .px(px(12.0))
                .pt(px(6.0))
                .pb(px(10.0))
                .gap(px(4.0))
                .rounded(px(RADIUS_LARGE))
                .bg(panel)
                .on_mouse_enter(move |_: &MouseEvent| hover(ui, true))
                .on_mouse_leave(move |_: &MouseEvent| hover(ui, false))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(left(ui))
                        .child(centre(ui))
                        .child(right(ui)),
                )
                .child(progress::line(ui)),
        )
}

/// An icon button shared by touch, pointer and D-pad. Admission and actions
/// are resolved from the same stable identity on every input path.
fn button(ui: Ui, control: Control, icon: impl Fn() -> Icon + 'static, size: f32) -> View {
    let target_window =
        (control == Control::Fullscreen).then(nsh_core::element::desktop::current_window);
    view()
        .role(AccessibilityRole::Button)
        .accessible(true)
        .test_id(control_id(control))
        .accessibility_state(move || AccessibilityState {
            disabled: !enabled(ui, control),
            selected: focused(ui, control),
            pressed: (control == Control::Record).then(|| {
                if crate::recordings_view::recording_active(ui) {
                    Checked::True
                } else {
                    Checked::False
                }
            }),
            ..Default::default()
        })
        .flex_shrink_0()
        .w(px(size))
        .h(px(size))
        .rounded(px(size / 2.0))
        .border(px(2.0))
        .border_color(move || {
            tint(
                ui.theme.with(|t| t.accent.light),
                if focused(ui, control) { 255 } else { 0 },
            )
        })
        .items_center()
        .justify_center()
        .label(move || {
            if control == Control::Record && crate::recordings_view::recording_active(ui) {
                tr("Stop recording")
            } else {
                tr(control_label(control))
            }
        })
        .hover(|s| s.bg(tint(WHITE, RESTING_ALPHA * 2)))
        .on_press(move || {
            ui.touched();
            activate(ui, control, target_window.clone());
        })
        .child(
            svg(move || icons::src(icon()))
                .size(px(size * 0.55))
                .text_color(move || {
                    tint(
                        if control == Control::Record
                            && crate::recordings_view::recording_active(ui)
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
        )
}

fn control_label(control: Control) -> &'static str {
    match control {
        Control::Mute => crate::i18n::key("Mute"),
        Control::Rewind => crate::i18n::key("Rewind"),
        Control::PlayPause => crate::i18n::key("Play or pause"),
        Control::Forward => crate::i18n::key("Fast-forward"),
        Control::Record => crate::i18n::key("Record"),
        Control::Favourite => crate::i18n::key("Favourite"),
        Control::Tracks => crate::i18n::key("Subtitles and tracks"),
        Control::Details => crate::i18n::key("Programme details"),
        Control::Picture => crate::i18n::key("Picture size"),
        Control::Channels => crate::i18n::key("Channel list"),
        Control::Multiview => crate::i18n::key("Multiview"),
        Control::Pip => crate::i18n::key("Picture in picture"),
        Control::Guide => crate::i18n::key("TV guide"),
        Control::Fullscreen => crate::i18n::key("Full screen"),
        Control::Timeline => crate::i18n::key("Playback position"),
    }
}
fn control_id(control: Control) -> &'static str {
    match control {
        Control::Mute => "play-bar-mute",
        Control::Rewind => "play-bar-rewind",
        Control::PlayPause => "play-bar-play-pause",
        Control::Forward => "play-bar-forward",
        Control::Record => "play-bar-record",
        Control::Favourite => "play-bar-favourite",
        Control::Tracks => "play-bar-tracks",
        Control::Details => "play-bar-details",
        Control::Picture => "play-bar-picture",
        Control::Channels => "play-bar-channels",
        Control::Multiview => "play-bar-multiview",
        Control::Pip => "play-bar-pip",
        Control::Guide => "play-bar-guide",
        Control::Fullscreen => "play-bar-fullscreen",
        Control::Timeline => "play-bar-seek",
    }
}

fn tracks(ui: Ui) -> Option<Track> {
    [Track::Text, Track::Audio, Track::Video]
        .into_iter()
        .find(|k| ui.player.main().tracks.with(|t| t.has_choice(*k)))
}
fn enabled(ui: Ui, control: Control) -> bool {
    match control {
        Control::Rewind | Control::Forward | Control::Timeline => {
            matches!(ui.playback.get(), Playback::CatchUp { .. }) || crate::on_demand::seekable(ui)
        }
        Control::Record => crate::player_panels::enabled(ui, iptv_core::panels::Control::Record),
        Control::Favourite | Control::Details => ui.playing.get().is_some(),
        Control::Tracks => tracks(ui).is_some(),
        Control::Multiview => crate::capabilities::multiview(ui).available(),
        Control::Pip => crate::pip::can_enter(ui),
        Control::Fullscreen => DESKTOP && idiom() != Idiom::Tv,
        _ => true,
    }
}
fn activate(
    ui: Ui,
    control: Control,
    target_window: Option<nsh_core::element::desktop::WindowRef>,
) {
    used(ui);
    if !enabled(ui, control) {
        return;
    }
    log::info!("iptv play bar: {}", control_label(control));
    use crate::chooser::{self, Kind};
    match control {
        Control::Mute => crate::media::toggle_mute(ui),
        Control::Rewind | Control::Forward => {
            let steps = crate::seek::steps();
            crate::seek::step(
                ui,
                if control == Control::Forward {
                    steps.forward
                } else {
                    -steps.rewind
                },
            );
        }
        Control::PlayPause => crate::player::toggle_pause(ui),
        Control::Record => crate::recordings_view::toggle_current(ui),
        Control::Favourite => {
            if let Some(ch) = ui.playing.get_untracked() {
                crate::favorites::toggle(ui, ch);
            }
        }
        Control::Tracks => {
            let open = ui
                .chooser
                .with_untracked(|c| c.as_ref().is_some_and(|c| matches!(c.kind, Kind::Track(_))));
            if open {
                chooser::close(ui);
            } else if let Some(kind) = untracked(|| tracks(ui)) {
                chooser::open(ui, Kind::Track(kind));
            }
        }
        Control::Details => details(ui),
        Control::Picture => chooser::toggle(ui, Kind::Picture),
        Control::Channels => {
            crate::channels_view::open(ui, ListMode::Overlay);
            ui.play_bar.update(PlayBar::hide);
        }
        Control::Multiview => {
            ui.play_bar.update(PlayBar::hide);
            crate::multiview::open(ui);
        }
        Control::Pip => {
            ui.play_bar.update(PlayBar::hide);
            crate::pip::enter(ui);
        }
        Control::Guide => crate::player_keys::press_quick(ui, QuickAction::Guide),
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
            if crate::seek::pending(ui) {
                crate::seek::commit_now(ui);
            }
            ui.play_bar
                .update(|b| b.focus_control(Control::PlayPause, ui.now_ms()));
        }
    }
    used(ui);
}

pub(super) fn focused(ui: Ui, control: Control) -> bool {
    idiom() == Idiom::Tv && ui.keys_shown() && ui.play_bar.with(|b| b.focus == Some(control))
}
/// Explicit TV long OK/Menu: keep short OK's info-card semantics intact.
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
                let enabled = [
                    Control::Mute,
                    Control::Rewind,
                    Control::PlayPause,
                    Control::Forward,
                    Control::Record,
                    Control::Favourite,
                    Control::Tracks,
                    Control::Details,
                    Control::Picture,
                    Control::Channels,
                    Control::Multiview,
                    Control::Pip,
                    Control::Guide,
                ]
                .into_iter()
                .filter(|c| (*c != Control::Pip || crate::pip::supported(ui)) && enabled(ui, *c))
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
            if let Some(control) = ui.play_bar.with_untracked(|b| b.focus) {
                activate(ui, control, None);
            }
        }
        Action::ShowControls if key == RemoteKey::Menu => ui.play_bar.update(PlayBar::hide),
        Action::ShowControls => remote(ui),
        _ => return false,
    }
    true
}

/// D-pad and native adjustable actions use the same seek admission and step.
pub(super) fn seek_timeline(ui: Ui, forward: bool) {
    if enabled(ui, Control::Timeline) {
        let step = crate::seek::steps().timebar;
        crate::seek::step(ui, if forward { step } else { -step });
        used(ui);
    }
}

fn badge(value: impl Fn() -> Option<String> + Copy + 'static) -> impl IntoElement {
    text(move || value().unwrap_or_default())
        .px(move || px(if value().is_some() { 6.0 } else { 0.0 }))
        .py(px(1.0))
        .rounded(px(4.0))
        .text_size(px(12.0))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(WHITE))
        .bg(move || {
            tint(
                WHITE,
                if value().is_some() {
                    RESTING_ALPHA * 3
                } else {
                    0
                },
            )
        })
}

/// Mute, volume, quality, channel.
fn left(ui: Ui) -> impl IntoElement {
    let size = target();
    let muted = move || crate::media::muted(ui) || ui.media.volume.with(|v| v.level == 0);
    let height = move || {
        ui.player
            .main()
            .size
            .get()
            .map(|(_, h)| h)
            .filter(|h| *h > 0)
    };
    let lines = move || height().map(|h| format!("{h}p"));
    let class = move || {
        ui.player
            .main()
            .size
            .get()
            .filter(|(w, h)| *w > 0 && *h > 0)
            .map(|(w, h)| resolution_class(w, h).to_owned())
    };
    // It gives way first on a narrow window: the channel's name shortens,
    // then the rest is cut, never the buttons beside it.
    view()
        .flex_1()
        .min_w(px(size))
        .overflow_hidden()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(button(
            ui,
            Control::Mute,
            move || if muted() { Icon::Muted } else { Icon::Volume },
            size,
        ))
        .child(when(|| DESKTOP && idiom() != Idiom::Tv, move || volume(ui)))
        .child(badge(lines))
        .child(badge(class))
        .child(
            text(move || {
                ui.data_version.get();
                crate::on_demand::title(ui)
                    .or_else(|| ui.playing.get().map(channel_name))
                    .unwrap_or_default()
            })
            .flex_shrink(1.0)
            .number_of_lines(1)
            .text_size(px(TEXT_SIZE))
            .text_color(rgb(WHITE)),
        )
}

/// The desktop's volume: press or drag along it.
fn volume(ui: Ui) -> impl IntoElement {
    const W: f32 = 96.0;
    let level = move || {
        if crate::media::muted(ui) {
            0.0
        } else {
            f32::from(ui.media.volume.with(|v| v.level)) / 100.0
        }
    };
    let set = move |e: &TouchEvent| {
        if let Some(p) = e.points.iter().find(|p| p.changed) {
            let at = (p.location.x.0 / W * 100.0).round() as i32;
            crate::media::set_volume(ui, at);
            used(ui);
        }
    };
    view()
        .w(px(W))
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

/// Rewind, play/pause, fast-forward.
fn centre(ui: Ui) -> impl IntoElement {
    let size = target();
    view()
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(button(ui, Control::Rewind, || Icon::Rewind, size))
        .child(button(
            ui,
            Control::PlayPause,
            move || {
                if ui.paused.get() {
                    Icon::Play
                } else {
                    Icon::Pause
                }
            },
            size + 8.0,
        ))
        .child(button(ui, Control::Forward, || Icon::Forward, size))
}

/// Favourite, tracks, details, picture size, channels, multiview, guide,
/// window full screen, frame rate.
fn right(ui: Ui) -> impl IntoElement {
    let icon_window = nsh_core::element::desktop::current_window();
    let size = target();
    let favourite = move || {
        ui.playing
            .get()
            .is_some_and(|ch| ui.favorites.with(|f| f.contains(&ch)))
    };
    // Reserve space for every control, including recording and optional
    // native window controls, so none overlap the transport hit targets.
    let buttons = move || {
        8.0 + if DESKTOP && idiom() != Idiom::Tv {
            1.0
        } else {
            0.0
        } + if crate::pip::supported(ui) { 1.0 } else { 0.0 }
    };
    view()
        .flex_1()
        .min_w(move || px(buttons() * size + (buttons() - 1.0) * 2.0 + FPS_W + 6.0))
        .flex_row()
        .items_center()
        .justify_end()
        .gap(px(2.0))
        .child(button(ui, Control::Record, || Icon::Record, size))
        .child(button(
            ui,
            Control::Favourite,
            move || {
                if favourite() {
                    Icon::StarFilled
                } else {
                    Icon::Star
                }
            },
            size,
        ))
        .child(button(ui, Control::Tracks, || Icon::Subtitles, size))
        .child(button(ui, Control::Details, || Icon::Info, size))
        .child(button(ui, Control::Picture, || Icon::PictureSize, size))
        .child(button(ui, Control::Channels, || Icon::ChannelList, size))
        .child(
            button(ui, Control::Multiview, || Icon::Multiview, size)
                // On a detour it returns to the grid.
                .label(move || {
                    if crate::multiview::detour(ui) {
                        tr("Back to multiview")
                    } else {
                        tr("Multiview")
                    }
                }),
        )
        // Picture in picture, where the device has the window (the
        // control strip has it too, but touch has no way to that strip).
        .child(when(
            move || crate::pip::supported(ui),
            move || button(ui, Control::Pip, || Icon::PictureInPicture, size),
        ))
        .child(button(ui, Control::Guide, || Icon::Guide, size))
        .child(when(
            || DESKTOP && idiom() != Idiom::Tv,
            move || {
                let icon_window = icon_window.clone();
                button(
                    ui,
                    Control::Fullscreen,
                    move || {
                        if icon_window
                            .state()
                            .is_some_and(|s| s.mode == nsh_core::desktop::WindowMode::FullScreen)
                        {
                            Icon::ExitFullScreen
                        } else {
                            Icon::FullScreen
                        }
                    },
                    size,
                )
            },
        ))
        .child(
            text(move || fps_label(ui.player.main().format.with(|f| f.fps)).unwrap_or_default())
                .w(px(FPS_W))
                .text_right()
                .ml(px(6.0))
                .text_size(px(12.0))
                .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA)),
        )
}

/// The playing programme's details (the description popover), or closes
/// them when they show.
fn details(ui: Ui) {
    if ui.description.with_untracked(Option::is_some) {
        crate::menu_view::close(ui);
        return;
    }
    let (channel, p) = untracked(|| progress::programme(ui));
    crate::recordings_view::remember(
        ui,
        Origin {
            channel,
            program_start: p.start,
        },
    );
    ui.description.set(Some(Origin {
        channel,
        program_start: p.start,
    }));
}
