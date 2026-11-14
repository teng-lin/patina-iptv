//! Shared choice families. Variant keys are stable (stored in backups);
//! their labels come from `settings.choice.<family>.<variant>`. Families
//! that several settings share (sort orders, sizes) use one label family
//! with per-setting subsets.

use crate::schema::Choices;

/// Accent bundles by stable name. The values mirror the names of
/// `ACCENTS` in `crates/iptv-app/src/theme.rs` (keep the two lists in step).
pub static ACCENT: Choices = Choices {
    family: "accent",
    variants: &[
        "amber",
        "black",
        "blue",
        "blue_grey",
        "brown",
        "cyan",
        "green",
        "grey",
        "indigo",
        "lime",
        "orange",
        "pink",
        "purple",
        "red",
        "teal",
        "yellow",
    ],
};

/// Background bundles by stable name; mirrors `BACKGROUNDS` in
/// `crates/iptv-app/src/theme.rs`.
pub static BACKGROUND: Choices = Choices {
    family: "background",
    variants: &[
        "amber",
        "black",
        "blue",
        "blue_grey",
        "brown",
        "cyan",
        "dark",
        "dark_grey",
        "green",
        "grey",
        "indigo",
        "lime",
        "orange",
        "pink",
        "purple",
        "red",
        "teal",
        "yellow",
    ],
};

pub static PLAYLIST_SORT: Choices = Choices {
    family: "sort",
    variants: &["manual", "name"],
};

pub static GROUP_SORT: Choices = Choices {
    family: "sort",
    variants: &["playlist_order", "name", "manual"],
};

pub static CHANNEL_SORT: Choices = Choices {
    family: "sort",
    variants: &["playlist_order", "name", "watch_time", "manual"],
};

pub static VOD_SORT: Choices = Choices {
    family: "sort",
    variants: &[
        "playlist_order",
        "date_added",
        "name",
        "rating",
        "last_modified",
    ],
};

pub static SIZE: Choices = Choices {
    family: "size",
    variants: &["small", "medium", "large", "extra_large"],
};

pub static CLOCK_SIZE: Choices = Choices {
    family: "size",
    variants: &["small", "medium", "large"],
};

pub static LEVEL: Choices = Choices {
    family: "level",
    variants: &["none", "low", "medium", "high"],
};

pub static SELECTION_STYLE: Choices = Choices {
    family: "selection_style",
    variants: &["accent_fill", "neutral_fill"],
};

pub static BROWSE_MODE: Choices = Choices {
    family: "browse_mode",
    variants: &["preview", "overlay"],
};

pub static NOW_HIGHLIGHT: Choices = Choices {
    family: "now_highlight",
    variants: &["plain", "colour", "progress_only"],
};

pub static CORNER: Choices = Choices {
    family: "corner",
    variants: &["top_left", "top_right", "bottom_left", "bottom_right"],
};

pub static LOGO_SOURCE: Choices = Choices {
    family: "logo_source",
    variants: &["playlist", "epg", "folder"],
};

pub static DECODER: Choices = Choices {
    family: "decoder",
    variants: &["hardware", "software"],
};

pub static EXTERNAL_PLAYER: Choices = Choices {
    family: "external_player",
    variants: &["never", "vod_only", "always"],
};

/// `player.pip_on_leave`: `auto` is on phones and tablets, off on a TV.
pub static PIP_ON_LEAVE: Choices = Choices {
    family: "pip_on_leave",
    variants: &["auto", "always", "never"],
};

pub static CATCHUP_SCHEME: Choices = Choices {
    family: "catchup_scheme",
    variants: &["auto", "shift", "append", "flussonic", "xtream", "disabled"],
};

pub static REWIND_LIVE_KEY: Choices = Choices {
    family: "rewind_live_key",
    variants: &["off", "left", "down", "rewind"],
};

pub static SEEK_KEYS: Choices = Choices {
    family: "seek_keys",
    variants: &["left_right", "down_up", "transport"],
};

pub static PLAYER_UP_DOWN: Choices = Choices {
    family: "player_up_down",
    variants: &["switch_channels", "channel_list"],
};

pub static RECORDING_FORMAT: Choices = Choices {
    family: "recording_format",
    variants: &["ts", "mkv"],
};

pub static REMINDER_ACTION: Choices = Choices {
    family: "reminder_action",
    variants: &["ask", "switch_channel", "notify_only"],
};

pub static PIN_SCOPE: Choices = Choices {
    family: "pin_scope",
    variants: &["blocked_content", "blocked_content_and_settings"],
};

pub static UNLOCK_WINDOW: Choices = Choices {
    family: "unlock_window",
    variants: &[
        "every_time",
        "min_10",
        "min_30",
        "hour_1",
        "hour_2",
        "until_restart",
        "until_unblocked_channel",
    ],
};

pub static PIN_INPUT: Choices = Choices {
    family: "pin_input",
    variants: &["keypad", "number_keys"],
};

pub static KEYBOARD: Choices = Choices {
    family: "keyboard",
    variants: &["system", "built_in"],
};

/// The clock times are written on: the locale's own, or one fixed.
pub static CLOCK: Choices = Choices {
    family: "clock",
    variants: &["auto", "h12", "h24"],
};

pub static PLAYLIST_OPENS: Choices = Choices {
    family: "playlist_opens",
    variants: &["all_channels", "favorites"],
};

pub static CARD_STYLE: Choices = Choices {
    family: "card_style",
    variants: &["poster", "landscape", "compact"],
};

/// Every family, for integrity checks.
pub static ALL: &[&Choices] = &[
    &ACCENT,
    &BACKGROUND,
    &PLAYLIST_SORT,
    &GROUP_SORT,
    &CHANNEL_SORT,
    &VOD_SORT,
    &SIZE,
    &CLOCK_SIZE,
    &LEVEL,
    &SELECTION_STYLE,
    &BROWSE_MODE,
    &NOW_HIGHLIGHT,
    &CORNER,
    &LOGO_SOURCE,
    &DECODER,
    &EXTERNAL_PLAYER,
    &PIP_ON_LEAVE,
    &CATCHUP_SCHEME,
    &REWIND_LIVE_KEY,
    &SEEK_KEYS,
    &PLAYER_UP_DOWN,
    &RECORDING_FORMAT,
    &REMINDER_ACTION,
    &PIN_SCOPE,
    &UNLOCK_WINDOW,
    &PIN_INPUT,
    &KEYBOARD,
    &CLOCK,
    &PLAYLIST_OPENS,
    &CARD_STYLE,
];

/// Interface language suggestions. Locale settings remain text in storage so
/// existing backups and future language tags continue to round-trip.
pub static LANGUAGE: Choices = Choices {
    family: "language",
    variants: &[
        "system", "en", "es", "fr", "de", "it", "pt", "ru", "zh", "zh-Hant", "ja", "ko",
    ],
};
