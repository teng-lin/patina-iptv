//! The pointer's play bar at the bottom of full-screen video: on the
//! desktop the mouse moving over the video shows it, on a tablet a tap
//! (a click on the desktop) shows it and another hides it; it hides after
//! a few seconds without input, but not while the mouse rests on it
//! (`iptv_core::play_bar`). A TV never shows it: its remote keeps the info
//! card and the control row (`player_panels.rs`), and any key hides the
//! bar.
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
use iptv_core::play_bar::PlayBar;
use iptv_core::tracks::{Kind as Track, fps_label, resolution_class};
use nsh_core::TouchEvent;
use nsh_core::platform::Idiom;
use nsh_core::prelude::*;

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
        && !ui.keys_shown()
        && ui.play_bar.with(|b| b.shown)
        && !crate::multiview::is_open(ui)
        && available()
}

/// Leaving full screen or a key hides the bar; it stays while a popup
/// over the video (a chooser, the programme details) is open.
pub fn bind(ui: Ui) {
    effect(move || {
        let away = ui.mode.get() != ViewMode::Fullscreen || ui.keys_shown();
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
    let now = ui.now_ms();
    ui.play_bar.update(|b| f(b, now));
    let Some(at) = ui.play_bar.with_untracked(PlayBar::hide_at_ms) else {
        return;
    };
    let _task = set_timeout(Duration::from_millis(at.saturating_sub(now)), move || {
        let now = ui.now_ms();
        // Later input moved the deadline: then this does nothing.
        if ui
            .play_bar
            .with_untracked(|b| b.hide_at_ms().is_some_and(|at| now >= at))
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
    if !available() {
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
    let side = target();
    view()
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .pl_safe_area_plus(px(20.0))
        .pt_safe_area_plus(px(16.0))
        // Only the button takes presses: around it they reach the video.
        .pointer_events(PointerEvents::BoxNone)
        .child(crate::ui_kit::round_back(
            side,
            0.0,
            back_bg(ui),
            move || {
                if crate::multiview::detour(ui) {
                    tr("Multiview")
                } else {
                    tr("Guide")
                }
            },
            move || {
                ui.touched();
                log::info!("iptv play bar: back");
                ui.panels.update(|p| p.hide());
                ui.play_bar.update(PlayBar::hide);
                crate::go_back(ui);
            },
        ))
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

/// An icon button; `enabled` dims it and keeps it from acting.
fn button(
    ui: Ui,
    name: &'static str,
    icon: impl Fn() -> Icon + 'static,
    enabled: impl Fn() -> bool + Copy + 'static,
    size: f32,
    act: impl Fn() + 'static,
) -> View {
    view()
        .role(AccessibilityRole::Button)
        .accessibility_state(move || AccessibilityState {
            disabled: !enabled(),
            ..Default::default()
        })
        .w(px(size))
        .h(px(size))
        .rounded(px(size / 2.0))
        .items_center()
        .justify_center()
        .label(move || tr(name))
        .hover(|s| s.bg(tint(WHITE, RESTING_ALPHA * 2)))
        .on_press(move || {
            ui.touched();
            log::info!("iptv play bar: {name}");
            if enabled() {
                act();
            }
            used(ui);
        })
        .child(
            svg(move || icons::src(icon()))
                .size(px(size * 0.55))
                .text_color(move || {
                    tint(WHITE, if enabled() { 0xFF } else { TEXT_DISABLED_ALPHA })
                }),
        )
}

fn always() -> bool {
    true
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
    let muted = move || ui.media.volume.with(|v| v.muted || v.effective() == 0);
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
        .min_w(px(0.0))
        .overflow_hidden()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(button(
            ui,
            "Mute",
            move || if muted() { Icon::Muted } else { Icon::Volume },
            always,
            size,
            move || crate::media::toggle_mute(ui),
        ))
        .child(when(|| DESKTOP, move || volume(ui)))
        .child(badge(lines))
        .child(badge(class))
        .child(
            text(move || {
                ui.data_version.get();
                ui.playing.get().map(channel_name).unwrap_or_default()
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
    let level = move || f32::from(ui.media.volume.with(|v| v.effective())) / 100.0;
    let set = move |e: &TouchEvent| {
        if let Some(p) = e.points.iter().find(|p| p.changed) {
            let at = (p.location.x.0 / W * 100.0).round() as i32;
            ui.media.volume.update(|v| v.set(at));
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
    let catch_up = move || matches!(ui.playback.get(), Playback::CatchUp { .. });
    let seek = move |forward: bool| {
        let steps = crate::seek::steps();
        let delta = if forward {
            steps.forward
        } else {
            -steps.rewind
        };
        crate::seek::step(ui, delta);
    };
    view()
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(button(
            ui,
            "Rewind",
            || Icon::Rewind,
            catch_up,
            size,
            move || seek(false),
        ))
        .child(button(
            ui,
            "Play or pause",
            move || {
                if ui.paused.get() {
                    Icon::Play
                } else {
                    Icon::Pause
                }
            },
            always,
            size + 8.0,
            move || ui.paused.update(|p| *p = !*p),
        ))
        .child(button(
            ui,
            "Fast-forward",
            || Icon::Forward,
            catch_up,
            size,
            move || seek(true),
        ))
}

/// Favourite, tracks, details, picture size, channels, multiview, guide,
/// window full screen, frame rate.
fn right(ui: Ui) -> impl IntoElement {
    use crate::chooser::{self, Kind};
    let size = target();
    let favourite = move || {
        ui.playing
            .get()
            .is_some_and(|ch| ui.favorites.with(|f| f.contains(&ch)))
    };
    let tracks = move || {
        [Track::Text, Track::Audio, Track::Video]
            .into_iter()
            .find(|k| ui.player.main().tracks.with(|t| t.has_choice(*k)))
    };
    let playing = move || ui.playing.get().is_some();
    // Never narrower than its buttons (seven on the desktop, six
    // elsewhere) and the frame rate: they must not slide under the centre.
    let buttons = if DESKTOP { 7.0 } else { 6.0 };
    view()
        .flex_1()
        .min_w(px(buttons * size + (buttons - 1.0) * 2.0 + FPS_W + 6.0))
        .flex_row()
        .items_center()
        .justify_end()
        .gap(px(2.0))
        .child(button(
            ui,
            "Favourite",
            move || {
                if favourite() {
                    Icon::StarFilled
                } else {
                    Icon::Star
                }
            },
            playing,
            size,
            move || {
                if let Some(ch) = ui.playing.get_untracked() {
                    crate::favorites::toggle(ui, ch);
                }
            },
        ))
        .child(button(
            ui,
            "Subtitles and tracks",
            || Icon::Subtitles,
            move || tracks().is_some(),
            size,
            move || {
                let open = ui.chooser.with_untracked(|c| {
                    c.as_ref().is_some_and(|c| matches!(c.kind, Kind::Track(_)))
                });
                if open {
                    chooser::close(ui);
                } else if let Some(kind) = untracked(tracks) {
                    chooser::open(ui, Kind::Track(kind));
                }
            },
        ))
        .child(button(
            ui,
            "Programme details",
            || Icon::Info,
            playing,
            size,
            move || details(ui),
        ))
        .child(button(
            ui,
            "Picture size",
            || Icon::PictureSize,
            always,
            size,
            move || chooser::toggle(ui, Kind::Picture),
        ))
        .child(button(
            ui,
            "Channel list",
            || Icon::ChannelList,
            always,
            size,
            move || {
                crate::channels_view::open(ui, ListMode::Overlay);
                ui.play_bar.update(PlayBar::hide);
            },
        ))
        .child(
            button(
                ui,
                "Multiview",
                || Icon::Multiview,
                playing,
                size,
                move || {
                    ui.play_bar.update(PlayBar::hide);
                    crate::multiview::open(ui);
                },
            )
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
            move || {
                button(
                    ui,
                    "Picture in picture",
                    || Icon::PictureInPicture,
                    move || crate::pip::can_enter(ui),
                    size,
                    move || {
                        ui.play_bar.update(PlayBar::hide);
                        crate::pip::enter(ui);
                    },
                )
            },
        ))
        .child(button(
            ui,
            "TV guide",
            || Icon::Guide,
            always,
            size,
            move || crate::player_keys::press_quick(ui, QuickAction::Guide),
        ))
        .child(when(
            || DESKTOP,
            move || {
                button(
                    ui,
                    "Full screen",
                    || {
                        if crate::desktop::fullscreen() {
                            Icon::ExitFullScreen
                        } else {
                            Icon::FullScreen
                        }
                    },
                    always,
                    size,
                    crate::desktop::toggle_fullscreen,
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
    ui.description.set(Some(Origin {
        channel,
        program_start: p.start,
    }));
}
