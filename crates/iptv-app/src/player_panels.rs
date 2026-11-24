//! Player panels over full-screen video for the remote (and keys), as
//! available through the info key and custom bindings: a top bar (the playing channel's
//! group; live state, sleep timer, date and time), the info card (`card`:
//! logo, number and name, badges, program, one time line, next
//! program), action cards and the legacy control strip. A channel change
//! shows a compact banner. The card's time line is the
//! only one: a replay's card shows its seek bar there. A pointer (desktop
//! mouse, tablet touch) and the default TV keys use the shared play bar
//! instead (`play_bar.rs`). State lives in `iptv_presentation::panels`; keys in `player_keys.rs`;
//! this draws them and schedules their idle deadline. Panels are views over
//! the video SurfaceView. [`status`] draws the waiting and error
//! states of `player.rs`.

use crate::i18n::{tr, trf};
use std::time::Duration;

use iptv_core::tracks::Kind as Track;
use iptv_presentation::capabilities::Device;
use iptv_presentation::panels::{CONTROLS, Control, QUICK_ACTIONS, QuickAction, Shown};
use iptv_presentation::player_info::date_time;
use iptv_presentation::sound::delay_label;
use iptv_presentation::tracks::{audio_layout_in, summary_in};
use nsh_core::prelude::*;

use crate::fonts::{OSD_BADGE, OSD_DETAIL, OSD_TEXT};
use crate::theme::{FOCUS_ALPHA, RADIUS, RADIUS_LARGE, RESTING_ALPHA, TEXT_SECONDARY_ALPHA};
use crate::{Playback, Ui, ViewMode, tint};

mod card;
mod status;
mod words;

pub use status::status;
pub(crate) use words::{failure_message, waiting_text};

const WHITE: u32 = 0xFFFFFF;
/// Panel fill over video: the theme's panel color, mostly opaque.
const PANEL_ALPHA: u8 = 0xE0;

/// "Name · value", or the name alone.
fn with(name: &str, value: Option<String>) -> String {
    match value {
        Some(v) if !v.is_empty() => format!("{name} · {v}"),
        _ => name.to_owned(),
    }
}

/// The control strip's labels (ours), with the current value where the
/// reference shows one.
fn label(ui: Ui, control: Control) -> String {
    use Control::*;
    let main = ui.player.main();
    let tracks = main.tracks;
    match control {
        Search => tr("Search").to_owned(),
        ChannelList => tr("Channels").to_owned(),
        Recordings => tr("Recordings").to_owned(),
        Record => if crate::recordings_view::recording_active(ui) {
            tr("Stop recording")
        } else {
            tr("Record")
        }
        .to_owned(),
        Multiview if crate::multiview::detour(ui) => tr("Back to multiview").to_owned(),
        Multiview => tr("Multiview").to_owned(),
        PictureInPicture => with(
            tr("Picture in picture"),
            ui.pip
                .with(|pip| {
                    iptv_presentation::capabilities::pip(pip, crate::pip::playing(ui)).reason()
                })
                .map(|reason| crate::capabilities::reason(reason).to_owned()),
        ),
        VideoTracks => {
            let playing = main.size.get().map(|(_, h)| format!("{h}p"));
            with(
                tr("Video"),
                tracks
                    .with(|t| summary_in(t, Track::Video, crate::i18n::language()))
                    .or(playing),
            )
        }
        AudioTracks => {
            let ch = main.format.with(|f| f.channels);
            let layout = (ch > 0).then(|| audio_layout_in(ch, crate::i18n::language()));
            with(
                tr("Audio"),
                tracks
                    .with(|t| summary_in(t, Track::Audio, crate::i18n::language()))
                    .or(layout),
            )
        }
        AudioOffset => with(
            tr("Audio delay"),
            Some(delay_label(
                crate::i18n::language(),
                ui.media.delay_ms.get(),
            )),
        ),
        Captions => with(
            tr("Subtitles"),
            Some(
                tracks
                    .with(|t| summary_in(t, Track::Text, crate::i18n::language()))
                    .unwrap_or_else(|| tr("Off").to_owned()),
            ),
        ),
        Scaling => with(
            tr("Picture size"),
            Some(crate::chooser::picture_name(ui).into()),
        ),
        SleepTimer => with(
            tr("Sleep timer"),
            Some(
                ui.sleep
                    .with(|t| t.remaining_minutes(ui.now_ms()))
                    .map_or(tr("Off").to_owned(), |m| {
                        trf("{minutes} min", &[("minutes", m.into())])
                    }),
            ),
        ),
        Favorite if is_favorite(ui) => tr("★ Favorite").to_owned(),
        Favorite => tr("☆ Favorite").to_owned(),
    }
}

fn is_favorite(ui: Ui) -> bool {
    ui.playing
        .get()
        .is_some_and(|ch| ui.app.favorites.with(|f| f.contains(&ch)))
}

/// Whether the strip shows `control` at all on `device` as it is now.
/// What this host (or this player window) can never do is left out
/// rather than drawn disabled, and the focus moves off a control that
/// leaves (`Panels::offer`); a control shows disabled only while the
/// viewer can change its condition. The features that follow the
/// device's form factor and input are left out here.
pub(crate) fn offered(ui: Ui, device: Device, control: Control) -> bool {
    let main = ui.player_surface == 0;
    match control {
        Control::Record | Control::Recordings => crate::host::HAS_WORKERS && main,
        Control::Search => main,
        Control::Multiview => main && device.host.decoder_slots() > 1,
        // Shown until the host reports, then only where it can: never a TV
        // (`Pip::report`), the iPad or a browser TV (`pip::bind`).
        Control::PictureInPicture => ui.pip.with(|p| !p.reported || p.supported),
        _ => true,
    }
}

/// Whether a control can act now; the rest show disabled.
pub fn enabled(ui: Ui, control: Control) -> bool {
    let choice = |k| ui.player.main().tracks.with(|t| t.has_choice(k));
    match control {
        Control::Multiview => crate::capabilities::multiview(ui).available(),
        Control::Record => {
            crate::host::HAS_WORKERS
                && ui.player_surface == 0
                && ui.playback.get() == crate::Playback::Live
                && !ui.recordings.with(|s| s.busy || s.starting.is_some())
                && ui
                    .playing
                    .get()
                    .is_some_and(crate::recordings_view::available)
        }
        Control::Recordings => crate::host::HAS_WORKERS && ui.player_surface == 0,
        Control::Search => ui.player_surface == 0,
        Control::ChannelList | Control::Favorite | Control::Scaling | Control::SleepTimer => true,
        Control::VideoTracks => choice(Track::Video),
        Control::AudioTracks => choice(Track::Audio),
        Control::Captions => choice(Track::Text),
        Control::AudioOffset => crate::media::AUDIO_DELAY && ui.playing.get().is_some(),
        Control::PictureInPicture => crate::pip::can_enter(ui),
    }
}

/// The panels as shown: hidden outside full screen and in multiview.
fn shown(ui: Ui) -> Shown {
    if ui.mode.get() == ViewMode::Fullscreen && !crate::multiview::is_open(ui) {
        ui.panels.with(|p| p.shown)
    } else {
        Shown::Hidden
    }
}

/// `PointerEvents::None` while hidden: nothing in a hidden layer may say
/// `Auto` or `BoxNone` (NSH's pointer events inherit as CSS's, so either
/// would take presses through it).
fn events(hidden: bool, open: PointerEvents) -> PointerEvents {
    if hidden { PointerEvents::None } else { open }
}

pub(crate) fn bind_auto_hide(ui: Ui) {
    // A screen reader's or switch user moves through the panels at their
    // own pace: while one runs they stay until hidden (WCAG 2.2.1); the
    // play bar does the same (`play_bar::bind`).
    effect(move || {
        let on = crate::device::assistive();
        if untracked(|| ui.panels.with(|p| p.waits_for_assistive() != on)) {
            let now = ui.now_ms();
            untracked(|| ui.panels.update(|p| p.assistive(on, now)));
        }
    });
    // A channel change can publish the deadline from a debounce whose
    // owner is disposed when playback updates the focused target. Bind
    // to the window so the deadline remains armed through that change.
    let deadline = computed(move || ui.panels.with(|p| p.hide_at_ms()));
    effect(move || {
        if let Some(at) = deadline.get() {
            drop(spawn(async move {
                timer_until(ui.app.epoch + Duration::from_millis(at)).await;
                let now = ui.now_ms();
                if ui.panels.with_untracked(|p| p.hide_at_ms()) != Some(at) {
                    return;
                }
                ui.panels.update(|p| {
                    if p.tick(now) {
                        log::info!("iptv panels auto-hide now={now} at={at}");
                    }
                });
            }));
        }
    });
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let shown = computed(move || shown(ui));
    let panel = move || tint(ui.app.theme.with(|t| t.background.panel), PANEL_ALPHA);
    let hidden = move || shown.get() == Shown::Hidden;
    let banner = move || shown.get() == Shown::Banner;
    let card = move || matches!(shown.get(), Shown::Info | Shown::Card);
    let cards = move || shown.get() == Shown::Card;
    let controls = move || shown.get() == Shown::Controls;
    view()
        .absolute()
        .inset_0()
        .pt_safe_area()
        .px(px(32.0))
        .pb(px(24.0))
        .test_id("player-panels")
        .accessibility_elements_hidden(move || hidden() || ui.any_overlay_open())
        .opacity(move || if hidden() { 0.0_f32 } else { 1.0 })
        .pointer_events(move || events(hidden(), PointerEvents::BoxNone))
        // The banner is compact: no top bar over it.
        .child(
            top_bar(ui, panel)
                .test_id("player-top-bar")
                .accessibility_elements_hidden(banner)
                .opacity(move || if banner() { 0.0_f32 } else { 1.0 }),
        )
        .child(view().flex_1().pointer_events(PointerEvents::None))
        .child(
            view()
                .gap(px(10.0))
                .pointer_events(move || events(hidden(), PointerEvents::BoxNone))
                .child(when(banner, move || card::banner(ui, panel)))
                .child(when(
                    move || {
                        card()
                            && ui.player.recorded.with(Option::is_none)
                            && ui.player.vod.with(Option::is_none)
                    },
                    move || card::info_card(ui, panel),
                ))
                .child(when(
                    move || {
                        card()
                            && (ui.player.recorded.with(Option::is_some)
                                || ui.player.vod.with(Option::is_some))
                    },
                    move || {
                        let size = crate::scale::Scale::new();
                        crate::ui_kit::card(ui, size, 620.0)
                            .child(crate::ui_kit::title(size, move || {
                                if let Some(title) = crate::vod_view::title(ui) {
                                    return title;
                                }
                                ui.player.recorded.with(|r| {
                                    r.as_ref()
                                        .map(|(job, _)| job.request.title.clone())
                                        .unwrap_or_default()
                                })
                            }))
                            .child(crate::ui_kit::body(size, move || {
                                ui.player.recorded.with(|r| {
                                    r.as_ref()
                                        .map(|(job, _)| job.request.channel_name.clone())
                                        .unwrap_or_default()
                                })
                            }))
                            .child(card::time_line(ui))
                    },
                ))
                .child(when(cards, move || quick_row(ui, panel)))
                .child(
                    // Mounted while hidden: the strip keeps its scroll.
                    view()
                        .px(px(8.0))
                        .py(move || px(if controls() { 6.0 } else { 0.0 }))
                        .rounded(px(RADIUS_LARGE))
                        .bg(panel)
                        .test_id("player-controls")
                        .accessibility_elements_hidden(move || !controls())
                        .opacity(move || if controls() { 1.0_f32 } else { 0.0 })
                        .pointer_events(move || events(!controls(), PointerEvents::Auto))
                        .child(
                            strip(ui, controls).h(move || px(if controls() { 44.0 } else { 0.0 })),
                        ),
                )
                .child(hint_line(ui, shown)),
        )
}

/// The playing channel's group at the left; live state, sleep timer, date
/// and time at the right.
fn top_bar(ui: Ui, panel: impl Fn() -> Rgba + Copy + 'static) -> View {
    let live = move || !ui.paused.get();
    view()
        .mt(px(16.0))
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .px(px(20.0))
        .py(px(10.0))
        .rounded(px(RADIUS_LARGE))
        .bg(panel)
        .child(
            text(move || {
                ui.guide_data();
                ui.playing.get().map_or(String::new(), card::group_of)
            })
            .flex_1()
            .number_of_lines(1)
            .text_size(px(OSD_TEXT))
            .text_color(rgb(WHITE)),
        )
        .child(
            text(move || {
                if ui.player.vod.with(Option::is_some) {
                    if live() { tr("Video") } else { tr("Paused") }
                } else if ui.player.recorded.with(Option::is_some) {
                    if live() { tr("Recorded") } else { tr("PAUSED") }
                } else {
                    match (ui.playback.get(), live()) {
                        (Playback::CatchUp { .. }, true) => tr("CATCH-UP"),
                        (_, true) => tr("LIVE"),
                        (_, false) => tr("PAUSED"),
                    }
                }
            })
            .px(px(8.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .text_size(px(OSD_BADGE))
            .font_weight(FontWeight::BOLD)
            .text_color(move || {
                if live() {
                    rgb(crate::theme::FOCUSED_TEXT)
                } else {
                    rgb(WHITE)
                }
            })
            .bg(move || {
                if live() {
                    rgb(ui.app.theme.with(|t| t.accent.light))
                } else {
                    tint(WHITE, RESTING_ALPHA * 3)
                }
            }),
        )
        .child(
            // Sleep timer: minutes left as of the last redraw.
            text(move || {
                ui.sleep
                    .with(|t| t.remaining_minutes(ui.now_ms()))
                    .map_or(String::new(), |m| {
                        trf("Sleep {minutes} min", &[("minutes", m.into())])
                    })
            })
            .text_size(px(OSD_DETAIL))
            .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA)),
        )
        .child(
            text(move || {
                ui.guide_data();
                let s = crate::schedule();
                let unix = crate::guide::unix_of(&*s, ui.focus.with(|f| f.now));
                date_time(
                    unix,
                    crate::time::offset_minutes(unix),
                    crate::i18n::clock_style(),
                )
            })
            .text_size(px(OSD_TEXT))
            .text_color(rgb(WHITE)),
        )
}

/// A pill button; `focused` draws the key focus, `dim` a disabled one.
fn pill(
    ui: Ui,
    label: impl Fn() -> String + 'static,
    focused: impl Fn() -> bool + Copy + 'static,
    dim: impl Fn() -> bool + 'static,
    press: impl Fn() + 'static,
) -> Text {
    text(label)
        .px(px(14.0))
        .py(px(8.0))
        .rounded(px(RADIUS))
        .text_size(px(OSD_DETAIL))
        .text_color(move || tint(WHITE, if dim() { 0x60 } else { 0xFF }))
        .border(px(2.0))
        .border_color(move || {
            tint(
                ui.app.theme.with(|t| t.accent.light),
                if focused() { 255 } else { 0 },
            )
        })
        .bg(move || {
            if focused() {
                tint(ui.app.theme.with(|t| t.accent.main), FOCUS_ALPHA)
            } else {
                tint(WHITE, RESTING_ALPHA)
            }
        })
        .on_press(move || {
            ui.touched();
            press();
        })
}

/// The action cards under the info card (a pointer has the play bar,
/// `play_bar.rs`): none focused until Left or Right.
fn quick_row(ui: Ui, panel: impl Fn() -> Rgba + Copy + 'static) -> impl IntoElement {
    view()
        .test_id("player-actions")
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(10.0))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(RADIUS_LARGE))
        .bg(panel)
        .children(QUICK_ACTIONS.iter().enumerate().map(move |(i, action)| {
            let action = *action;
            // Multiview only where the strip offers it (`offered`).
            let offered = move || ui.panels.with(|p| p.offers_quick(action));
            when(offered, move || quick_card(ui, i, action))
        }))
        .self_start()
}

/// Action card `i` of [`QUICK_ACTIONS`].
fn quick_card(ui: Ui, i: usize, action: QuickAction) -> impl IntoElement {
    let focused = move || {
        ui.keys_shown()
            && ui
                .panels
                .with(|p| p.shown == Shown::Card && p.quick == Some(i))
    };
    pill(
        ui,
        move || {
            let detour = crate::multiview::detour(ui);
            words::quick_label(action, is_favorite(ui), detour).to_owned()
        },
        focused,
        move || action == QuickAction::Multiview && ui.playing.get().is_none(),
        move || {
            ui.panels.update(|p| p.quick = Some(i));
            crate::player_keys::press_quick(ui, action);
        },
    )
}

/// What the keys do now, under the panels (keys only).
fn hint_line(ui: Ui, shown: Memo<Shown>) -> impl IntoElement {
    let words = move || {
        if !ui.keys_shown() {
            return "";
        }
        let focused = ui.panels.with(|p| p.quick.is_some());
        let replay = matches!(ui.playback.get(), Playback::CatchUp { .. });
        let seek = replay.then(|| {
            ui.app.settings.get();
            crate::settings::choice(iptv_settings::keys::SEEK_KEYS)
        });
        words::hint(crate::input::PROFILE, shown.get(), focused, seek)
    };
    text(words)
        .px(px(8.0))
        .rounded(px(RADIUS))
        .bg(move || tint(ui.app.theme.with(|t| t.background.panel), PANEL_ALPHA))
        .text_size(px(OSD_DETAIL))
        .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA))
        .accessibility_elements_hidden(move || words().is_empty())
        .opacity(move || if words().is_empty() { 0.0_f32 } else { 1.0 })
        .pointer_events(PointerEvents::None)
}

/// The control strip: a horizontal list of the controls offered here
/// ([`offered`]) that keeps the focused control on screen.
fn strip(ui: Ui, open: impl Fn() -> bool + Copy + 'static) -> VirtualList {
    let list = element_ref();
    // Follows the device's signals (`device.rs`) and the host's picture in
    // picture report: a control that leaves while focused passes the focus
    // to its nearest neighbor shown.
    effect(move || {
        let device = crate::device::current();
        let shown: Vec<Control> = CONTROLS
            .into_iter()
            .filter(|&control| offered(ui, device, control))
            .collect();
        // A resize or a report that leaves the strip as it is does not
        // touch the panels, whose readers would otherwise all run again.
        if ui
            .panels
            .with_untracked(|p| p.controls().eq(shown.iter().copied()))
        {
            return;
        }
        untracked(|| {
            ui.panels
                .update(|p| p.offer(|control| shown.contains(&control)))
        });
    });
    let focus = computed(move || {
        ui.panels
            .with(|p| p.controls().position(|c| c == p.focused()).unwrap_or(0))
    });
    effect(move || {
        let at = focus.get();
        if open() {
            list.scroll_to_index(at as u32, percentage(0.5), px(0.0), true);
        }
    });
    let count = computed(move || ui.panels.with(|p| p.controls().count()));
    virtual_list(count, move |at| {
        // A recycled cell takes another row: read its index each time.
        let control = computed(move || {
            let index = at.index();
            ui.panels.with(|p| p.controls().nth(index))
        });
        let focused = move || {
            ui.keys_shown()
                && ui
                    .panels
                    .with(|p| p.shown == Shown::Controls && control.get() == Some(p.focused()))
        };
        pill(
            ui,
            move || control.get().map(|c| label(ui, c)).unwrap_or_default(),
            focused,
            move || !control.get().is_some_and(|c| enabled(ui, c)),
            move || {
                let Some(control) = control.get_untracked() else {
                    return;
                };
                ui.panels.update(|p| p.focus_control(control));
                crate::player_keys::press_control(ui, control);
            },
        )
        .mr(px(10.0))
    })
    .horizontal()
    .estimated_item_size(px(170.0))
    .element_ref(&list)
}
