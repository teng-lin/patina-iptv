//! EPG, appearance, guide/list presentation and logos.
//!
//! Default provenance markers as in `general.rs`; `// default: declared
//! (06)` is behaviour the reference documents as its default
//! (`06-defaults-and-input.md`) without a recovered value.

use super::{choices::*, key};
use crate::schema::*;
use crate::scope::Scope;

const fn epg(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::Epg, sub, key, ty, summary)
}

const fn appearance(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::Appearance, sub, key, ty, summary)
}

const fn guide(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::Guide, sub, key, ty, summary)
}

const fn logos(key: &'static str, ty: ValueType, summary: &'static str) -> Setting {
    Setting::new(Section::Logos, "", key, ty, summary)
}

pub static EPG: &[Setting] = &[
    // default: ours
    epg(
        "updates",
        "epg.update_interval",
        hours(0, 168, 1, 24),
        key("Download guide data again after this long; 0 updates only on request."),
    ),
    // default: ours
    epg(
        "updates",
        "epg.update_on_start",
        boolean(true),
        key("Fetch guide data when the app starts if the interval has passed."),
    ),
    // default: ours
    epg(
        "updates",
        "epg.update_on_playlist_change",
        boolean(true),
        key("Fetch guide data after a playlist is added, changed or refreshed."),
    ),
    // default: ours — a week, so the guide reaches back as far as the
    // usual catch-up archives (3 to 7 days) and replays can be chosen.
    epg(
        "storage",
        "epg.keep_past",
        days(0, 14, 1, 7),
        key(
            "How many days of finished programmes stay in the guide (for catch-up and browsing back).",
        ),
    ),
    // default: ours
    epg(
        "storage",
        "epg.store_descriptions",
        boolean(true),
        key("Keep programme descriptions; turning this off saves storage on large guides."),
    ),
    // default: declared (06) — partial scan unless a full scan is chosen
    epg(
        "matching",
        "epg.full_scan",
        boolean(false),
        key("Match guide data against every channel instead of a quick partial pass; slower."),
    )
    .advanced(),
    // default: ours
    epg(
        "matching",
        "epg.time_offset",
        offset_hm(-720, 840, 15, 0),
        key("Shift a playlist's guide times when the source's time zone is wrong."),
    )
    .scope(Scope::Playlist),
];

pub static APPEARANCE: &[Setting] = &[
    // default: observed (07) — accent Blue
    appearance(
        "colours",
        "appearance.accent",
        colour(&ACCENT, "blue"),
        key("Accent bundle used for focus, selection and progress."),
    ),
    // default: observed (07) — background Dark
    appearance(
        "colours",
        "appearance.background",
        colour(&BACKGROUND, "dark"),
        key("Background bundle for panels and windows."),
    ),
    // default: ours
    appearance(
        "colours",
        "appearance.selection_style",
        choice(&SELECTION_STYLE, "accent_fill"),
        key("Whether the focused item is filled with the accent or a neutral tone."),
    ),
    // default: ours
    appearance(
        "colours",
        "appearance.ui_transparency",
        int(0, 80, 5, IntUnit::Percent, 20),
        key("How much video shows through browsing surfaces drawn over it."),
    ),
    // default: ours
    appearance(
        "colours",
        "appearance.panel_transparency",
        choice(&LEVEL, "low"),
        key("See-through level of the player's info and control panels."),
    ),
    // default: observed (07) — Medium
    appearance(
        "text",
        "appearance.font_size",
        choice(&SIZE, "medium"),
        key("Text scale across the interface."),
    ),
    // default: ours
    appearance(
        "text",
        "appearance.two_line_channel_names",
        boolean(false),
        key("Wrap long channel names onto a second line instead of truncating."),
    ),
    // default: ours
    appearance(
        "text",
        "appearance.two_line_program_titles",
        boolean(false),
        key("Wrap long programme titles onto a second line instead of truncating."),
    ),
    // default: ours
    appearance(
        "motion",
        "appearance.animated_scrolling",
        boolean(true),
        key("Animate list and guide scrolling; off jumps directly."),
    ),
    // default: ours
    appearance(
        "motion",
        "appearance.animated_transitions",
        boolean(true),
        key("Animate screens and panels as they open and close."),
    ),
];

pub static GUIDE: &[Setting] = &[
    guide(
        "content",
        "guide.visible_minutes",
        int(60, 360, 30, IntUnit::None, 150),
        key("Minutes visible across the programme grid; navigation retains its focused programme."),
    ),
    guide(
        "content",
        "guide.show_logos",
        boolean(true),
        key("Show channel logos in the guide column."),
    ),
    // default: observed (07) — the guide opened in preview mode
    guide(
        "browsing",
        "guide.browse_mode",
        choice(&BROWSE_MODE, "preview"),
        key(
            "Guide layout: video in a preview window beside the grid, or the grid over full-screen video.",
        ),
    ),
    // default: ours
    guide(
        "browsing",
        "guide.list_browse_mode",
        choice(&BROWSE_MODE, "overlay"),
        key("Channel list layout: preview window or over full-screen video."),
    ),
    // default: ours
    guide(
        "content",
        "guide.show_channel_names",
        boolean(true),
        key("Channel rows carry the channel's name."),
    ),
    // default: ours
    guide(
        "content",
        "guide.show_channel_numbers",
        boolean(true),
        key("Channel rows carry the channel's number."),
    ),
    // default: ours
    guide(
        "content",
        "guide.show_now_playing",
        boolean(true),
        key("Show the programme on air under each channel in the channel list."),
    ),
    // default: ours
    guide(
        "content",
        "guide.show_programmes",
        boolean(true),
        key("Show programme cells; off leaves a plain channel list."),
    ),
    // default: ours
    guide(
        "content",
        "guide.show_description",
        boolean(true),
        key("Show the focused programme's description in the details area."),
    ),
    // default: ours
    guide(
        "content",
        "guide.show_source_labels",
        boolean(false),
        key("Show which playlist and group a channel comes from."),
    ),
    // default: ours
    guide(
        "content",
        "guide.show_catchup_badge",
        boolean(true),
        key("Mark channels whose past programmes can be replayed."),
    ),
    // default: ours
    guide(
        "emphasis",
        "guide.dim_past",
        boolean(true),
        key("Draw finished programmes with less contrast."),
    ),
    // default: ours
    guide(
        "emphasis",
        "guide.highlight_playing",
        boolean(true),
        key("Colour the channel that is playing, independently of focus."),
    ),
    // default: ours
    guide(
        "emphasis",
        "guide.highlight_now",
        boolean(true),
        key("Emphasise programmes that are on air."),
    ),
    // default: ours
    guide(
        "emphasis",
        "guide.now_highlight_style",
        choice(&NOW_HIGHLIGHT, "colour"),
        key("How on-air programmes are emphasised: plain, tinted, or only by their progress bar."),
    )
    .depends(&[visible_if(&[when("guide.highlight_now", Test::IsTrue)])]),
    // default: ours
    guide(
        "emphasis",
        "guide.now_line_full_height",
        boolean(false),
        key("Extend the current-time line across the whole grid, not just the time bar."),
    ),
    // default: ours
    guide(
        "switching",
        "guide.autoplay",
        boolean(false),
        key("Start playing the focused channel after the switch delay, without OK."),
    ),
    // default: ours
    guide(
        "switching",
        "guide.switch_delay",
        dur(TimeUnit::Seconds, 0, 5_000, 250, 500),
        key(
            "Wait before tuning after a channel change, so fast zapping skips channels; cancelable.",
        ),
    ),
    // default: ours
    guide(
        "switching",
        "guide.fullscreen_timeout",
        secs(0, 60, 1, 0),
        key("Go full screen after a preview has played this long without input; 0 never does."),
    ),
    // default: ours
    guide(
        "switching",
        "guide.stay_on_list",
        boolean(false),
        key("Keep the channel list open after choosing a channel."),
    ),
    // default: ours
    guide(
        "switching",
        "guide.stay_on_guide",
        boolean(false),
        key("Keep the guide open after choosing a channel."),
    ),
    // default: ours
    guide(
        "switching",
        "guide.back_returns_to_now",
        boolean(true),
        key("Back in a guide scrolled away from now first returns to the programmes on air."),
    ),
];

pub static LOGOS: &[Setting] = &[
    // default: ours
    logos(
        "logos.source",
        choice(&LOGO_SOURCE, "playlist"),
        key("Preferred logo source when several have one; a playlist may override it."),
    )
    .scope(Scope::Playlist),
    // default: ours
    logos(
        "logos.folder",
        text(TextKind::Path, 1024, ""),
        key("Folder of logo images matched to channels by file name; empty for none."),
    ),
    // default: ours
    logos(
        "logos.inexact_match",
        boolean(false),
        key("Match logo files whose names only approximately equal the channel name."),
    )
    .depends(&[visible_if(&[when("logos.folder", Test::NotEmpty)])]),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The tables are built at compile time; this runs the helpers to
    /// check that each files its settings under its own section and adds
    /// nothing the row does not ask for.
    #[test]
    fn helpers_build_plain_settings_in_their_section() {
        let ty = boolean(false);
        let built = [
            (epg("updates", "epg.x", ty, "S."), Section::Epg),
            (
                appearance("text", "appearance.x", ty, "S."),
                Section::Appearance,
            ),
            (guide("", "guide.x", ty, "S."), Section::Guide),
            (logos("logos.x", ty, "S."), Section::Logos),
        ];
        for (s, section) in built {
            assert_eq!(s.section, section);
            assert_eq!((s.scope, s.flags), (Scope::Global, Flags::default()));
            assert!(s.depends.is_empty());
            assert_eq!(s.ty, ty);
        }
        let subsections = built.map(|(s, _)| s.subsection);
        assert_eq!(subsections, [Some("updates"), Some("text"), None, None]);
    }
}
