//! The info card (logo, number and name, stream badges, programme, its one
//! time line, next programme) and the compact banner a channel change
//! shows (number and name, programme, progress). The time line is the
//! programme's progress live and the seek bar in a replay
//! (`iptv_core::panels::Line`): the card owns it, nothing else draws one.

use iptv_core::panels::Line;
use iptv_core::player_info::{StreamFormat, badges, minutes_left};
use nsh_core::prelude::*;

use super::WHITE;
use crate::fonts::{OSD_BADGE, OSD_DETAIL, OSD_TEXT, OSD_TITLE};
use crate::guide::{Airing, Program};
use crate::theme::{RADIUS, RADIUS_LARGE, RESTING_ALPHA, TEXT_SECONDARY_ALPHA};
use crate::{Playback, Ui, channel_name, clock, tint};

const LOGO_W: f32 = 112.0;
const LOGO_H: f32 = 63.0;

fn secondary() -> Rgba {
    tint(WHITE, TEXT_SECONDARY_ALPHA)
}

/// The group the playing channel is in (groups start in ascending order).
pub(super) fn group_of(channel: usize) -> String {
    let s = crate::schedule();
    iptv_core::lists::group_of(&*s, channel).map_or_else(String::new, |g| s.group_name(g))
}

/// The channel's logo address (imported channels with an http(s) logo).
fn logo(channel: usize) -> Option<String> {
    logo_in(&*crate::schedule(), channel)
}

/// Read through the trait, so a combined ("All playlists") schedule finds
/// the logo in the playlist that owns the channel. Only http(s) addresses
/// go on to the image loader.
fn logo_in(schedule: &dyn iptv_core::guide::Schedule, channel: usize) -> Option<String> {
    let logo = schedule.channel_logo(channel)?;
    let logo = logo.trim();
    (logo.starts_with("http://") || logo.starts_with("https://")).then(|| logo.to_owned())
}

/// The programme on screen: live now, or the replayed one.
fn programme(ui: Ui) -> (usize, Program) {
    ui.guide_data();
    let channel = ui.playing.get().unwrap_or(0);
    let minute = match ui.playback.get() {
        Playback::Live => ui.focus.with(|f| f.now),
        Playback::CatchUp { start, .. } => start,
    };
    (channel, crate::schedule().program_at(channel, minute))
}

/// How far the programme on screen is, 0–1: where the archive plays (or
/// the seek being chosen), else now in the live programme.
fn progress(ui: Ui) -> f32 {
    if let Some((at, length)) = crate::seek::elapsed(ui) {
        return at as f32 / length.max(1) as f32;
    }
    let (_, p) = programme(ui);
    let now = ui.focus.with(|f| f.now);
    if p.airing(now) == Airing::Live {
        p.progress(now)
    } else {
        0.0
    }
}

/// The one time line's kind now.
fn line_kind(ui: Ui) -> Line {
    let replay = matches!(ui.playback.get(), Playback::CatchUp { .. });
    ui.panels
        .with(|p| p.line(replay))
        .unwrap_or(if replay { Line::Seek } else { Line::Progress })
}

/// Its ends: live, the programme's times and the minutes left; in a
/// replay, the position (⇢ while a seek is chosen) and the length.
fn ends(ui: Ui) -> (String, String) {
    match (line_kind(ui), crate::seek::elapsed(ui)) {
        (Line::Seek, Some((at, length))) => {
            let seeking = if ui.seek_target.get().is_some() {
                "⇢ "
            } else {
                ""
            };
            (
                format!("{seeking}{}", crate::seek::hms(at)),
                crate::seek::hms(length),
            )
        }
        _ => {
            let (_, p) = programme(ui);
            let at = ui.focus.with(|f| f.now);
            let left =
                minutes_left(crate::i18n::language(), p.start, p.end, at).unwrap_or_default();
            (format!("{}–{}", clock(p.start), clock(p.end)), left)
        }
    }
}

/// The time line: a track with the programme's progress; a replay's seek
/// bar has a dot where it plays.
fn time_line(ui: Ui) -> View {
    let accent = move || rgb(ui.theme.with(|t| t.accent.light));
    let dot = move || line_kind(ui) == Line::Seek;
    view()
        .test_id("player-time-line")
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .child(
            text(move || ends(ui).0)
                .text_size(px(OSD_DETAIL))
                .text_color(rgb(WHITE)),
        )
        .child(
            view()
                .flex_1()
                .h(px(12.0))
                .justify_center()
                .child(
                    view()
                        .h(px(4.0))
                        .rounded(px(2.0))
                        .bg(tint(WHITE, RESTING_ALPHA * 2))
                        .child(
                            view()
                                .h_full()
                                .rounded(px(2.0))
                                .w(move || percentage(progress(ui)))
                                .bg(accent),
                        ),
                )
                .child(
                    view()
                        .absolute()
                        .top(px(0.0))
                        .left(move || percentage(progress(ui)))
                        .ml(px(-6.0))
                        .w(px(12.0))
                        .h(px(12.0))
                        .rounded(px(6.0))
                        .bg(rgb(WHITE))
                        .opacity(move || if dot() { 1.0_f32 } else { 0.0 }),
                ),
        )
        .child(
            text(move || ends(ui).1)
                .text_size(px(OSD_DETAIL))
                .text_color(secondary()),
        )
}

fn number_and_name(ui: Ui, channel: usize) -> String {
    // As in the guide: its place in the list on screen.
    format!(
        "{}  {}",
        crate::lists::number(ui, channel),
        channel_name(channel)
    )
}

pub(super) fn info_card(ui: Ui, panel: impl Fn() -> Rgba + Copy + 'static) -> impl IntoElement {
    let channel = move || programme(ui).0;
    let now = move || programme(ui).1;
    let next = move || crate::schedule().program_at(channel(), now().end);
    let format = move || {
        let main = ui.player.main();
        let (width, height) = main.size.get().unwrap_or((0, 0));
        let f = main.format.get();
        StreamFormat { width, height, ..f }
    };
    view()
        .test_id("player-card")
        .flex_row()
        .items_center()
        .gap(px(20.0))
        .px(px(22.0))
        .py(px(16.0))
        .rounded(px(RADIUS_LARGE))
        .bg(panel)
        .child(
            // The logo slot stays when a channel has none.
            view()
                .w(px(LOGO_W))
                .h(px(LOGO_H))
                .rounded(px(RADIUS))
                .bg(tint(WHITE, RESTING_ALPHA))
                .child(
                    image(crate::content_image::source(move || {
                        logo(channel()).unwrap_or_else(crate::grid::no_logo)
                    }))
                    .w_full()
                    .h_full()
                    .object_fit(ResizeMode::Contain),
                ),
        )
        .child(
            view()
                .flex_1()
                .gap(px(5.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            text(move || number_and_name(ui, channel()))
                                .flex_1()
                                .number_of_lines(1)
                                .text_size(px(OSD_DETAIL))
                                .text_color(secondary()),
                        )
                        .children((0..3).map(move |i| badge(ui, format, i))),
                )
                .child(
                    text(move || crate::schedule().title(channel(), now()))
                        .number_of_lines(1)
                        .text_size(px(OSD_TITLE))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(WHITE)),
                )
                .child(time_line(ui))
                .child(
                    text(move || {
                        let p = next();
                        crate::i18n::trf(
                            "Next  {time}  {title}",
                            &[
                                ("time", (&clock(p.start)).into()),
                                ("title", (&crate::schedule().title(channel(), p)).into()),
                            ],
                        )
                    })
                    .number_of_lines(1)
                    .text_size(px(OSD_DETAIL))
                    .text_color(secondary()),
                ),
        )
}

/// The channel-change banner: number and name, the programme on now, its
/// progress and minutes left, in one compact panel.
pub(super) fn banner(ui: Ui, panel: impl Fn() -> Rgba + Copy + 'static) -> impl IntoElement {
    let channel = move || programme(ui).0;
    view()
        .test_id("player-banner")
        .gap(px(6.0))
        .px(px(20.0))
        .py(px(12.0))
        .rounded(px(RADIUS_LARGE))
        .bg(panel)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(14.0))
                .child(
                    text(move || number_and_name(ui, channel()))
                        .number_of_lines(1)
                        .text_size(px(OSD_TEXT))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(WHITE)),
                )
                .child(
                    text(move || crate::schedule().title(channel(), programme(ui).1))
                        .flex_1()
                        .number_of_lines(1)
                        .text_size(px(OSD_TEXT))
                        .text_color(rgb(WHITE)),
                ),
        )
        .child(time_line(ui))
}

/// Stream badge `index` (resolution class, frame rate, audio layout), once
/// the player reported it.
fn badge(
    ui: Ui,
    format: impl Fn() -> StreamFormat + Copy + 'static,
    index: usize,
) -> impl IntoElement {
    let value = move || badges(&format()).get(index).cloned();
    text(move || value().unwrap_or_default())
        .px(move || px(if value().is_some() { 6.0 } else { 0.0 }))
        .py(px(1.0))
        .rounded(px(4.0))
        .text_size(px(OSD_BADGE))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(WHITE))
        .bg(move || {
            let alpha = if value().is_some() {
                RESTING_ALPHA * 3
            } else {
                0
            };
            tint(WHITE, alpha)
        })
        .opacity(move || {
            if ui.playing.get().is_some() {
                1.0_f32
            } else {
                0.0
            }
        })
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use iptv_epg::GuideStore;
    use iptv_guide::EpgSchedule;
    use iptv_guide::combined::{CombinedSchedule, SourceSchedule};

    use super::logo_in;

    const NOW: i64 = 1_800_000_000;

    /// A generated playlist: one channel with an http(s) logo, one with a
    /// logo the image loader must not be handed, one without.
    fn playlist(name: &str) -> EpgSchedule {
        let parsed = iptv_providers::m3u::parse_m3u(&format!(
            "#EXTM3U\n\
             #EXTINF:-1 tvg-logo=\" https://logo.example.test/{name}.png \",{name} One\n\
             http://stream.example.test/{name}/1\n\
             #EXTINF:-1 tvg-logo=\"file:///generated/{name}.png\",{name} Two\n\
             http://stream.example.test/{name}/2\n\
             #EXTINF:-1,{name} Three\n\
             http://stream.example.test/{name}/3\n"
        ));
        EpgSchedule::from_catalog(
            &parsed.groups,
            &parsed.channels.items,
            GuideStore::default(),
            NOW,
        )
    }

    #[test]
    fn a_single_playlist_card_shows_only_http_logos() {
        let schedule = playlist("alpha");
        assert_eq!(
            logo_in(&schedule, 0).as_deref(),
            Some("https://logo.example.test/alpha.png")
        );
        assert_eq!(logo_in(&schedule, 1), None);
        assert_eq!(logo_in(&schedule, 2), None);
        assert_eq!(logo_in(&schedule, 99), None);
    }

    #[test]
    fn a_combined_playlist_card_shows_the_owning_playlists_logo() {
        let part = |id, name: &str| SourceSchedule {
            id,
            name: name.into(),
            schedule: Rc::new(playlist(name)),
        };
        let all = CombinedSchedule::new(vec![part(1, "alpha"), part(2, "beta")], NOW);
        // Channels 0..3 are the first playlist's, 3..6 the second's.
        assert_eq!(
            logo_in(&all, 0).as_deref(),
            Some("https://logo.example.test/alpha.png")
        );
        assert_eq!(
            logo_in(&all, 3).as_deref(),
            Some("https://logo.example.test/beta.png")
        );
        assert_eq!(logo_in(&all, 4), None);
        assert_eq!(logo_in(&all, 5), None);
        assert_eq!(logo_in(&all, 99), None);
    }
}
