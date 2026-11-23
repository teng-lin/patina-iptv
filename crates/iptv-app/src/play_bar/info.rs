//! Program metadata above the transport controls.

use iptv_core::guide::Listing;
use iptv_presentation::tracks::{fps_label, resolution_class};
use nsh_core::prelude::*;

use crate::fonts::{OSD_BADGE, OSD_DETAIL, OSD_TITLE};
use crate::i18n::tr;
use crate::theme::{RADIUS, RESTING_ALPHA, TEXT_SECONDARY_ALPHA};
use crate::{Playback, Ui, channel_name, clock, tint};

use super::{WHITE, progress};

pub(super) const ESTIMATED_HEIGHT: f32 = 64.0;
const LIVE_BG: u32 = 0xA93D42;

#[derive(Clone, PartialEq, Eq)]
struct ProgramInfo {
    channel: String,
    title: String,
    synopsis: Option<String>,
    next: Option<(String, String)>,
}

fn program_info(ui: Ui) -> Option<ProgramInfo> {
    ui.guide_data();
    if crate::on_demand::active(ui) || ui.playing.get().is_none() {
        return None;
    }
    let (channel, program) = progress::program(ui);
    let schedule = crate::schedule();
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
        synopsis: crate::header::program_description(&*schedule, channel, program)
            .filter(|description| !description.trim().is_empty()),
        next,
    })
}

pub(super) fn header(ui: Ui) -> impl IntoElement {
    let program = computed(move || program_info(ui));
    let next = computed(move || program.with(|p| p.as_ref().and_then(|p| p.next.clone())));
    let size = move || ui.player.main().size.get().filter(|&(w, h)| w > 0 && h > 0);
    let fps = computed(move || fps_label(ui.player.main().format.with(|f| f.fps)));
    let live = move || !ui.paused.get() && ui.playback.get() == Playback::Live;
    let compact = || window().width.0 < 480.0;
    let secondary = tint(WHITE, TEXT_SECONDARY_ALPHA);
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(16.0))
        .px(px(4.0))
        .py(px(4.0))
        .child(
            view()
                .flex_1()
                .min_w(move || -> Length {
                    if compact() {
                        percentage(1.0).into()
                    } else {
                        px(0.0).into()
                    }
                })
                .gap(px(3.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(when(
                            move || program.with(Option::is_some),
                            move || {
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.0))
                                    .min_w(px(0.0))
                                    .child(
                                        text(move || {
                                            program.with(|p| {
                                                p.as_ref()
                                                    .map_or_else(String::new, |p| p.channel.clone())
                                            })
                                        })
                                        .flex_shrink(1.0)
                                        .min_w(px(0.0))
                                        .number_of_lines(1)
                                        .text_size(px(OSD_DETAIL))
                                        .text_color(secondary),
                                    )
                                    .child(badge(
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
                                    ))
                            },
                        ))
                        .child(when(
                            move || size().is_some(),
                            move || {
                                view()
                                    .flex_shrink_0()
                                    .flex_row()
                                    .gap(px(6.0))
                                    .child(badge(
                                        move || {
                                            size().map(|(_, h)| format!("{h}p")).unwrap_or_default()
                                        },
                                        quality_bg,
                                    ))
                                    .child(badge(
                                        move || {
                                            size()
                                                .map(|(w, h)| resolution_class(w, h).to_owned())
                                                .unwrap_or_default()
                                        },
                                        quality_bg,
                                    ))
                            },
                        ))
                        .child(when(
                            move || fps.with(Option::is_some),
                            move || {
                                text(move || fps.get().unwrap_or_default())
                                    .flex_shrink_0()
                                    .text_size(px(OSD_BADGE))
                                    .text_color(secondary)
                            },
                        )),
                )
                .child(
                    text(move || {
                        crate::on_demand::title(ui)
                            .or_else(|| program.with(|p| p.as_ref().map(|p| p.title.clone())))
                            .filter(|title| !title.trim().is_empty())
                            .or_else(|| ui.playing.get().map(channel_name))
                            .unwrap_or_default()
                    })
                    .test_id("bar-title")
                    .number_of_lines(1)
                    .text_size(px(OSD_TITLE))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(WHITE)),
                )
                .child(when(
                    move || program.with(|p| p.as_ref().is_some_and(|p| p.synopsis.is_some())),
                    move || {
                        text(move || {
                            program
                                .with(|p| p.as_ref().and_then(|p| p.synopsis.clone()))
                                .unwrap_or_default()
                        })
                        .number_of_lines(2)
                        .text_size(px(OSD_DETAIL))
                        .text_color(secondary)
                    },
                )),
        )
        .child(when(
            move || next.with(Option::is_some),
            move || {
                view()
                    .min_w(px(0.0))
                    .max_w(move || percentage(if compact() { 1.0 } else { 0.36 }))
                    .gap(px(3.0))
                    .child(
                        text(move || {
                            next.with(|p| {
                                p.as_ref().map_or_else(String::new, |(time, _)| {
                                    format!("{}  {time}", tr("Next"))
                                })
                            })
                        })
                        .number_of_lines(1)
                        .text_right()
                        .text_size(px(OSD_DETAIL))
                        .text_color(secondary),
                    )
                    .child(
                        text(move || {
                            next.with(|p| {
                                p.as_ref()
                                    .map_or_else(String::new, |(_, title)| title.clone())
                            })
                        })
                        .number_of_lines(1)
                        .text_right()
                        .text_size(px(OSD_DETAIL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(WHITE)),
                    )
            },
        ))
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
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(RADIUS))
        .text_size(px(OSD_BADGE))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(WHITE))
        .bg(bg)
}
