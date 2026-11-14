//! Player panels over full-screen video for the remote (and keys), as
//! observed in the reference UX: OK shows a top bar (the playing channel's
//! group; live state, sleep timer, date and time), the info card (`card`:
//! logo, number and name, badges, programme, one time line, next
//! programme) and the action cards beneath it; Long OK or Menu the control
//! strip; a channel change a compact banner. The card's time line is the
//! only one: a replay's card shows its seek bar there. A pointer (desktop
//! mouse, tablet touch) has the play bar instead (`play_bar.rs`, never on
//! a TV). State lives in `iptv_core::panels`; keys in `player_keys.rs`;
//! this only draws them. Panels are views over the video SurfaceView
//! (never alpha on the video). [`status`] draws the waiting and error
//! states of `player.rs`.

use crate::i18n::{tr, trf};

use iptv_core::panels::{CONTROLS, Control, QUICK_ACTIONS, Shown};
use iptv_core::player_info::date_time;
use iptv_core::sound::delay_label;
use iptv_core::tracks::{Kind as Track, audio_layout};
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
/// Panel fill over video: the theme's panel colour, mostly opaque.
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
        Multiview if crate::multiview::detour(ui) => tr("Back to multiview").to_owned(),
        Multiview => tr("Multiview").to_owned(),
        PictureInPicture => tr("Picture in picture").to_owned(),
        VideoTracks => {
            let playing = main.size.get().map(|(_, h)| format!("{h}p"));
            with(
                tr("Video"),
                tracks.with(|t| t.summary(Track::Video)).or(playing),
            )
        }
        AudioTracks => {
            let ch = main.format.with(|f| f.channels);
            let layout = (ch > 0).then(|| audio_layout(ch));
            with(
                tr("Audio"),
                tracks.with(|t| t.summary(Track::Audio)).or(layout),
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
                    .with(|t| t.summary(Track::Text))
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
        Favorite if is_favorite(ui) => tr("★ Favourite").to_owned(),
        Favorite => tr("☆ Favourite").to_owned(),
        ChannelOptions => tr("Channel options").to_owned(),
        Settings => tr("Settings").to_owned(),
    }
}

fn is_favorite(ui: Ui) -> bool {
    ui.playing
        .get()
        .is_some_and(|ch| ui.favorites.with(|f| f.contains(&ch)))
}

/// Whether a control can act now; the rest show disabled.
pub fn enabled(ui: Ui, control: Control) -> bool {
    let choice = |k| ui.player.main().tracks.with(|t| t.has_choice(k));
    match control {
        Control::Multiview => ui.playing.get().is_some(),
        Control::Search
        | Control::ChannelList
        | Control::Favorite
        | Control::Scaling
        | Control::SleepTimer => true,
        Control::VideoTracks => choice(Track::Video),
        Control::AudioTracks => choice(Track::Audio),
        Control::Captions => choice(Track::Text),
        Control::AudioOffset => crate::media::AUDIO_DELAY && ui.playing.get().is_some(),
        Control::PictureInPicture => crate::pip::can_enter(ui),
        Control::Recordings | Control::ChannelOptions | Control::Settings => false,
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

pub fn layer(ui: Ui) -> impl IntoElement {
    let shown = computed(move || shown(ui));
    let panel = move || tint(ui.theme.with(|t| t.background.panel), PANEL_ALPHA);
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
        .opacity(move || if hidden() { 0.0_f32 } else { 1.0 })
        .pointer_events(move || events(hidden(), PointerEvents::BoxNone))
        // The banner is compact: no top bar over it.
        .child(top_bar(ui, panel).opacity(move || if banner() { 0.0_f32 } else { 1.0 }))
        .child(view().flex_1().pointer_events(PointerEvents::None))
        .child(
            view()
                .gap(px(10.0))
                .pointer_events(move || events(hidden(), PointerEvents::BoxNone))
                .child(when(banner, move || card::banner(ui, panel)))
                .child(when(card, move || card::info_card(ui, panel)))
                .child(when(cards, move || quick_row(ui, panel)))
                .child(
                    // Mounted while hidden: the strip keeps its scroll.
                    view()
                        .px(px(8.0))
                        .py(move || px(if controls() { 6.0 } else { 0.0 }))
                        .rounded(px(RADIUS_LARGE))
                        .bg(panel)
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
            text(move || match (ui.playback.get(), live()) {
                (Playback::CatchUp { .. }, true) => "CATCH-UP",
                (_, true) => "LIVE",
                (_, false) => "PAUSED",
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
                    rgb(ui.theme.with(|t| t.accent.light))
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
                ui.theme.with(|t| t.accent.light),
                if focused() { 255 } else { 0 },
            )
        })
        .bg(move || {
            if focused() {
                tint(ui.theme.with(|t| t.accent.main), FOCUS_ALPHA)
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
                move || {
                    action == iptv_core::panels::QuickAction::Multiview
                        && ui.playing.get().is_none()
                },
                move || {
                    ui.panels.update(|p| p.quick = Some(i));
                    crate::player_keys::press_quick(ui, action);
                },
            )
        }))
        .self_start()
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
            ui.settings.get();
            crate::settings::choice(iptv_settings::keys::SEEK_KEYS)
        });
        words::hint(crate::input::PROFILE, shown.get(), focused, seek)
    };
    text(words)
        .px(px(8.0))
        .rounded(px(RADIUS))
        .bg(move || tint(ui.theme.with(|t| t.background.panel), PANEL_ALPHA))
        .text_size(px(OSD_DETAIL))
        .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA))
        .opacity(move || if words().is_empty() { 0.0_f32 } else { 1.0 })
        .pointer_events(PointerEvents::None)
}

/// The control strip: a horizontal list that keeps the focused control on
/// screen.
fn strip(ui: Ui, open: impl Fn() -> bool + Copy + 'static) -> VirtualList {
    let list = element_ref();
    let focus = computed(move || ui.panels.with(|p| p.focus));
    effect(move || {
        let at = focus.get();
        if open() {
            list.scroll_to_index(at as u32, percentage(0.5), px(0.0), true);
        }
    });
    virtual_list(CONTROLS.len(), move |at| {
        let index = at.index();
        let control = CONTROLS[index];
        let focused = move || {
            ui.keys_shown()
                && ui
                    .panels
                    .with(|p| p.shown == Shown::Controls && p.focus == index)
        };
        pill(
            ui,
            move || label(ui, control),
            focused,
            move || !enabled(ui, control),
            move || {
                ui.panels.update(|p| p.focus = index);
                crate::player_keys::press_control(ui, control);
            },
        )
        .mr(px(10.0))
    })
    .horizontal()
    .estimated_item_size(px(170.0))
    .element_ref(&list)
}
