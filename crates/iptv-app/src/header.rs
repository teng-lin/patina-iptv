//! The guide's header: the preview at the top left (the video layer is laid
//! out under it in guide mode, `panels::preview_rect`) and, to its right,
//! the focused programme: its title (Roboto Medium) with the favourite star
//! at the right, its times with a progress bar and the minutes left (the
//! group's name at the right), and its description in grey. At the right
//! the list on screen (a tap opens the groups) and Add playlist.

use crate::i18n::{tr, trf};

use iptv_core::lists::group_of;
use std::time::Duration;

use nsh_core::prelude::*;

use crate::fonts::{HEADER_BODY, HEADER_TIMES, HEADER_TITLE, MEDIUM, SANS};
use crate::guide::{Airing, Pane, Program};
use crate::panels::{PREVIEW_MARGIN, PREVIEW_RADIUS, header_now};
use crate::scale::Scale;
use crate::theme::{BRIGHT_TEXT, RADIUS, RESTING_ALPHA, TEXT_SECONDARY_ALPHA};
use crate::{Ui, ViewMode, clock, tint};

const WHITE: u32 = 0xFFFFFF;
/// The progress bar on the times line (at full size: it, the type and the
/// gaps scale with the grid, `scale.rs`).
const PROGRESS_W: f32 = 64.0;
const PROGRESS_H: f32 = 4.0;
/// Lines of description under the times.
const BODY_LINES: u32 = 4;
/// The opaque ring that rounds the video's corners in the preview.
const RING: f32 = 12.0;

fn secondary() -> Rgba {
    tint(WHITE, TEXT_SECONDARY_ALPHA)
}

/// The focused programme: data version (a schedule swap re-renders even on
/// equal minutes), now, row, catalog channel, programme. `None` on an
/// empty list.
type Focused = Option<(u64, i32, usize, usize, Program)>;

/// Its times, in our words.
fn span(p: Program) -> String {
    format!("{} – {}", clock(p.start), clock(p.end))
}

/// How far along it is, in our words.
fn left(now: i32, p: Program) -> String {
    match p.airing(now) {
        Airing::Live => trf(
            "{minutes} min left",
            &[("minutes", (p.end - now).unsigned_abs().into())],
        ),
        Airing::Upcoming => trf(
            "in {minutes} min",
            &[("minutes", (p.start - now).unsigned_abs().into())],
        ),
        Airing::Ended => tr("ended").to_owned(),
    }
}

fn preview(ui: Ui, size: Scale) -> impl IntoElement {
    // Transparent while something plays: the video layer sits under this
    // rectangle in guide mode. A tap opens full screen.
    let idle = move || ui.playing.get().is_none();
    view()
        .role(AccessibilityRole::Button)
        .w(move || px(header_now().1))
        .h(move || px(header_now().2))
        .rounded(px(PREVIEW_RADIUS))
        .bg(move || {
            let cell = ui.theme.with(|t| t.guide().cell);
            if idle() {
                crate::grid::fill(ui, cell)
            } else {
                tint(cell, 0)
            }
        })
        .items_center()
        .justify_center()
        .on_press(move || {
            ui.touched();
            if ui.playing.get().is_some() {
                ui.mode.set(crate::ViewMode::Fullscreen);
            }
        })
        .child(
            text(move || if idle() { "Nothing playing" } else { "" })
                .font_family(SANS)
                .text_size(move || size.text_px(HEADER_TIMES))
                .text_color(secondary()),
        )
}

/// Rounds the video's corners: a ring of the window colour whose inner edge
/// is the preview's rounded rectangle (guide mode only).
fn preview_ring(ui: Ui) -> impl IntoElement {
    view()
        .absolute()
        .left(px(PREVIEW_MARGIN - RING))
        .top(px(PREVIEW_MARGIN - RING))
        .w(move || px(header_now().1 + 2.0 * RING))
        .h(move || px(header_now().2 + 2.0 * RING))
        .border(px(RING))
        .rounded(px(PREVIEW_RADIUS + RING))
        .border_color(move || rgb(ui.theme.with(|t| t.background.window)))
        .opacity(move || {
            let shown = ui.mode.get() == ViewMode::Guide && ui.playing.get().is_some();
            if shown { 1.0_f32 } else { 0.0 }
        })
        .pointer_events(PointerEvents::None)
}

/// Keys closer together than this are a held key: the description waits
/// for them to pause.
const SETTLE: Duration = Duration::from_millis(160);

/// The focused programme as the description shows it: at once after a
/// pause, blank while keys repeat (a held key moves the focus 15–20 times a
/// second, and laying out four lines of description each time costs a TV
/// box more than the rest of the move), then the programme the keys
/// stopped on.
fn settled(ui: Ui, program: Memo<Focused>) -> Signal<Focused> {
    let shown = signal(program.get_untracked());
    let last = stored(None::<u64>);
    let generation = stored(0_u32);
    effect(move || {
        let p = program.get();
        let now = ui.now_ms();
        let held = last
            .with(|l| l.replace(now))
            .is_some_and(|t| now.saturating_sub(t) < SETTLE.as_millis() as u64);
        let g = generation.with(|g| {
            *g += 1;
            *g
        });
        if !held {
            shown.set(p);
            return;
        }
        if shown.with_untracked(Option::is_some) {
            shown.set(None);
        }
        let _task = set_timeout(SETTLE, move || {
            if generation.with(|now| *now == g) {
                shown.set(program.get_untracked());
            }
        });
    });
    shown
}

fn details(ui: Ui, program: Memo<Focused>, size: Scale) -> impl IntoElement {
    let settled = settled(ui, program);
    // Read once per focused programme (a stored guide reads it from disk).
    let description = computed(move || {
        ui.guide_pages.get();
        settled.with(|p| {
            let (_, _, _, ch, p) = (*p)?;
            let schedule = crate::schedule();
            if let Some(s) = schedule
                .as_any()
                .and_then(|s| s.downcast_ref::<iptv_guide::combined::CombinedSchedule>())
            {
                s.description(ch, p)
            } else {
                schedule
                    .as_any()
                    .and_then(|s| s.downcast_ref::<iptv_guide::EpgSchedule>())
                    .and_then(|s| s.description(ch, p))
            }
        })
    });
    let favourite = move || {
        program
            .with(|p| p.map(|p| p.3))
            .is_some_and(|ch| ui.favorites.with(|f| f.contains(&ch)))
    };
    let live =
        move || program.with(|p| p.is_some_and(|(_, now, _, _, p)| p.airing(now) == Airing::Live));
    let group = move || {
        program.with(|p| {
            let (_, _, _, ch, _) = (*p)?;
            let catalog = crate::schedule();
            group_of(&*catalog, ch).map(|g| catalog.group_name(g))
        })
    };
    view()
        .flex_1()
        .gap(move || size.px(6.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(move || size.px(12.0))
                .child(
                    text(move || {
                        program.with(|p| {
                            p.map_or_else(String::new, |(_, _, _, ch, p)| {
                                crate::schedule().title(ch, p)
                            })
                        })
                    })
                    .flex_1()
                    .number_of_lines(1)
                    .font_family(SANS)
                    .font_weight(MEDIUM)
                    .text_size(move || size.text_px(HEADER_TITLE))
                    .text_color(rgb(WHITE)),
                )
                .child(
                    text(move || if favourite() { "★" } else { "" })
                        .text_size(move || size.text_px(HEADER_TITLE * 0.8))
                        .text_color(move || rgb(ui.theme.with(|t| t.accent.light))),
                ),
        )
        .child(
            // Times, progress and minutes left; the group at the right.
            view()
                .flex_row()
                .items_center()
                .gap(move || size.px(12.0))
                .child(
                    text(move || program.with(|p| p.map(|p| span(p.4)).unwrap_or_default()))
                        .number_of_lines(1)
                        .font_family(SANS)
                        .text_size(move || size.text_px(HEADER_TIMES))
                        .text_color(rgb(BRIGHT_TEXT)),
                )
                .child(
                    view()
                        .w(move || px(if live() { size.of(PROGRESS_W) } else { 0.0 }))
                        .h(move || size.px(PROGRESS_H))
                        .rounded(move || size.px(PROGRESS_H / 2.0))
                        .bg(tint(WHITE, RESTING_ALPHA * 3))
                        .child(
                            view()
                                .h_full()
                                .rounded(move || size.px(PROGRESS_H / 2.0))
                                .w(move || {
                                    let done = program.with(|p| {
                                        p.map_or(0.0, |(_, now, _, _, p)| p.progress(now))
                                    });
                                    px(if live() {
                                        size.of(PROGRESS_W) * done
                                    } else {
                                        0.0
                                    })
                                })
                                .bg(move || rgb(ui.theme.with(|t| t.accent.light))),
                        ),
                )
                .child(
                    text(move || {
                        program
                            .with(|p| p.map(|(_, now, _, _, p)| left(now, p)).unwrap_or_default())
                    })
                    .number_of_lines(1)
                    .font_family(SANS)
                    .text_size(move || size.text_px(HEADER_TIMES))
                    .text_color(secondary()),
                )
                .child(
                    text(move || group().unwrap_or_default())
                        .flex_1()
                        .number_of_lines(1)
                        .text_right()
                        .font_family(SANS)
                        .text_size(move || size.text_px(HEADER_TIMES))
                        .text_color(secondary()),
                ),
        )
        .child(
            text(move || description.get().unwrap_or_default())
                .number_of_lines(BODY_LINES)
                .font_family(SANS)
                .text_size(move || size.text_px(HEADER_BODY))
                .text_color(secondary()),
        )
}

/// The list on screen (a tap opens the groups), Add playlist, and in
/// development the key stats ([`debug_overlay`]).
fn side(ui: Ui) -> impl IntoElement {
    view()
        .items_end()
        .gap(px(8.0))
        .child(
            text(move || format!("☰  {}", crate::lists::list_name(ui)))
                .role(AccessibilityRole::Button)
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(RADIUS))
                .bg(tint(WHITE, RESTING_ALPHA))
                .number_of_lines(1)
                .text_size(px(15.0))
                .text_color(rgb(WHITE))
                .on_press(move || {
                    ui.touched();
                    crate::lists::open_groups(ui);
                }),
        )
        .child(crate::wizard_view::add_button(ui))
        .child(view().flex_1())
        .children(debug_overlay().then(|| {
            text(move || {
                ui.stats
                    .with(|s| format!("keys {} · {} µs (max {})", s.keys, s.last_us, s.max_us))
            })
            .text_size(px(11.0))
            .text_color(tint(WHITE, 0x60))
        }))
}

/// The key-timing line in the header is for development: debug builds, or
/// `IPTV_RS_DEBUG_OVERLAY` set (not empty, not `0`). The log line in
/// `input.rs` stays in every build (`tools/perf-android.sh` reads it).
pub fn debug_overlay() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        overlay_wanted(
            cfg!(debug_assertions),
            std::env::var("IPTV_RS_DEBUG_OVERLAY").ok().as_deref(),
        )
    })
}

fn overlay_wanted(debug_build: bool, env: Option<&str>) -> bool {
    debug_build || env.is_some_and(|v| !v.is_empty() && v != "0")
}

pub fn header(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let program = computed(move || {
        let version = ui.guide_data();
        let view = crate::lists::view(ui);
        ui.focus.with(|f| {
            let p = f.program(&view);
            view.channel(f.channel)
                .map(|ch| (version, f.now, f.channel, ch, p))
        })
    });
    view()
        .pt_safe_area()
        .bg(move || ui.surface(ui.theme.with(|t| t.background.window)))
        .child(
            view()
                .h(move || px(header_now().0))
                .p(px(PREVIEW_MARGIN))
                .flex_row()
                .gap(move || size.px(24.0))
                .child(preview(ui, size))
                .child(details(ui, program, size))
                .child(side(ui))
                .child(preview_ring(ui))
                .child(when(move || corner_back_shown(ui), move || corner_back(ui))),
        )
}

/// Whether the guide's ‹ shows: on pointer and touch devices, until the rail
/// (the top of the chain) is open.
fn corner_back_shown(ui: Ui) -> bool {
    ui.back_controls()
        && ui.mode.get() == ViewMode::Guide
        && ui.focus.with(|f| f.pane) != Pane::Rail
}

/// A pointer or touch route to the left columns, over the preview where
/// the full-screen player keeps its ‹: one step of the chain a remote's
/// Back and Left walk, the guide to its groups, the groups to the rail
/// with its names (Search, Settings).
fn corner_back(ui: Ui) -> impl IntoElement {
    let groups = move || ui.focus.with(|f| f.pane) == Pane::Guide;
    view()
        .absolute()
        // The player's ‹ is 20 and 16 points in from the safe area.
        .left(px(20.0))
        .top(px(16.0))
        .child(crate::ui_kit::round_back(
            crate::ui_kit::TOUCH,
            0.0,
            crate::play_bar::back_bg(ui),
            move || {
                if groups() { tr("Groups") } else { tr("Menu") }
            },
            move || {
                ui.touched();
                if groups() {
                    crate::lists::open_groups(ui);
                } else {
                    crate::lists::open_rail(ui);
                }
            },
        ))
}

#[cfg(test)]
mod tests {
    use super::overlay_wanted;

    #[test]
    fn the_key_timing_line_is_for_development_only() {
        assert!(!overlay_wanted(false, None), "release builds hide it");
        assert!(!overlay_wanted(false, Some("")));
        assert!(!overlay_wanted(false, Some("0")));
        assert!(overlay_wanted(false, Some("1")), "unless asked for");
        assert!(overlay_wanted(true, None), "debug builds show it");
    }
}
