//! The play bar's progress indicators: a slim top hairline (`live_hairline`)
//! for live TV, and an interactive timeline (`line`) with start/end times for
//! catch-up and on-demand playback (press or drag seeks, `seek.rs`).

use crate::i18n::tr;
use nsh_core::platform::Idiom;
use nsh_core::prelude::*;
use nsh_core::{TouchEvent, TouchPhase};

use crate::theme::{RADIUS, RESTING_ALPHA, TEXT_SECONDARY_ALPHA};
use crate::{Playback, Ui, clock, tint};

const WHITE: u32 = 0xFFFFFF;
pub(super) const HAIRLINE_HEIGHT: f32 = 2.5;
pub(super) const TIMELINE_HEIGHT: f32 = 20.0;

/// Slim non-interactive progress hairline shown at the top of the live bar.
pub(super) fn live_hairline(ui: Ui) -> impl IntoElement {
    let fraction = move || {
        let (_, p) = program(ui);
        p.progress(ui.guide.focus.with(|f| f.now)).clamp(0.0, 1.0)
    };
    let accent = move || rgb(ui.app.theme.with(|t| t.accent.light));
    view()
        .test_id("live-progress-hairline")
        .accessible(true)
        .label(move || {
            let (_, p) = program(ui);
            format!(
                "{} · {} / {}",
                tr("Playback position"),
                clock(p.start),
                clock(p.end)
            )
        })
        .mx(px(4.0))
        .h(px(HAIRLINE_HEIGHT))
        .rounded(px(HAIRLINE_HEIGHT / 2.0))
        .bg(tint(WHITE, RESTING_ALPHA * 2))
        .child(
            view()
                .h_full()
                .rounded(px(HAIRLINE_HEIGHT / 2.0))
                .w(move || percentage(fraction()))
                .bg(accent),
        )
}

/// The program shown on the bar: the pending zap target when a channel switch
/// is waiting to tune, otherwise the currently playing live or replayed program.
pub(super) fn program(ui: Ui) -> (usize, crate::guide::Program) {
    ui.guide_data();
    let schedule = crate::runtime_owner::schedule();
    if ui.playback.playback.get() == Playback::Live
        && let Some(channel) = crate::browse_playback::pending_channel(ui)
    {
        return (
            channel,
            schedule.program_at(channel, ui.guide.focus.with(|f| f.now)),
        );
    }
    let channel = ui.playback.playing.get().unwrap_or(0);
    let minute = match ui.playback.playback.get() {
        Playback::Live => ui.guide.focus.with(|f| f.now),
        Playback::CatchUp { start, .. } => start,
    };
    (channel, schedule.program_at(channel, minute))
}

/// The progress line with its times; in catch-up a press or a drag on it
/// seeks.
pub(super) fn line(ui: Ui) -> impl IntoElement {
    let width = signal(0.0_f32);
    let catch_up = move || {
        matches!(ui.playback.playback.get(), Playback::CatchUp { .. })
            || crate::on_demand::seekable(ui)
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
            let (_, p) = program(ui);
            p.progress(ui.guide.focus.with(|f| f.now))
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
            let (_, p) = program(ui);
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
    let accent = move || rgb(ui.app.theme.with(|t| t.accent.light));
    let time = move |s: Box<dyn Fn() -> String>| {
        text(s)
            .w(px(64.0))
            .number_of_lines(1)
            .text_center()
            .text_size(px(12.0))
            .text_color(tint(WHITE, TEXT_SECONDARY_ALPHA))
    };
    let touch_slop = if matches!(idiom(), Idiom::Desktop | Idiom::Tv) {
        0.0
    } else {
        8.0
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
                .h(px(TIMELINE_HEIGHT))
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
                        .top(px((TIMELINE_HEIGHT - 12.0) / 2.0))
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
                        .role(AccessibilityRole::Adjustable)
                        .label(move || {
                            format!("{} · {} / {}", tr("Playback position"), ends().0, ends().1)
                        })
                        .accessibility_actions(vec![
                            nsh_core::AccessibilityActionDef {
                                name: "increment".into(),
                                label: None,
                            },
                            nsh_core::AccessibilityActionDef {
                                name: "decrement".into(),
                                label: None,
                            },
                        ])
                        .on_accessibility_action(move |action: &str| match action {
                            "increment" => super::seek_timeline(ui, true),
                            "decrement" => super::seek_timeline(ui, false),
                            _ => {}
                        })
                        .disabled(move || !catch_up())
                        .accessibility_state(move || AccessibilityState {
                            selected: super::focused(
                                ui,
                                iptv_presentation::play_bar::Control::Timeline,
                            ),
                            ..Default::default()
                        })
                        .border(px(2.0))
                        .border_color(move || {
                            tint(
                                ui.app.theme.with(|t| t.accent.light),
                                if super::focused(
                                    ui,
                                    iptv_presentation::play_bar::Control::Timeline,
                                ) {
                                    255
                                } else {
                                    0
                                },
                            )
                        })
                        .absolute()
                        .inset_0()
                        .hit_slop(EdgeInsets {
                            top: px(touch_slop),
                            bottom: px(touch_slop),
                            left: px(0.0),
                            right: px(0.0),
                        })
                        .rounded(px(RADIUS))
                        .on_touch(seek),
                ),
        )
        .child(time(Box::new(move || ends().1)))
}

/// Where a press `x` points into a progress bar `width` points wide seeks
/// to, in seconds into a program `length` seconds long, no further than
/// `limit` (what the archive holds so far).
fn seek_target(x: f32, width: f32, length: i32, limit: i32) -> i32 {
    // A NaN width seeks nowhere too.
    if width.is_nan() || width <= 0.0 || length <= 0 {
        return 0;
    }
    let fraction = (x / width).clamp(0.0, 1.0);
    ((fraction * length as f32).round() as i32).clamp(0, limit.clamp(0, length))
}

#[cfg(test)]
mod tests {
    use super::seek_target;

    #[test]
    fn a_press_on_the_progress_bar_seeks_within_the_archive() {
        // A 60-minute program, 600 px wide.
        assert_eq!(seek_target(300.0, 600.0, 3_600, 3_600), 1_800);
        assert_eq!(seek_target(-20.0, 600.0, 3_600, 3_600), 0);
        assert_eq!(seek_target(900.0, 600.0, 3_600, 3_600), 3_600);
        // Still airing: no further than what aired.
        assert_eq!(seek_target(540.0, 600.0, 3_600, 1_200), 1_200);
        // Nothing laid out yet, or no length.
        assert_eq!(seek_target(10.0, 0.0, 3_600, 3_600), 0);
        assert_eq!(seek_target(10.0, 600.0, 0, 0), 0);
    }
}
