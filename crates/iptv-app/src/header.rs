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
use iptv_core::layout::Shortcuts;

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
    let idle = move || ui.playing.get().is_none() && !crate::on_demand::active(ui);
    let focused = move || ui.keys_shown() && ui.focus.with(|f| f.pane == Pane::Preview);
    view()
        .test_id("guide-preview")
        .role(AccessibilityRole::Button)
        .label(move || tr("Back to the player"))
        .accessibility_state(move || AccessibilityState {
            selected: focused(),
            disabled: idle(),
            ..Default::default()
        })
        .border(px(3.0))
        .border_color(move || {
            tint(
                ui.app.theme.with(|t| t.accent.light),
                if focused() { 255 } else { 0 },
            )
        })
        .w(move || px(header_now().1))
        .h(move || px(header_now().2))
        .rounded(px(PREVIEW_RADIUS))
        .bg(move || {
            let cell = ui.app.theme.with(|t| t.guide().cell);
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
            if !idle() {
                ui.mode.set(crate::ViewMode::Fullscreen);
                ui.show_playing();
            }
        })
        .child(
            text(move || if idle() { tr("Nothing playing") } else { "" })
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
        .border_color(move || rgb(ui.app.theme.with(|t| t.background.window)))
        .opacity(move || {
            let shown = ui.mode.get() == ViewMode::Guide
                && (ui.playing.get().is_some() || crate::on_demand::active(ui));
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
        ui.app.guide_pages.get();
        settled.with(|p| {
            let (_, _, _, ch, p) = (*p)?;
            let schedule = crate::schedule();
            programme_description(&*schedule, ch, p)
        })
    });
    let favourite = move || {
        program
            .with(|p| p.map(|p| p.3))
            .is_some_and(|ch| ui.app.favorites.with(|f| f.contains(&ch)))
    };
    let live =
        move || program.with(|p| p.is_some_and(|(_, now, _, _, p)| p.airing(now) == Airing::Live));
    let group = move || {
        program.with(|p| {
            let (_, _, _, ch, _) = (*p)?;
            let catalog = crate::schedule();
            group_of(&*catalog, ch).map(|g| catalog.group_name_in(g, crate::i18n::language()))
        })
    };
    view()
        .test_id("guide-header-details")
        .flex_1()
        // A compact header gives it less room than its lines want: clip them
        // (the header grows for the title and times: `GuideHeader`).
        .overflow_hidden()
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
                                crate::schedule().title_in(ch, p, crate::i18n::language())
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
                        .text_color(move || rgb(ui.app.theme.with(|t| t.accent.light))),
                )
                .child(when(
                    move || more_shown(ui, program) || more_reserved(),
                    move || {
                        // A slot the ⋯'s width: kept empty for keys where
                        // touch or a pointer is usual, so the title's room
                        // does not change when the input does.
                        view()
                            .test_id("guide-more-slot")
                            .flex_shrink_0()
                            .w(move || px(more_side(size)))
                            .child(when(
                                move || more_shown(ui, program),
                                move || more(ui, size),
                            ))
                    },
                )),
        )
        .child(
            // Times, progress and minutes left; the group at the right. A
            // narrow header wraps them onto a second line rather than clip,
            // and a part wider than the column (large text in a Split View
            // window) wraps within it.
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(move || size.px(12.0))
                .child(
                    text(move || program.with(|p| p.map(|p| span(p.4)).unwrap_or_default()))
                        .max_w(percentage(1.0))
                        .number_of_lines(2)
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
                                .bg(move || rgb(ui.app.theme.with(|t| t.accent.light))),
                        ),
                )
                .child(
                    text(move || {
                        program
                            .with(|p| p.map(|(_, now, _, _, p)| left(now, p)).unwrap_or_default())
                    })
                    .max_w(percentage(1.0))
                    .number_of_lines(2)
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

/// The ⋯ at the end of the focused programme's title, for touch and a
/// pointer: the menu the remote's Menu key opens for it (favourites,
/// recording, details, replay). Its box is the title's text size (under
/// the title's line height at every scale and text size, so showing it
/// moves no text), its press reaches the least touch size around it.
/// Keys hide it: they have Menu.
fn more(ui: Ui, size: Scale) -> View {
    crate::ui_kit::round_more(
        move || more_side(size),
        move || tint(WHITE, RESTING_ALPHA),
        || tr("More options").to_owned(),
        move || {
            ui.touched();
            crate::menu_view::open(ui);
        },
    )
    .test_id("guide-more")
}

fn more_side(size: Scale) -> f32 {
    size.text_px(HEADER_TITLE).0.round()
}

/// Whether the header's ⋯ shows: for touch and a pointer, with a
/// programme to open, and not during an inline reorder of channels or
/// groups (the guide takes no pointer then, and the order is unfinished).
fn more_shown(ui: Ui, program: Memo<Focused>) -> bool {
    !ui.keys_shown()
        && program.with(Option::is_some)
        && !crate::organization_view::inline_order_open(ui)
}

/// Whether the title keeps the ⋯'s room while it is hidden: wherever touch
/// or a pointer is usual. A TV's or a terminal's remote rarely gives way to
/// one, so there the title keeps the room and the ⋯ takes it when shown.
fn more_reserved() -> bool {
    !matches!(
        idiom(),
        nsh_core::platform::Idiom::Tv | nsh_core::platform::Idiom::Terminal
    )
}

fn library_shortcuts(ui: Ui) -> View {
    let size = Scale::new();
    view().flex_row().gap(px(8.0)).children(
        [
            (
                iptv_core::browse::RailItem::Movies,
                iptv_core::vod::Kind::Movie,
                (|| tr("Movies")) as fn() -> &'static str,
            ),
            (
                iptv_core::browse::RailItem::Shows,
                iptv_core::vod::Kind::Series,
                || tr("Shows"),
            ),
        ]
        .into_iter()
        .map(move |(item, kind, label)| {
            // The label follows a live change of language.
            crate::ui_kit::button(
                ui,
                size,
                label,
                || false,
                move || crate::vod_view::open(ui, kind),
            )
            .test_id(if kind == iptv_core::vod::Kind::Movie {
                "open-movies"
            } else {
                "open-shows"
            })
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                svg(crate::rail::icon(item))
                    .size(px(22.0))
                    .text_color(rgb(WHITE)),
            )
        }),
    )
}

/// The list on screen; a tap opens the groups.
fn list_button(ui: Ui) -> impl IntoElement {
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
        })
}

/// Clear while the inline channel order is open over the header.
fn shortcuts_opacity(ui: Ui) -> f32 {
    if crate::organization_view::inline_order_open(ui) {
        0.0
    } else {
        1.0
    }
}

/// The list on screen (a tap opens the groups), Add playlist, and in
/// development the key stats ([`debug_overlay`]).
fn side(ui: Ui) -> impl IntoElement {
    view()
        .items_end()
        .opacity(move || shortcuts_opacity(ui))
        .gap(px(8.0))
        .child(list_button(ui))
        .child(library_shortcuts(ui))
        .child(crate::wizard_view::add_button(ui))
        .child(view().flex_1())
        .children(key_stats(ui))
}

/// The key stats line, in development only ([`debug_overlay`]).
fn key_stats(ui: Ui) -> Option<Text> {
    debug_overlay().then(|| {
        text(move || {
            ui.stats
                .with(|s| format!("keys {} · {} µs (max {})", s.keys, s.last_us, s.max_us))
        })
        .text_size(px(11.0))
        .text_color(tint(WHITE, 0x60))
    })
}

/// The same shortcuts in a compact header ([`Shortcuts::Below`]): one row
/// under the preview and the focused programme, scrolled sideways where the
/// window is narrower than the row.
fn shortcuts_row(ui: Ui) -> impl IntoElement {
    scroll()
        .horizontal()
        .scrollbar(false)
        .test_id("guide-header-shortcuts")
        .opacity(move || shortcuts_opacity(ui))
        .mt(move || px(crate::panels::guide_header_now().row_gap))
        .h(move || px(crate::panels::guide_header_now().row))
        .flex_shrink_0()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(list_button(ui))
                .child(library_shortcuts(ui))
                .child(crate::wizard_view::add_button(ui))
                .children(key_stats(ui)),
        )
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
    // A compact window moves the shortcuts under the preview and the
    // programme (`iptv_core::layout`). Only this header's own views change:
    // the preview keeps its place, and the player under it is resized.
    let below = computed(move || crate::panels::shortcuts_now() == Shortcuts::Below);
    view()
        .pt_safe_area()
        .accessibility_elements_hidden(move || crate::organization_view::guide_order_open(ui))
        .bg(move || ui.surface(ui.app.theme.with(|t| t.background.window)))
        .child(
            view()
                .h(move || px(header_now().0))
                .p(px(PREVIEW_MARGIN))
                .child(
                    view()
                        .flex_1()
                        .min_h(px(0.0))
                        .flex_row()
                        .gap(move || size.px(24.0))
                        .child(preview(ui, size))
                        .child(details(ui, program, size))
                        .child(when(move || !below.get(), move || side(ui))),
                )
                .child(when(move || below.get(), move || shortcuts_row(ui)))
                .child(preview_ring(ui))
                .child(when(move || corner_back_shown(ui), move || corner_back(ui))),
        )
}

/// Whether the guide's ‹ shows: on pointer and touch devices, until the rail
/// (the top of the chain) is open.
fn corner_back_shown(ui: Ui) -> bool {
    ui.back_controls()
        && ui.mode.get() == ViewMode::Guide
        && ui.focus.with(|f| f.layout_pane()) != Pane::Rail
}

/// The corner ‹'s distance from the header's left edge: 20 points into
/// the preview, plus the part of a leading inset `inset` that overlaps the
/// preview, kept inside a preview `preview_w` wide.
fn corner_back_left(inset: f32, preview_w: f32) -> f32 {
    let wanted = 20.0 + (inset - PREVIEW_MARGIN).max(0.0);
    let last = PREVIEW_MARGIN + preview_w - crate::ui_kit::TOUCH;
    wanted.min(last.max(PREVIEW_MARGIN))
}

/// A pointer or touch route to the left columns, over the preview where
/// the full-screen player keeps its ‹: one step of the chain a remote's
/// Back and Left walk, the guide to its groups, the groups to the rail
/// with its names (Search, Settings).
fn corner_back(ui: Ui) -> impl IntoElement {
    let groups = move || ui.focus.with(|f| f.layout_pane()) == Pane::Guide;
    view()
        .test_id("guide-corner-back")
        .absolute()
        // The player's ‹ is 20 and 16 points in from the safe area: the
        // header is below the top inset (no ancestor pads the sides), and
        // the part of the leading one over the preview (an iPad window's
        // controls) moves the ‹ along it clear of them, never past the
        // preview's right edge.
        .left(move || px(corner_back_left(safe_area().left.0, header_now().1)))
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
    fn the_corner_back_clears_a_leading_inset_inside_the_preview() {
        use super::corner_back_left;
        // No inset, or one inside the preview's margin: where it always was.
        assert_eq!(corner_back_left(0.0, 300.0), 20.0);
        assert_eq!(corner_back_left(12.0, 300.0), 20.0);
        // An iPad window's controls: only the part over the preview counts.
        assert_eq!(corner_back_left(65.0, 300.0), 73.0);
        // A large inset beside a narrow preview: the ‹ stays inside it.
        assert_eq!(corner_back_left(120.0, 114.0), 12.0 + 114.0 - 48.0);
    }

    #[test]
    fn the_key_timing_line_is_for_development_only() {
        assert!(!overlay_wanted(false, None), "release builds hide it");
        assert!(!overlay_wanted(false, Some("")));
        assert!(!overlay_wanted(false, Some("0")));
        assert!(overlay_wanted(false, Some("1")), "unless asked for");
        assert!(overlay_wanted(true, None), "debug builds show it");
    }
}

/// Description lookup shared by the header and selected-programme Details.
pub(crate) fn programme_description(
    schedule: &dyn crate::guide::Schedule,
    channel: usize,
    programme: Program,
) -> Option<String> {
    if let Some(combined) = schedule
        .as_any()
        .and_then(|schedule| schedule.downcast_ref::<iptv_guide::combined::CombinedSchedule>())
    {
        combined.description(channel, programme)
    } else {
        schedule
            .as_any()
            .and_then(|schedule| schedule.downcast_ref::<iptv_guide::EpgSchedule>())
            .and_then(|schedule| schedule.description(channel, programme))
    }
}

#[cfg(test)]
mod description_regressions {
    use super::programme_description;
    use iptv_core::guide::Schedule;
    use iptv_epg::{
        ChannelKey, EpgRef, FeedId, GuideStore, MatchKind, ProgrammeData, SourceBuilder,
        StoreOptions,
    };
    use iptv_guide::{
        EpgSchedule, GuideChannel,
        combined::{CombinedSchedule, SourceSchedule},
    };
    use std::rc::Rc;

    fn schedule(now: i64, description: Option<&str>) -> EpgSchedule {
        let key = ChannelKey::new("synthetic-shared-key");
        let mut builder = SourceBuilder::new(
            FeedId(1),
            0,
            StoreOptions {
                store_descriptions: true,
                ..Default::default()
            },
            now,
        );
        builder.push_data(
            "synthetic-epg",
            &ProgrammeData {
                start: 1_800_000_000 - 1800,
                stop: Some(1_800_000_000 + 1800),
                title: "Synthetic selected programme",
                desc: description,
                ..Default::default()
            },
        );
        let mut store = GuideStore::new();
        store.replace_source(builder.finish());
        store.set_matches(std::collections::HashMap::from([(
            key.clone(),
            vec![EpgRef {
                source: FeedId(1),
                epg_id: "synthetic-epg".into(),
                kind: MatchKind::TvgId,
            }],
        )]));
        EpgSchedule::new(
            vec![(
                "Synthetic group".into(),
                vec![GuideChannel {
                    key,
                    name: "Synthetic channel".into(),
                    number: 1,
                    catchup_days: 0,
                }],
            )],
            store,
            now,
        )
    }
    #[test]
    fn descriptions_follow_selected_source_programme_and_empty_data() {
        let now = 1_800_000_000;
        let first = Rc::new(schedule(now, Some("Synthetic amber description")));
        let second = Rc::new(schedule(now + 3600, Some("Synthetic birch description")));
        let selected = first.program_at(0, 0);
        assert_eq!(
            programme_description(&*first, 0, selected).as_deref(),
            Some("Synthetic amber description")
        );
        let combined = CombinedSchedule::new(
            vec![
                SourceSchedule {
                    id: 1,
                    name: "Synthetic first".into(),
                    schedule: first,
                },
                SourceSchedule {
                    id: 2,
                    name: "Synthetic second".into(),
                    schedule: second,
                },
            ],
            now,
        );
        for (channel, expected) in [
            (0, "Synthetic amber description"),
            (1, "Synthetic birch description"),
        ] {
            assert_eq!(
                programme_description(&combined, channel, combined.program_at(channel, 0))
                    .as_deref(),
                Some(expected)
            );
        }
        let empty = schedule(now, None);
        assert_eq!(
            programme_description(&empty, 0, empty.program_at(0, 0)),
            None
        );
        assert_eq!(programme_description(&empty, 9, selected), None);
    }
}
