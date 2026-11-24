//! Typed keys for the settings other crates read in code, so a read cannot
//! ask for the wrong type. The test suite checks each against the schema.
//! Add a constant here when a crate starts reading a setting.

/// A key whose value is a [`Value::Bool`](crate::Value::Bool).
/// ```compile_fail
/// let forged = iptv_settings::keys::BoolKey("guide.switch_delay");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoolKey(pub(crate) &'static str);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntKey(pub(crate) &'static str);
/// Milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DurationKey(pub(crate) &'static str);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnumKey(pub(crate) &'static str);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextKey(pub(crate) &'static str);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorKey(pub(crate) &'static str);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingsKey(pub(crate) &'static str);

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
pub const ACCENT: ColorKey = ColorKey("appearance.accent");
pub const BACKGROUND: ColorKey = ColorKey("appearance.background");
pub const FONT_SIZE: EnumKey = EnumKey("appearance.font_size");

// Organization.
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
/// Whether the groups column offers the Favorites category.
pub const SHOW_FAVORITES: BoolKey = BoolKey("channels.show_favorites");
pub const CONFIRM_EXIT: BoolKey = BoolKey("general.confirm_exit");
pub const CONFIRM_EXIT_WINDOW: DurationKey = DurationKey("general.confirm_exit_window");

// Appearance over video.
pub const UI_TRANSPARENCY: IntKey = IntKey("appearance.ui_transparency");

// Guide and player behavior.
pub const BACK_RETURNS_TO_NOW: BoolKey = BoolKey("guide.back_returns_to_now");
pub const INFO_ON_SWITCH: BoolKey = BoolKey("player.info_on_switch");
/// A vertical swipe over full-screen live video changes channel (#285).
pub const SWIPE_TO_ZAP: BoolKey = BoolKey("playback.swipe_to_zap");
pub const PIP_ON_LEAVE: EnumKey = EnumKey("player.pip_on_leave");
/// Multiview: focusing a tile makes it heard (#179).
pub const SOUND_FOLLOWS_TILE: BoolKey = BoolKey("player.sound_follows_tile");

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
    (LANGUAGE.0, "text"),
    (CLOCK.0, "enum"),
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
    (ACCENT.0, "color"),
    (BACKGROUND.0, "color"),
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
    (SHOW_FAVORITES.0, "bool"),
    (CONFIRM_EXIT.0, "bool"),
    (CONFIRM_EXIT_WINDOW.0, "duration"),
    (UI_TRANSPARENCY.0, "integer"),
    (BACK_RETURNS_TO_NOW.0, "bool"),
    (INFO_ON_SWITCH.0, "bool"),
    (SWIPE_TO_ZAP.0, "bool"),
    (PIP_ON_LEAVE.0, "enum"),
    (SOUND_FOLLOWS_TILE.0, "bool"),
    (SEEK_KEYS.0, "enum"),
    (FORWARD_STEP.0, "duration"),
    (REWIND_STEP.0, "duration"),
    (TIMEBAR_STEP.0, "duration"),
    (SEARCH_FAVORITES_FIRST.0, "bool"),
    (SEARCH_INCLUDE_PAST_WITHOUT_CATCHUP.0, "bool"),
    (SEARCH_STAY.0, "bool"),
    (SEARCH_SHOW_HISTORY.0, "bool"),
    (PLAYLISTS_INCLUDE_VOD.0, "bool"),
    (PARENTAL_UNLOCK_WINDOW.0, "enum"),
    (PARENTAL_LIVE_ENABLED.0, "bool"),
    (PARENTAL_VOD_ENABLED.0, "bool"),
    (PARENTAL_REQUIRE_PIN_FOR.0, "enum"),
    (GUIDE_LIST_BROWSE_MODE.0, "enum"),
    (GUIDE_SHOW_CHANNEL_NUMBERS.0, "bool"),
    (PARENTAL_PIN_INPUT.0, "enum"),
    (GUIDE_VISIBLE_MINUTES.0, "integer"),
    (VOD_SORT.0, "enum"),
    (VOD_AUTOPLAY_NEXT_EPISODE.0, "bool"),
    (GUIDE_SHOW_LOGOS.0, "bool"),
    (GUIDE_STAY_ON_GUIDE.0, "bool"),
    (GUIDE_STAY_ON_LIST.0, "bool"),
];

pub const PLAYLISTS_INCLUDE_VOD: BoolKey = BoolKey("playlists.include_vod");

pub const PARENTAL_UNLOCK_WINDOW: EnumKey = EnumKey("parental.unlock_window");

pub const PARENTAL_LIVE_ENABLED: BoolKey = BoolKey("parental.live_enabled");

pub const PARENTAL_VOD_ENABLED: BoolKey = BoolKey("parental.vod_enabled");

pub const PARENTAL_REQUIRE_PIN_FOR: EnumKey = EnumKey("parental.require_pin_for");

pub const GUIDE_LIST_BROWSE_MODE: EnumKey = EnumKey("guide.list_browse_mode");

pub const GUIDE_SHOW_CHANNEL_NUMBERS: BoolKey = BoolKey("guide.show_channel_numbers");

pub const PARENTAL_PIN_INPUT: EnumKey = EnumKey("parental.pin_input");

pub const GUIDE_VISIBLE_MINUTES: IntKey = IntKey("guide.visible_minutes");

pub const VOD_SORT: EnumKey = EnumKey("vod.sort");

pub const VOD_AUTOPLAY_NEXT_EPISODE: BoolKey = BoolKey("vod.autoplay_next_episode");

pub const GUIDE_SHOW_LOGOS: BoolKey = BoolKey("guide.show_logos");

impl BoolKey {
    /// Resolve a catalog key, rejecting unknown keys and wrong value types.
    pub fn try_new(key: &'static str) -> Result<Self, crate::SettingsError> {
        let setting = crate::catalog::setting(key)
            .ok_or_else(|| crate::SettingsError::UnknownKey(key.to_owned()))?;
        let found = setting.ty.type_name();
        if found != "bool" {
            return Err(crate::SettingsError::TypeMismatch {
                key,
                expected: "bool",
                found,
            });
        }
        Ok(Self(key))
    }
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl IntKey {
    /// Resolve a catalog key, rejecting unknown keys and wrong value types.
    pub fn try_new(key: &'static str) -> Result<Self, crate::SettingsError> {
        let setting = crate::catalog::setting(key)
            .ok_or_else(|| crate::SettingsError::UnknownKey(key.to_owned()))?;
        let found = setting.ty.type_name();
        if found != "integer" {
            return Err(crate::SettingsError::TypeMismatch {
                key,
                expected: "integer",
                found,
            });
        }
        Ok(Self(key))
    }
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl DurationKey {
    /// Resolve a catalog key, rejecting unknown keys and wrong value types.
    pub fn try_new(key: &'static str) -> Result<Self, crate::SettingsError> {
        let setting = crate::catalog::setting(key)
            .ok_or_else(|| crate::SettingsError::UnknownKey(key.to_owned()))?;
        let found = setting.ty.type_name();
        if found != "duration" {
            return Err(crate::SettingsError::TypeMismatch {
                key,
                expected: "duration",
                found,
            });
        }
        Ok(Self(key))
    }
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl EnumKey {
    /// Resolve a catalog key, rejecting unknown keys and wrong value types.
    pub fn try_new(key: &'static str) -> Result<Self, crate::SettingsError> {
        let setting = crate::catalog::setting(key)
            .ok_or_else(|| crate::SettingsError::UnknownKey(key.to_owned()))?;
        let found = setting.ty.type_name();
        if found != "enum" {
            return Err(crate::SettingsError::TypeMismatch {
                key,
                expected: "enum",
                found,
            });
        }
        Ok(Self(key))
    }
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl TextKey {
    /// Resolve a catalog key, rejecting unknown keys and wrong value types.
    pub fn try_new(key: &'static str) -> Result<Self, crate::SettingsError> {
        let setting = crate::catalog::setting(key)
            .ok_or_else(|| crate::SettingsError::UnknownKey(key.to_owned()))?;
        let found = setting.ty.type_name();
        if found != "text" {
            return Err(crate::SettingsError::TypeMismatch {
                key,
                expected: "text",
                found,
            });
        }
        Ok(Self(key))
    }
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl ColorKey {
    /// Resolve a catalog key, rejecting unknown keys and wrong value types.
    pub fn try_new(key: &'static str) -> Result<Self, crate::SettingsError> {
        let setting = crate::catalog::setting(key)
            .ok_or_else(|| crate::SettingsError::UnknownKey(key.to_owned()))?;
        let found = setting.ty.type_name();
        if found != "color" {
            return Err(crate::SettingsError::TypeMismatch {
                key,
                expected: "color",
                found,
            });
        }
        Ok(Self(key))
    }
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl BindingsKey {
    /// Resolve a catalog key, rejecting unknown keys and wrong value types.
    pub fn try_new(key: &'static str) -> Result<Self, crate::SettingsError> {
        let setting = crate::catalog::setting(key)
            .ok_or_else(|| crate::SettingsError::UnknownKey(key.to_owned()))?;
        let found = setting.ty.type_name();
        if found != "key_bindings" {
            return Err(crate::SettingsError::TypeMismatch {
                key,
                expected: "key_bindings",
                found,
            });
        }
        Ok(Self(key))
    }
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

pub const GUIDE_STAY_ON_GUIDE: BoolKey = BoolKey("guide.stay_on_guide");

pub const GUIDE_STAY_ON_LIST: BoolKey = BoolKey("guide.stay_on_list");
