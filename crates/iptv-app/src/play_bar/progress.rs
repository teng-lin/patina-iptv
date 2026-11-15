//! The play bar's progress line: in catch-up where the archive plays
//! (a press or a drag seeks there, `seek.rs`), live how far the programme
//! is; its times at both ends.

use iptv_core::play_bar::seek_target;
use nsh_core::prelude::*;
use nsh_core::{TouchEvent, TouchPhase};

use crate::theme::{RADIUS, RESTING_ALPHA, TEXT_SECONDARY_ALPHA};
use crate::{Playback, Ui, clock, tint};

const WHITE: u32 = 0xFFFFFF;

/// The programme on screen: live now, or the replayed one.
pub(super) fn programme(ui: Ui) -> (usize, crate::guide::Program) {
    ui.data_version.get();
    let channel = ui.playing.get().unwrap_or(0);
    let minute = match ui.playback.get() {
        Playback::Live => ui.focus.with(|f| f.now),
        Playback::CatchUp { start, .. } => start,
    };
    (channel, crate::schedule().program_at(channel, minute))
}

/// The progress line with its times; in catch-up a press or a drag on it
/// seeks.
pub(super) fn line(ui: Ui) -> impl IntoElement {
    let width = signal(0.0_f32);
    let catch_up = move || {
        matches!(ui.playback.get(), Playback::CatchUp { .. }) || crate::on_demand::seekable(ui)
    };
    let fraction = move || match crate::seek::elapsed(ui) {
        Some((at, length)) => {
            if length > 0 {
                at as f32 / length as f32
            } else {
                0.0
            }
        }
        None => {
            let (_, p) = programme(ui);
            p.progress(ui.focus.with(|f| f.now))
        }
    };
    let ends = move || match crate::seek::elapsed(ui) {
        Some((at, length)) => (
            crate::seek::hms(at),
            if length > 0 {
                crate::seek::hms(length)
            } else {
                "–:––".into()
            },
        ),
        None => {
            let (_, p) = programme(ui);
            (clock(p.start), clock(p.end))
        }
    };
    let seek = move |e: &TouchEvent| {
        let Some(p) = e.points.iter().find(|p| p.changed) else {
            return;
        };
        let Some((length, limit)) = crate::seek::span(ui) else {
            return;
        };
        let target = seek_target(p.location.x.0, width.get_untracked(), length, limit);
        let done = matches!(e.phase, TouchPhase::End);
        if !matches!(e.phase, TouchPhase::Cancel) {
            crate::seek::to(ui, target, done);
        }
        super::used(ui);
    };
    let accent = move || rgb(ui.theme.with(|t| t.accent.light));
    let time = move |s: Box<dyn Fn() -> String>| {
        text(s)
            .w(px(64.0))
            .text_center()
            .text_size(px(12.0))
            .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA))
    };
    view()
        .test_id("play-bar-time-line")
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(6.0))
        .child(time(Box::new(move || ends().0)))
        .child(
            view()
                .flex_1()
                .h(px(20.0))
                .justify_center()
                .on_layout(move |b: &Bounds<Points>| width.set(b.size.width.0))
                .child(
                    view()
                        .h(px(4.0))
                        .rounded(px(2.0))
                        .bg(tint(WHITE, RESTING_ALPHA * 2))
                        .child(
                            view()
                                .h_full()
                                .rounded(px(2.0))
                                .w(move || percentage(fraction()))
                                .bg(accent),
                        ),
                )
                .child(
                    // The playing place (catch-up: draggable).
                    view()
                        .absolute()
                        .top(px(4.0))
                        .left(move || percentage(fraction()))
                        .ml(px(-6.0))
                        .w(px(12.0))
                        .h(px(12.0))
                        .rounded(px(6.0))
                        .bg(rgb(WHITE))
                        .opacity(move || if catch_up() { 1.0_f32 } else { 0.0 }),
                )
                // On top, for presses and drags along the whole line.
                .child(
                    view()
                        .test_id("play-bar-seek")
                        .absolute()
                        .inset_0()
                        .rounded(px(RADIUS))
                        .on_touch(seek),
                ),
        )
        .child(time(Box::new(move || ends().1)))
}
