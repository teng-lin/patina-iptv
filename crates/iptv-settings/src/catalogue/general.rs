//! General, playlists and channel organisation.
//!
//! Default provenance: `// default: ours` is iptv-rs's choice;
//! `// default: observed (07)` is the value the reference player displayed
//! on a fresh install, observed at run time on Android TV.

use super::choices::*;
use crate::schema::*;
use crate::scope::Scope;

const fn general(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::General, sub, key, ty, summary)
}

const fn playlists(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::Playlists, sub, key, ty, summary)
}

const fn channels(
    sub: &'static str,
    key: &'static str,
    ty: ValueType,
    summary: &'static str,
) -> Setting {
    Setting::new(Section::Channels, sub, key, ty, summary)
}

pub static GENERAL: &[Setting] = &[
    // default: ours
    general(
        "startup",
        "general.start_on_boot",
        boolean(false),
        "Launch the app once the device has booted, where the platform permits it.",
    ),
    // default: ours
    general(
        "startup",
        "general.start_on_wake",
        boolean(false),
        "Bring the app back when the device leaves standby; not every device reports this.",
    ),
    // default: ours
    general(
        "startup",
        "general.resume_last_channel",
        boolean(true),
        "Start playing the channel that was on when the app was last closed.",
    ),
    // default: ours
    general(
        "startup",
        "general.confirm_exit",
        boolean(true),
        "Leaving the app from the top level needs Back twice in a short window.",
    ),
    // default: ours
    general(
        "startup",
        "general.confirm_exit_window",
        dur(TimeUnit::Millis, 500, 5_000, 100, 2_000),
        "How long the second Back press may take to count as confirmation.",
    )
    .advanced()
    .depends(&[visible_if(&[when("general.confirm_exit", Test::IsTrue)])]),
    // default: observed (07)
    general(
        "language",
        "general.language",
        text(TextKind::Locale, 35, "system"),
        "Interface language: `system` follows the device, otherwise a language tag.",
    ),
    // default: ours
    general(
        "language",
        "general.clock",
        choice(&CLOCK, "auto"),
        "Clock for times of day: `auto` follows the language's convention, or 12-hour, or 24-hour.",
    ),
    // default: ours
    general(
        "language",
        "general.keyboard",
        choice(&KEYBOARD, "system"),
        "Which on-screen keyboard text fields open: the platform's or the app's own.",
    ),
    // default: observed (07) — not set
    general(
        "network",
        "general.user_agent",
        text(TextKind::Plain, 512, ""),
        "HTTP User-Agent for provider requests; empty sends the app's own.",
    )
    .scope(Scope::Playlist),
    // default: observed (07) — not set
    general(
        "network",
        "general.udp_proxy",
        text(TextKind::HostPort, 261, ""),
        "Relay that turns multicast UDP streams into HTTP, as host:port; empty plays UDP directly.",
    )
    .advanced(),
    // default: ours
    general(
        "privacy",
        "general.usage_statistics",
        boolean(false),
        "Share anonymous usage counts. Off unless the user opts in.",
    ),
    // default: ours
    general(
        "updates",
        "general.auto_update",
        boolean(false),
        "Install new versions automatically where the app distributes its own updates.",
    ),
    // default: ours
    general(
        "updates",
        "general.beta_updates",
        boolean(false),
        "Offer pre-release builds when checking for updates.",
    )
    .advanced(),
];

pub static PLAYLISTS: &[Setting] = &[
    // default: ours
    playlists(
        "content",
        "playlists.include_live",
        boolean(true),
        "Import live channels from a playlist. The global value applies to new playlists.",
    )
    .scope(Scope::Playlist),
    // default: ours
    playlists(
        "content",
        "playlists.include_vod",
        boolean(true),
        "Import movies and series from a playlist that offers them.",
    )
    .scope(Scope::Playlist),
    // default: ours
    playlists(
        "content",
        "playlists.max_connections",
        int(1, 20, 1, IntUnit::Count, 1),
        "Streams the provider allows at once; limits multiview and background recording.",
    )
    .scope(Scope::Playlist)
    .advanced(),
    // default: ours
    playlists(
        "content",
        "playlists.sort",
        choice(&PLAYLIST_SORT, "manual"),
        "Order of playlists in lists that show several.",
    ),
    // default: ours
    playlists(
        "updates",
        "playlists.update_interval",
        hours(0, 168, 1, 24),
        "Refresh a playlist after this long; 0 refreshes only on request.",
    )
    .scope(Scope::Playlist),
    // default: ours
    playlists(
        "updates",
        "playlists.update_on_start",
        boolean(false),
        "Refresh playlists every time the app starts, regardless of the interval.",
    )
    .scope(Scope::Playlist),
];

pub static CHANNELS: &[Setting] = &[
    // default: ours
    channels(
        "sorting",
        "channels.group_sort",
        choice(&GROUP_SORT, "playlist_order"),
        "Order of groups; a playlist may override it.",
    )
    .scope(Scope::Playlist),
    // default: observed (07)
    channels(
        "sorting",
        "channels.channel_sort",
        choice(&CHANNEL_SORT, "playlist_order"),
        "Order of channels; a playlist or a single group may override it.",
    )
    .scope(Scope::Group),
    // default: ours
    channels(
        "categories",
        "channels.show_new_groups",
        boolean(true),
        "Groups that appear after a refresh start visible rather than hidden.",
    ),
    // default: ours
    channels(
        "categories",
        "channels.show_all_channels",
        boolean(true),
        "Offer a category with every channel of the playlist.",
    ),
    // default: ours
    channels(
        "categories",
        "channels.show_favorites",
        boolean(true),
        "Offer a category with the favourite channels.",
    ),
    // default: ours
    channels(
        "categories",
        "channels.show_all_playlists",
        boolean(true),
        "Offer a category that merges the channels of every playlist.",
    ),
    // default: ours
    channels(
        "categories",
        "channels.group_all_playlists_by_playlist",
        boolean(false),
        "Inside the merged category, keep each playlist's channels together.",
    )
    .depends(&[visible_if(&[when(
        "channels.show_all_playlists",
        Test::IsTrue,
    )])]),
    // default: ours
    channels(
        "categories",
        "channels.playlist_opens",
        choice(&PLAYLIST_OPENS, "all_channels"),
        "Which category opens when a playlist is chosen.",
    ),
    // default: ours
    channels(
        "categories",
        "channels.show_hidden_group_channels",
        boolean(false),
        "Still list channels whose group is hidden (in the all-channel categories).",
    ),
    // default: ours
    channels(
        "names",
        "channels.strip_prefixes",
        text(TextKind::CommaList, 512, ""),
        "Comma-separated fragments removed from the start of channel names.",
    )
    .scope(Scope::Playlist),
    // default: ours
    channels(
        "names",
        "channels.strip_suffixes",
        text(TextKind::CommaList, 512, ""),
        "Comma-separated fragments removed from the end of channel names.",
    )
    .scope(Scope::Playlist),
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
            (general("startup", "general.x", ty, "S."), Section::General),
            (playlists("", "playlists.x", ty, "S."), Section::Playlists),
            (channels("names", "channels.x", ty, "S."), Section::Channels),
        ];
        for (s, section) in built {
            assert_eq!(s.section, section);
            assert_eq!((s.scope, s.flags), (Scope::Global, Flags::default()));
            assert!(s.depends.is_empty());
            assert_eq!(s.ty, ty);
        }
        let subsections = built.map(|(s, _)| s.subsection);
        assert_eq!(subsections, [Some("startup"), None, Some("names")]);
    }
}
