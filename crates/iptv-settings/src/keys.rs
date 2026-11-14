//! Typed keys for the settings other crates read in code, so a read cannot
//! ask for the wrong type. The test suite checks each against the schema.
//! Add a constant here when a crate starts reading a setting.

/// A key whose value is a [`Value::Bool`](crate::Value::Bool).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoolKey(pub &'static str);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntKey(pub &'static str);
/// Milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DurationKey(pub &'static str);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnumKey(pub &'static str);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextKey(pub &'static str);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColourKey(pub &'static str);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingsKey(pub &'static str);

pub const LANGUAGE: TextKey = TextKey("general.language");
pub const CLOCK: EnumKey = EnumKey("general.clock");

// Input (iptv-nav).
pub const LONG_PRESS: DurationKey = DurationKey("remote.long_press");
pub const DIGIT_TIMEOUT: DurationKey = DurationKey("remote.digit_timeout");
pub const KEY_OVERRIDES: BindingsKey = BindingsKey("remote.key_overrides");
pub const PLAYER_UP_DOWN: EnumKey = EnumKey("remote.player_up_down");

// Player panels (iptv-core panels::Timeouts).
pub const INFO_PANEL_TIMEOUT: DurationKey = DurationKey("player.info_panel_timeout");
pub const CONTROLS_PANEL_TIMEOUT: DurationKey = DurationKey("player.controls_panel_timeout");

// Channel switching.
pub const SWITCH_DELAY: DurationKey = DurationKey("guide.switch_delay");
pub const FULLSCREEN_TIMEOUT: DurationKey = DurationKey("guide.fullscreen_timeout");
pub const AUTOPLAY: BoolKey = BoolKey("guide.autoplay");
pub const BROWSE_MODE: EnumKey = EnumKey("guide.browse_mode");

// Appearance (iptv-app theme).
pub const ACCENT: ColourKey = ColourKey("appearance.accent");
pub const BACKGROUND: ColourKey = ColourKey("appearance.background");
pub const FONT_SIZE: EnumKey = EnumKey("appearance.font_size");

// Organisation.
pub const CHANNEL_SORT: EnumKey = EnumKey("channels.channel_sort");
pub const GROUP_SORT: EnumKey = EnumKey("channels.group_sort");
pub const RECENT_COUNT: IntKey = IntKey("history.recent_count");
pub const USER_AGENT: TextKey = TextKey("general.user_agent");

// EPG refresh and storage (iptv-import refresh policy).
pub const EPG_UPDATE_INTERVAL: DurationKey = DurationKey("epg.update_interval");
pub const EPG_UPDATE_ON_START: BoolKey = BoolKey("epg.update_on_start");
pub const EPG_UPDATE_ON_PLAYLIST_CHANGE: BoolKey = BoolKey("epg.update_on_playlist_change");
pub const EPG_KEEP_PAST: DurationKey = DurationKey("epg.keep_past");
pub const EPG_STORE_DESCRIPTIONS: BoolKey = BoolKey("epg.store_descriptions");

// Startup and leaving.
pub const RESUME_LAST_CHANNEL: BoolKey = BoolKey("general.resume_last_channel");
pub const CONFIRM_EXIT: BoolKey = BoolKey("general.confirm_exit");
pub const CONFIRM_EXIT_WINDOW: DurationKey = DurationKey("general.confirm_exit_window");

// Appearance over video.
pub const UI_TRANSPARENCY: IntKey = IntKey("appearance.ui_transparency");

// Guide and player behaviour.
pub const BACK_RETURNS_TO_NOW: BoolKey = BoolKey("guide.back_returns_to_now");
pub const INFO_ON_SWITCH: BoolKey = BoolKey("player.info_on_switch");
pub const PIP_ON_LEAVE: EnumKey = EnumKey("player.pip_on_leave");

// Catch-up seeking.
pub const SEEK_KEYS: EnumKey = EnumKey("catchup.seek_keys");
pub const FORWARD_STEP: DurationKey = DurationKey("catchup.forward_step");
pub const REWIND_STEP: DurationKey = DurationKey("catchup.rewind_step");
pub const TIMEBAR_STEP: DurationKey = DurationKey("catchup.timebar_step");

// Search.
pub const SEARCH_FAVORITES_FIRST: BoolKey = BoolKey("search.favorites_first");
pub const SEARCH_INCLUDE_PAST_WITHOUT_CATCHUP: BoolKey =
    BoolKey("search.include_past_without_catchup");
pub const SEARCH_STAY: BoolKey = BoolKey("search.stay_on_search");
pub const SEARCH_SHOW_HISTORY: BoolKey = BoolKey("search.show_history");

/// Every typed key with its expected type name, for the schema test.
pub const ALL: &[(&str, &str)] = &[
    (LONG_PRESS.0, "duration"),
    (DIGIT_TIMEOUT.0, "duration"),
    (KEY_OVERRIDES.0, "key_bindings"),
    (PLAYER_UP_DOWN.0, "enum"),
    (INFO_PANEL_TIMEOUT.0, "duration"),
    (CONTROLS_PANEL_TIMEOUT.0, "duration"),
    (SWITCH_DELAY.0, "duration"),
    (FULLSCREEN_TIMEOUT.0, "duration"),
    (AUTOPLAY.0, "bool"),
    (BROWSE_MODE.0, "enum"),
    (ACCENT.0, "colour"),
    (BACKGROUND.0, "colour"),
    (FONT_SIZE.0, "enum"),
    (CHANNEL_SORT.0, "enum"),
    (GROUP_SORT.0, "enum"),
    (RECENT_COUNT.0, "integer"),
    (USER_AGENT.0, "text"),
    (EPG_UPDATE_INTERVAL.0, "duration"),
    (EPG_UPDATE_ON_START.0, "bool"),
    (EPG_UPDATE_ON_PLAYLIST_CHANGE.0, "bool"),
    (EPG_KEEP_PAST.0, "duration"),
    (EPG_STORE_DESCRIPTIONS.0, "bool"),
    (RESUME_LAST_CHANNEL.0, "bool"),
    (CONFIRM_EXIT.0, "bool"),
    (CONFIRM_EXIT_WINDOW.0, "duration"),
    (UI_TRANSPARENCY.0, "integer"),
    (BACK_RETURNS_TO_NOW.0, "bool"),
    (INFO_ON_SWITCH.0, "bool"),
    (PIP_ON_LEAVE.0, "enum"),
    (SEEK_KEYS.0, "enum"),
    (FORWARD_STEP.0, "duration"),
    (REWIND_STEP.0, "duration"),
    (TIMEBAR_STEP.0, "duration"),
    (SEARCH_FAVORITES_FIRST.0, "bool"),
    (SEARCH_INCLUDE_PAST_WITHOUT_CATCHUP.0, "bool"),
    (SEARCH_STAY.0, "bool"),
    (SEARCH_SHOW_HISTORY.0, "bool"),
];
