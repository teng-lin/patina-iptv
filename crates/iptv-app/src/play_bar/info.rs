//! Compact single-line program metadata above the transport controls.

use iptv_core::guide::Listing;
use iptv_presentation::touch_zap::Direction;
use iptv_presentation::tracks::fps_label;
use nsh_core::platform::Idiom;
use nsh_core::prelude::*;

use crate::fonts::{
    OSD_BADGE, OSD_COMPACT_DETAIL, OSD_COMPACT_TITLE, OSD_COMPACT_TV_TITLE, OSD_DETAIL,
};
use crate::i18n::tr;
use crate::theme::{RADIUS, RESTING_ALPHA, TEXT_SECONDARY_ALPHA};
use crate::{Playback, Ui, channel_name, clock, tint};

use super::{WHITE, progress};

/// Estimated single-line header height including vertical padding.
pub(super) const ESTIMATED_HEIGHT: f32 = 30.0;
const LIVE_BG: u32 = 0xA93D42;
/// Minimum viewport width to show the inline "Next" preview.
const NEXT_MIN_WIDTH: f32 = 720.0;
/// Minimum viewport width to show secondary stream quality badges (resolution, fps).
const QUALITY_MIN_WIDTH: f32 = 480.0;

#[derive(Clone, PartialEq, Eq)]
struct ProgramInfo {
    channel: String,
    title: String,
    next: Option<(String, String)>,
}

fn program_info(ui: Ui) -> Option<ProgramInfo> {
    if crate::on_demand::active(ui) || ui.playing.get().is_none() {
        return None;
    }
    let (channel, program) = progress::program(ui);
    let schedule = crate::runtime_owner::schedule();
    let next = if ui.playback.get() == Playback::Live {
        let next = schedule.program_at(channel, program.end);
        match schedule.listing(channel, next) {
            Some(Listing::Title(title))
                if next.start >= program.end && !title.trim().is_empty() =>
            {
                Some((clock(next.start), title))
            }
            _ => None,
        }
    } else {
        None
    };
    Some(ProgramInfo {
        channel: format!(
            "{}  {}",
            crate::lists::number(ui, channel),
            channel_name(channel)
        ),
        title: crate::listing_labels::listing(schedule.listing(channel, program)),
        next,
    })
}

pub(super) fn header(ui: Ui) -> impl IntoElement {
    let program = computed(move || program_info(ui));
    let next = computed(move || program.with(|p| p.as_ref().and_then(|p| p.next.clone())));
    let size = move || ui.player.main().size.get().filter(|&(w, h)| w > 0 && h > 0);
    let fps = computed(move || fps_label(ui.player.main().format.with(|f| f.fps)));
    let live = move || !ui.paused.get() && ui.playback.get() == Playback::Live;
    let show_quality = move || window().width.0 >= QUALITY_MIN_WIDTH && !crate::scale::phone();
    let show_next = move || next.with(Option::is_some) && window().width.0 >= NEXT_MIN_WIDTH;
    let has_title = move || {
        crate::on_demand::title(ui)
            .or_else(|| program.with(|p| p.as_ref().map(|p| p.title.clone())))
            .is_some_and(|title| !title.trim().is_empty())
    };
    let title_size = if idiom() == Idiom::Tv {
        OSD_COMPACT_TV_TITLE
    } else {
        OSD_COMPACT_TITLE
    };
    let detail_size = if idiom() == Idiom::Tv {
        OSD_DETAIL
    } else {
        OSD_COMPACT_DETAIL
    };
    let secondary = tint(WHITE, TEXT_SECONDARY_ALPHA);
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(4.0))
        .py(px(1.0))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(when(
                    move || program.with(Option::is_some),
                    move || {
                        text(move || {
                            program.with(|p| {
                                p.as_ref().map_or_else(String::new, |p| p.channel.clone())
                            })
                        })
                        .flex_shrink(2.0)
                        .min_w(px(0.0))
                        .max_w(percentage(0.35))
                        .number_of_lines(1)
                        .text_size(px(detail_size))
                        .text_color(secondary)
                    },
                ))
                // Ch± for a finger (#285): live only; direct flex sibling so it
                // never shrinks or overlaps the program title.
                .child(when(
                    move || program.with(Option::is_some) && crate::touch_zap::live(ui),
                    move || {
                        view()
                            .flex_row()
                            .flex_shrink_0()
                            .gap(px(4.0))
                            .child(crate::touch_zap::button(ui, Direction::Previous))
                            .child(crate::touch_zap::button(ui, Direction::Next))
                    },
                ))
                .child(when(
                    move || program.with(Option::is_some) && has_title(),
                    move || {
                        text("·")
                            .flex_shrink_0()
                            .text_size(px(detail_size))
                            .text_color(secondary)
                    },
                ))
                .child(
                    text(move || {
                        crate::on_demand::title(ui)
                            .or_else(|| program.with(|p| p.as_ref().map(|p| p.title.clone())))
                            .filter(|title| !title.trim().is_empty())
                            .or_else(|| {
                                (!crate::on_demand::active(ui) && program.with(Option::is_none))
                                    .then(|| ui.playing.get().map(channel_name))
                                    .flatten()
                            })
                            .unwrap_or_default()
                    })
                    .test_id("bar-title")
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .number_of_lines(1)
                    .text_size(px(title_size))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(WHITE)),
                )
                .child(when(
                    move || program.with(Option::is_some),
                    move || {
                        badge(
                            move || {
                                let label = if ui.paused.get() {
                                    tr("PAUSED")
                                } else if live() {
                                    tr("LIVE")
                                } else {
                                    tr("CATCH-UP")
                                };
                                label.to_owned()
                            },
                            move || if live() { rgb(LIVE_BG) } else { quality_bg() },
                        )
                    },
                ))
                .child(when(
                    move || show_quality() && size().is_some(),
                    move || {
                        badge(
                            move || size().map(|(_, h)| format!("{h}p")).unwrap_or_default(),
                            quality_bg,
                        )
                    },
                ))
                .child(when(
                    move || show_quality() && fps.with(Option::is_some),
                    move || {
                        text(move || fps.get().unwrap_or_default())
                            .flex_shrink_0()
                            .text_size(px(OSD_BADGE))
                            .text_color(secondary)
                    },
                )),
        )
        .child(when(show_next, move || {
            text(move || {
                next.with(|p| {
                    p.as_ref().map_or_else(String::new, |(time, title)| {
                        format!("{} {time} · {title}", tr("Next"))
                    })
                })
            })
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .max_w(percentage(0.28))
            .number_of_lines(1)
            .text_right()
            .text_size(px(detail_size))
            .text_color(secondary)
        }))
}

fn quality_bg() -> Rgba {
    tint(WHITE, RESTING_ALPHA * 3)
}

fn badge(
    label: impl Fn() -> String + 'static,
    bg: impl Fn() -> Rgba + 'static,
) -> impl IntoElement {
    text(label)
        .flex_shrink_0()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(RADIUS))
        .text_size(px(OSD_BADGE))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(WHITE))
        .bg(bg)
}
