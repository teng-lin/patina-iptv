//! History, search, recordings and reminders, parental controls, remote
//! and VOD.
//!
//! Default provenance markers as in `general.rs`. The parental PIN itself
//! is a secret, not a setting: it lives in the platform's secret store.

use super::{choices::*, key};
use crate::schema::*;
use crate::scope::Scope;

const fn history(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::History, sub, key, ty, summary)
}

const fn search(key: &'static str, ty: ValueType, summary: &'static str) -> Setting {
    Setting::new(Section::Search, "", key, ty, summary)
}

const fn recordings(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::Recordings, sub, key, ty, summary)
}

const fn parental(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::Parental, sub, key, ty, summary)
}

const fn remote(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::Remote, sub, key, ty, summary)
}

const fn vod(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::Vod, sub, key, ty, summary)
}

pub static HISTORY: &[Setting] = &[
    // default: ours
    history(
        "history",
        "history.keep_days",
        days(1, 90, 1, 30),
        key("Watched channels and programs are forgotten after this many days."),
    ),
    // default: ours
    history(
        "history",
        "history.add_after",
        secs(0, 600, 5, 60),
        key("A channel enters history only after playing this long."),
    ),
    // default: ours
    history(
        "recent",
        "history.recent_count",
        int(1, 20, 1, IntUnit::Count, 10),
        key("Length of the quick-switch list of recent channels."),
    ),
    // default: ours
    history(
        "recent",
        "history.recent_add_after",
        secs(0, 120, 1, 10),
        key("A channel enters the recent list only after playing this long."),
    ),
];

pub static SEARCH: &[Setting] = &[
    // default: ours
    search(
        "search.prefer_voice",
        boolean(false),
        key("Open search with voice input focused instead of the text field."),
    ),
    // Retired (#282): favorites always rank first. Not offered or read; a
    // stored value is kept.
    search(
        "search.favorites_first",
        boolean(true),
        key("Rank favorite channels above other matches."),
    ),
    // default: ours
    search(
        "search.show_history",
        boolean(true),
        key("Offer earlier queries before typing starts."),
    ),
    // Retired (#282): the results offer this as a filter instead. Not
    // offered or read; a stored value is kept.
    search(
        "search.include_past_without_catchup",
        boolean(false),
        key("Also match finished programs that cannot be replayed."),
    ),
    // Retired (#282): Back from a chosen result always returns to the
    // results. Not offered or read; a stored value is kept.
    search(
        "search.stay_on_search",
        boolean(false),
        key("Return to the results, keeping query and focus, after choosing a channel."),
    ),
];

pub static RECORDINGS: &[Setting] = &[
    // default: ours — empty means the app's own storage
    recordings(
        "recording",
        "recordings.folder",
        text(TextKind::Path, 1024, ""),
        key("Where recordings are written; empty uses app storage."),
    ),
    // default: ours
    recordings(
        "recording",
        "recordings.format",
        choice(&RECORDING_FORMAT, "ts"),
        key("Container recordings are saved in."),
    ),
    // default: ours
    recordings(
        "recording",
        "recordings.lead_padding",
        mins(0, 60, 1, 1),
        key("Start a program recording this early."),
    ),
    // default: ours
    recordings(
        "recording",
        "recordings.trail_padding",
        mins(0, 120, 1, 5),
        key("Keep recording this long after the program's scheduled end."),
    ),
    // default: ours
    recordings(
        "reminders",
        "recordings.reminder_lead",
        mins(0, 60, 1, 5),
        key("A reminder fires this long before its program starts."),
    ),
    // default: ours
    recordings(
        "reminders",
        "recordings.reminder_action",
        choice(&REMINDER_ACTION, "ask"),
        key("What a due reminder does: ask, tune straight away, or only notify."),
    ),
    // default: ours
    recordings(
        "reminders",
        "recordings.reminder_popup_timeout",
        secs(5, 120, 5, 30),
        key("How long a reminder prompt waits before dismissing itself."),
    )
    .depends(&[visible_if(&[when(
        "recordings.reminder_action",
        Test::EnumIn(&["ask", "notify_only"]),
    )])]),
    // default: ours
    recordings(
        "reminders",
        "recordings.reminder_wake",
        boolean(false),
        key("Wake the device from standby for a reminder, where the platform allows it."),
    ),
];

const IF_PARENTAL: &[Dependency] = &[visible_if(&[
    when("parental.live_enabled", Test::IsTrue),
    when("parental.vod_enabled", Test::IsTrue),
])];

pub static PARENTAL: &[Setting] = &[
    // default: ours
    parental(
        "scope",
        "parental.live_enabled",
        boolean(false),
        key("Blocked live channels and groups need the PIN."),
    ),
    // default: ours
    parental(
        "scope",
        "parental.vod_enabled",
        boolean(false),
        key("Blocked movies and series need the PIN."),
    ),
    // default: ours
    parental(
        "scope",
        "parental.require_pin_for",
        choice(&PIN_SCOPE, "blocked_content"),
        key("What the PIN guards: blocked content only, or settings as well."),
    )
    .depends(IF_PARENTAL),
    // Retired (#282): after digits typed on number keys, OK on a digit
    // key in the PIN prompt submits; after keypad presses it presses the
    // focused key (Continue submits), whatever this says. Not offered or
    // read; a stored value is kept.
    parental(
        "scope",
        "parental.pin_input",
        choice(&PIN_INPUT, "keypad"),
        key("Enter the PIN on an on-screen keypad or with the remote's number keys."),
    )
    .depends(IF_PARENTAL),
    // default: ours
    parental(
        "unlock",
        "parental.unlock_window",
        choice(&UNLOCK_WINDOW, "every_time"),
        key("How long a correct PIN keeps blocked content unlocked."),
    )
    .depends(IF_PARENTAL),
];

pub static REMOTE: &[Setting] = &[
    // default: ours (Android's long-press timeout; iptv-nav DEFAULT_LONG_PRESS_MS)
    remote(
        "timing",
        "remote.long_press",
        dur(TimeUnit::Millis, 200, 2_000, 50, 400),
        key("Hold a key this long for its long-press action."),
    ),
    // default: ours
    remote(
        "timing",
        "remote.digit_timeout",
        dur(TimeUnit::Millis, 500, 5_000, 100, 1_500),
        key("Typed channel numbers are committed after this pause."),
    ),
    // default: the reference's (Up/Down switch channels in full screen)
    remote(
        "mapping",
        "remote.player_up_down",
        choice(&PLAYER_UP_DOWN, "switch_channels"),
        key(
            "What Up and Down do over full-screen video: switch channels or open the channel list.",
        ),
    ),
    // default: ours — empty: only the built-in map applies
    remote(
        "mapping",
        "remote.key_overrides",
        bindings(256),
        key("User changes to the key → action map, applied over the built-in defaults."),
    ),
];

pub static VOD: &[Setting] = &[
    // default: ours
    vod(
        "catalog",
        "vod.movies_by_category",
        boolean(true),
        key("Split the full movie list into its categories."),
    ),
    // default: ours
    vod(
        "catalog",
        "vod.shows_by_category",
        boolean(true),
        key("Split the full series list into its categories."),
    ),
    // default: ours
    vod(
        "catalog",
        "vod.card_style",
        choice(&CARD_STYLE, "poster"),
        key("Shape of movie and series tiles."),
    ),
    // default: ours. The catalog's Sort writes it (#282); the settings
    // screen no longer offers it.
    vod(
        "catalog",
        "vod.sort",
        choice(&VOD_SORT, "name"),
        key("Order of movies and series; a playlist may override it."),
    )
    .scope(Scope::Playlist),
    // default: ours
    vod(
        "playback",
        "vod.autoplay_next_episode",
        boolean(true),
        key("Continue with the next episode when one ends."),
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The tables are built at compile time; this runs the helpers to
    /// check that each files its settings under its own section and adds
    /// nothing the row does not ask for.
    #[test]
    fn helpers_build_plain_settings_in_their_section() {
        let ty = boolean(true);
        let built = [
            (history("recent", "history.x", ty, "S."), Section::History),
            (search("search.x", ty, "S."), Section::Search),
            (
                recordings("", "recordings.x", ty, "S."),
                Section::Recordings,
            ),
            (parental("scope", "parental.x", ty, "S."), Section::Parental),
            (remote("timing", "remote.x", ty, "S."), Section::Remote),
            (vod("catalog", "vod.x", ty, "S."), Section::Vod),
        ];
        for (s, section) in built {
            assert_eq!(s.section, section);
            assert_eq!((s.scope, s.flags), (Scope::Global, Flags::default()));
            assert!(s.depends.is_empty());
            assert_eq!(s.ty, ty);
        }
        let subsections = built.map(|(s, _)| s.subsection);
        assert_eq!(
            subsections,
            [
                Some("recent"),
                None,
                None,
                Some("scope"),
                Some("timing"),
                Some("catalog")
            ]
        );
    }
}
