//! The catalogue's contract: every setting with its documented type,
//! default, range, scope and flags; every visibility condition; every
//! command. Each setting also round-trips through the store and the
//! backup format.

use iptv_settings::catalogue::{self, choices as c};
use iptv_settings::schema::{
    IntUnit, Test, TextKind, TimeUnit, ValueType, When, bindings, boolean, choice, colour, days,
    dur, hours, int, mins, offset_hm, offset_hms, secs, text,
};
use iptv_settings::{Change, KeyBinding, PressKind, Scope, Setting, Settings, SettingsError};
use iptv_settings::{Target, Value};

const GL: Scope = Scope::Global;
const PL: Scope = Scope::Playlist;
const GR: Scope = Scope::Group;

/// `(key, type with default and range, scope, flags)` in catalogue order;
/// flags: `a` advanced, `r` requires a restart. Built at run time with the
/// schema's constructors, so their unit conversions are checked as well.
fn documented() -> Vec<(&'static str, ValueType, Scope, &'static str)> {
    use IntUnit::{Count, Percent};
    use TextKind::{CommaList, HostPort, Locale, Path, Plain};
    use TimeUnit::{Millis, Seconds};
    vec![
        // General.
        ("general.start_on_boot", boolean(false), GL, ""),
        ("general.start_on_wake", boolean(false), GL, ""),
        ("general.resume_last_channel", boolean(true), GL, ""),
        ("general.confirm_exit", boolean(true), GL, ""),
        (
            "general.confirm_exit_window",
            dur(Millis, 500, 5_000, 100, 2_000),
            GL,
            "a",
        ),
        ("general.language", text(Locale, 35, "system"), GL, ""),
        ("general.clock", choice(&c::CLOCK, "auto"), GL, ""),
        ("general.keyboard", choice(&c::KEYBOARD, "system"), GL, ""),
        ("general.user_agent", text(Plain, 512, ""), PL, ""),
        ("general.udp_proxy", text(HostPort, 261, ""), GL, "a"),
        ("general.usage_statistics", boolean(false), GL, ""),
        ("general.auto_update", boolean(false), GL, ""),
        ("general.beta_updates", boolean(false), GL, "a"),
        // Playlists.
        ("playlists.include_live", boolean(true), PL, ""),
        ("playlists.include_vod", boolean(true), PL, ""),
        (
            "playlists.max_connections",
            int(1, 20, 1, Count, 1),
            PL,
            "a",
        ),
        (
            "playlists.sort",
            choice(&c::PLAYLIST_SORT, "manual"),
            GL,
            "",
        ),
        ("playlists.update_interval", hours(0, 168, 1, 24), PL, ""),
        ("playlists.update_on_start", boolean(false), PL, ""),
        // Channels.
        (
            "channels.group_sort",
            choice(&c::GROUP_SORT, "playlist_order"),
            PL,
            "",
        ),
        (
            "channels.channel_sort",
            choice(&c::CHANNEL_SORT, "playlist_order"),
            GR,
            "",
        ),
        ("channels.show_new_groups", boolean(true), GL, ""),
        ("channels.show_all_channels", boolean(true), GL, ""),
        ("channels.show_favorites", boolean(true), GL, ""),
        ("channels.show_all_playlists", boolean(true), GL, ""),
        (
            "channels.group_all_playlists_by_playlist",
            boolean(false),
            GL,
            "",
        ),
        (
            "channels.playlist_opens",
            choice(&c::PLAYLIST_OPENS, "all_channels"),
            GL,
            "",
        ),
        (
            "channels.show_hidden_group_channels",
            boolean(false),
            GL,
            "",
        ),
        ("channels.strip_prefixes", text(CommaList, 512, ""), PL, ""),
        ("channels.strip_suffixes", text(CommaList, 512, ""), PL, ""),
        // EPG.
        ("epg.update_interval", hours(0, 168, 1, 24), GL, ""),
        ("epg.update_on_start", boolean(true), GL, ""),
        ("epg.update_on_playlist_change", boolean(true), GL, ""),
        ("epg.keep_past", days(0, 14, 1, 7), GL, ""),
        ("epg.store_descriptions", boolean(true), GL, ""),
        ("epg.full_scan", boolean(false), GL, "a"),
        ("epg.time_offset", offset_hm(-720, 840, 15, 0), PL, ""),
        // Appearance.
        ("appearance.accent", colour(&c::ACCENT, "blue"), GL, ""),
        (
            "appearance.background",
            colour(&c::BACKGROUND, "dark"),
            GL,
            "",
        ),
        (
            "appearance.selection_style",
            choice(&c::SELECTION_STYLE, "accent_fill"),
            GL,
            "",
        ),
        (
            "appearance.ui_transparency",
            int(0, 80, 5, Percent, 20),
            GL,
            "",
        ),
        (
            "appearance.panel_transparency",
            choice(&c::LEVEL, "low"),
            GL,
            "",
        ),
        ("appearance.font_size", choice(&c::SIZE, "medium"), GL, ""),
        ("appearance.two_line_channel_names", boolean(false), GL, ""),
        ("appearance.two_line_program_titles", boolean(false), GL, ""),
        ("appearance.animated_scrolling", boolean(true), GL, ""),
        ("appearance.animated_transitions", boolean(true), GL, ""),
        // Guide.
        (
            "guide.visible_minutes",
            int(60, 360, 30, IntUnit::None, 150),
            GL,
            "",
        ),
        ("guide.show_logos", boolean(true), GL, ""),
        (
            "guide.browse_mode",
            choice(&c::BROWSE_MODE, "preview"),
            GL,
            "",
        ),
        (
            "guide.list_browse_mode",
            choice(&c::BROWSE_MODE, "overlay"),
            GL,
            "",
        ),
        ("guide.show_channel_names", boolean(true), GL, ""),
        ("guide.show_channel_numbers", boolean(true), GL, ""),
        ("guide.show_now_playing", boolean(true), GL, ""),
        ("guide.show_programmes", boolean(true), GL, ""),
        ("guide.show_description", boolean(true), GL, ""),
        ("guide.show_source_labels", boolean(false), GL, ""),
        ("guide.show_catchup_badge", boolean(true), GL, ""),
        ("guide.dim_past", boolean(true), GL, ""),
        ("guide.highlight_playing", boolean(true), GL, ""),
        ("guide.highlight_now", boolean(true), GL, ""),
        (
            "guide.now_highlight_style",
            choice(&c::NOW_HIGHLIGHT, "colour"),
            GL,
            "",
        ),
        ("guide.now_line_full_height", boolean(false), GL, ""),
        ("guide.autoplay", boolean(false), GL, ""),
        (
            "guide.switch_delay",
            dur(Seconds, 0, 5_000, 250, 500),
            GL,
            "",
        ),
        ("guide.fullscreen_timeout", secs(0, 60, 1, 0), GL, ""),
        ("guide.stay_on_list", boolean(false), GL, ""),
        ("guide.stay_on_guide", boolean(false), GL, ""),
        ("guide.stay_on_guide_overlay", boolean(true), GL, ""),
        ("guide.back_returns_to_now", boolean(true), GL, ""),
        // Logos.
        ("logos.source", choice(&c::LOGO_SOURCE, "playlist"), PL, ""),
        ("logos.folder", text(Path, 1024, ""), GL, ""),
        ("logos.inexact_match", boolean(false), GL, ""),
        // Player.
        ("player.info_panel_timeout", secs(1, 30, 1, 4), GL, ""),
        ("player.controls_panel_timeout", secs(1, 60, 1, 7), GL, ""),
        ("player.info_on_switch", boolean(true), GL, ""),
        ("player.switch_info_at_bottom", boolean(true), GL, ""),
        ("player.description_on_switch_only", boolean(false), GL, ""),
        ("player.show_media_info", boolean(true), GL, ""),
        ("player.numeric_resolution", boolean(false), GL, ""),
        ("player.clock", boolean(true), GL, ""),
        ("player.clock_date", boolean(false), GL, ""),
        (
            "player.clock_position",
            choice(&c::CORNER, "top_right"),
            GL,
            "",
        ),
        (
            "player.clock_size",
            choice(&c::CLOCK_SIZE, "medium"),
            GL,
            "",
        ),
        ("player.history_button", boolean(true), GL, ""),
        ("player.guide_button", boolean(true), GL, ""),
        (
            "player.pip_on_leave",
            choice(&c::PIP_ON_LEAVE, "auto"),
            GL,
            "",
        ),
        // Playback.
        (
            "playback.video_decoder",
            choice(&c::DECODER, "hardware"),
            GR,
            "",
        ),
        (
            "playback.audio_decoder",
            choice(&c::DECODER, "hardware"),
            GR,
            "",
        ),
        ("playback.tunneling", boolean(false), GL, "a"),
        ("playback.amlogic_workaround", boolean(false), GL, "ar"),
        ("playback.black_frame_on_switch", boolean(true), GL, ""),
        ("playback.passthrough", boolean(false), GL, ""),
        ("playback.prefer_surround", boolean(false), GL, ""),
        (
            "playback.audio_offset",
            dur(Millis, -2_000, 2_000, 10, 0),
            GL,
            "",
        ),
        ("playback.buffer", choice(&c::SIZE, "small"), GL, ""),
        ("playback.afr", boolean(false), GL, ""),
        ("playback.afr_refresh_rate", boolean(true), GL, ""),
        ("playback.afr_resolution", boolean(false), GL, ""),
        ("playback.afr_50_60_only", boolean(false), GL, ""),
        (
            "playback.external_player",
            choice(&c::EXTERNAL_PLAYER, "never"),
            GR,
            "",
        ),
        // Catch-up.
        ("catchup.scheme", choice(&c::CATCHUP_SCHEME, "auto"), PL, ""),
        ("catchup.archive_days", days(0, 30, 1, 0), PL, ""),
        (
            "catchup.start_offset",
            offset_hms(-3_600, 3_600, 1, 0),
            PL,
            "",
        ),
        (
            "catchup.rewind_live_key",
            choice(&c::REWIND_LIVE_KEY, "rewind"),
            GL,
            "",
        ),
        (
            "catchup.seek_keys",
            choice(&c::SEEK_KEYS, "left_right"),
            GL,
            "",
        ),
        ("catchup.forward_step", secs(5, 600, 5, 30), GL, ""),
        ("catchup.rewind_step", secs(5, 600, 5, 10), GL, ""),
        ("catchup.timebar_step", mins(1, 60, 1, 5), GL, ""),
        // History.
        ("history.keep_days", days(1, 90, 1, 30), GL, ""),
        ("history.add_after", secs(0, 600, 5, 60), GL, ""),
        ("history.recent_count", int(1, 20, 1, Count, 10), GL, ""),
        ("history.recent_add_after", secs(0, 120, 1, 10), GL, ""),
        // Search.
        ("search.prefer_voice", boolean(false), GL, ""),
        ("search.favorites_first", boolean(true), GL, ""),
        ("search.show_history", boolean(true), GL, ""),
        (
            "search.include_past_without_catchup",
            boolean(false),
            GL,
            "",
        ),
        ("search.stay_on_search", boolean(false), GL, ""),
        // Recordings and reminders.
        ("recordings.folder", text(Path, 1024, ""), GL, ""),
        (
            "recordings.format",
            choice(&c::RECORDING_FORMAT, "ts"),
            GL,
            "",
        ),
        ("recordings.lead_padding", mins(0, 60, 1, 1), GL, ""),
        ("recordings.trail_padding", mins(0, 120, 1, 5), GL, ""),
        ("recordings.reminder_lead", mins(0, 60, 1, 5), GL, ""),
        (
            "recordings.reminder_action",
            choice(&c::REMINDER_ACTION, "ask"),
            GL,
            "",
        ),
        (
            "recordings.reminder_popup_timeout",
            secs(5, 120, 5, 30),
            GL,
            "",
        ),
        ("recordings.reminder_wake", boolean(false), GL, ""),
        // Parental control.
        ("parental.live_enabled", boolean(false), GL, ""),
        ("parental.vod_enabled", boolean(false), GL, ""),
        (
            "parental.require_pin_for",
            choice(&c::PIN_SCOPE, "blocked_content"),
            GL,
            "",
        ),
        (
            "parental.pin_input",
            choice(&c::PIN_INPUT, "keypad"),
            GL,
            "",
        ),
        (
            "parental.unlock_window",
            choice(&c::UNLOCK_WINDOW, "every_time"),
            GL,
            "",
        ),
        // Remote.
        (
            "remote.long_press",
            dur(Millis, 200, 2_000, 50, 400),
            GL,
            "",
        ),
        (
            "remote.digit_timeout",
            dur(Millis, 500, 5_000, 100, 1_500),
            GL,
            "",
        ),
        (
            "remote.player_up_down",
            choice(&c::PLAYER_UP_DOWN, "switch_channels"),
            GL,
            "",
        ),
        ("remote.key_overrides", bindings(256), GL, ""),
        // Movies and series.
        ("vod.movies_by_category", boolean(true), GL, ""),
        ("vod.shows_by_category", boolean(true), GL, ""),
        ("vod.card_style", choice(&c::CARD_STYLE, "poster"), GL, ""),
        ("vod.sort", choice(&c::VOD_SORT, "playlist_order"), PL, ""),
        ("vod.autoplay_next_episode", boolean(true), GL, ""),
    ]
}

/// Every setting with dependencies, and what they are.
const CONDITIONS: &[(&str, &str)] = &[
    (
        "general.confirm_exit_window",
        "Visible if general.confirm_exit IsTrue",
    ),
    (
        "channels.group_all_playlists_by_playlist",
        "Visible if channels.show_all_playlists IsTrue",
    ),
    (
        "guide.now_highlight_style",
        "Visible if guide.highlight_now IsTrue",
    ),
    (
        "guide.stay_on_guide_overlay",
        "Visible if guide.browse_mode EnumIn([\"overlay\"])",
    ),
    ("logos.inexact_match", "Visible if logos.folder NotEmpty"),
    (
        "player.switch_info_at_bottom",
        "Visible if player.info_on_switch IsTrue",
    ),
    (
        "player.numeric_resolution",
        "Visible if player.show_media_info IsTrue",
    ),
    ("player.clock_date", "Enabled if player.clock IsTrue"),
    ("player.clock_position", "Visible if player.clock IsTrue"),
    ("player.clock_size", "Visible if player.clock IsTrue"),
    (
        "playback.afr_refresh_rate",
        "Enabled if playback.afr IsTrue",
    ),
    ("playback.afr_resolution", "Enabled if playback.afr IsTrue"),
    (
        "playback.afr_50_60_only",
        "Enabled if playback.afr IsTrue; Enabled if playback.afr_refresh_rate IsTrue",
    ),
    (
        "recordings.reminder_popup_timeout",
        "Visible if recordings.reminder_action EnumIn([\"ask\", \"notify_only\"])",
    ),
    (
        "parental.require_pin_for",
        "Visible if parental.live_enabled IsTrue or parental.vod_enabled IsTrue",
    ),
    (
        "parental.pin_input",
        "Visible if parental.live_enabled IsTrue or parental.vod_enabled IsTrue",
    ),
    (
        "parental.unlock_window",
        "Visible if parental.live_enabled IsTrue or parental.vod_enabled IsTrue",
    ),
];

/// `(key, flags, resets)` in catalogue order; flags: `c` asks for
/// confirmation, `r` requires a restart.
const COMMANDS: &[(&str, &str, &[&str])] = &[
    ("general.backup", "", &[]),
    ("general.restore", "cr", &[]),
    ("general.reset_watch_time", "c", &[]),
    ("general.restart", "c", &[]),
    ("general.send_logs", "", &[]),
    ("general.check_updates", "", &[]),
    ("general.about", "", &[]),
    ("playlists.manage", "", &[]),
    ("playlists.add", "", &[]),
    ("playlists.reorder", "", &[]),
    ("playlists.update_all", "", &[]),
    ("channels.manage_groups", "", &[]),
    ("epg.add_source", "", &[]),
    ("epg.manage_sources", "", &[]),
    ("epg.update_now", "", &[]),
    ("epg.clear", "c", &[]),
    ("logos.clear_cache", "", &[]),
    ("history.clear", "c", &[]),
    ("search.clear_history", "c", &[]),
    ("parental.change_pin", "", &[]),
    ("remote.customize", "", &[]),
    ("remote.restore_defaults", "c", &["remote.key_overrides"]),
];

fn describe(s: &Setting) -> String {
    s.depends
        .iter()
        .map(|d| {
            let any: Vec<String> = d
                .any_of
                .iter()
                .map(|w| format!("{} {:?}", w.key, w.test))
                .collect();
            format!("{:?} if {}", d.effect, any.join(" or "))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// A valid value other than the default.
fn other_value(s: &Setting) -> Value {
    let next = |default: i64, max: i64, step: i64| {
        if default + step <= max {
            default + step
        } else {
            default - step
        }
    };
    let other = |variants: &[&str], default: &str| -> String {
        let v = variants.iter().find(|v| **v != default).unwrap();
        (*v).to_owned()
    };
    match s.ty {
        ValueType::Bool { default } => Value::Bool(!default),
        ValueType::Int {
            max, step, default, ..
        } => Value::Int(next(default, max, step)),
        ValueType::Duration {
            max_ms,
            step_ms,
            default_ms,
            ..
        } => Value::Duration(next(default_ms, max_ms, step_ms)),
        ValueType::Enum { choices, default } => Value::Enum(other(choices.variants, default)),
        ValueType::Colour { palette, default } => Value::Colour(other(palette.variants, default)),
        ValueType::Text { kind, .. } => Value::text(match kind {
            TextKind::Plain => "SyntheticPlayer/1.0",
            TextKind::CommaList => "HD ,EXAMPLE: ",
            TextKind::Path => "/storage/example",
            TextKind::HostPort => "192.0.2.1:4022",
            TextKind::Locale => "pt-BR",
        }),
        ValueType::KeyBindings { .. } => Value::KeyBindings(vec![KeyBinding::new(
            Some("player"),
            "red",
            PressKind::Long,
            "show_guide",
        )]),
    }
}

/// A value of `w.key` for which `w` holds (or fails).
fn condition_value(w: &When, holds: bool) -> Value {
    let s = catalogue::setting(w.key).unwrap();
    match w.test {
        Test::IsTrue => Value::Bool(holds),
        Test::EnumIn(variants) => {
            let all = s.ty.choices().unwrap().variants;
            let v = all
                .iter()
                .find(|v| variants.contains(v) == holds)
                .expect("a variant on each side of the test");
            match s.ty {
                ValueType::Colour { .. } => Value::colour(v),
                _ => Value::choice(v),
            }
        }
        Test::NotEmpty if holds => other_value(s),
        Test::NotEmpty => Value::text(""),
    }
}

/// Sets every condition of `s` (and of the settings it depends on) so it
/// holds.
fn satisfy(st: &mut Settings, s: &Setting) {
    for w in s.depends.iter().flat_map(|d| d.any_of) {
        satisfy(st, catalogue::setting(w.key).unwrap());
        st.set(w.key, condition_value(w, true)).unwrap();
    }
}

#[test]
fn the_catalogue_holds_exactly_the_documented_settings() {
    let keys: Vec<&str> = catalogue::settings().map(|s| s.key).collect();
    let documented: Vec<&str> = documented().iter().map(|r| r.0).collect();
    assert_eq!(keys, documented);
}

#[test]
fn settings_have_their_documented_type_default_range_scope_and_flags() {
    let st = Settings::new();
    for (key, ty, scope, flags) in documented() {
        let s = catalogue::setting(key).unwrap();
        assert_eq!(s.ty, ty, "{key}");
        assert_eq!(s.scope, scope, "{key}");
        assert_eq!(s.flags.advanced, flags.contains('a'), "{key} advanced");
        assert_eq!(
            s.flags.requires_restart,
            flags.contains('r'),
            "{key} restart"
        );
        // A fresh store reads the default at every level.
        let default = s.default_value();
        for at in [Target::Global, Target::group("pl-1", "grp-1")] {
            assert_eq!(st.get_in(key, &at), Ok(default.clone()), "{key} at {at}");
        }
        assert_eq!(st.source(key, &Target::Global), Ok(None), "{key}");
    }
}

#[test]
fn durations_are_stored_in_milliseconds_whatever_their_unit() {
    let st = Settings::new();
    let ms = |key: &str| match st.get(key) {
        Ok(Value::Duration(ms)) => ms,
        other => panic!("{key}: {other:?}"),
    };
    assert_eq!(ms("player.info_panel_timeout"), 4_000);
    assert_eq!(ms("recordings.trail_padding"), 5 * 60_000);
    assert_eq!(ms("playlists.update_interval"), 24 * 3_600_000);
    assert_eq!(ms("epg.keep_past"), 7 * 86_400_000);
    let offset = catalogue::setting("epg.time_offset").unwrap();
    assert_eq!(
        offset.ty,
        ValueType::Duration {
            min_ms: -12 * 3_600_000,
            max_ms: 14 * 3_600_000,
            step_ms: 15 * 60_000,
            unit: TimeUnit::HoursMinutes,
            default_ms: 0,
        }
    );
    let start = catalogue::setting("catchup.start_offset").unwrap();
    assert!(matches!(
        start.ty,
        ValueType::Duration {
            min_ms: -3_600_000,
            max_ms: 3_600_000,
            step_ms: 1_000,
            unit: TimeUnit::HoursMinutesSeconds,
            ..
        }
    ));
}

#[test]
fn every_setting_round_trips_through_the_store_and_the_backup() {
    for s in catalogue::settings() {
        let key = s.key;
        let deepest = match s.scope {
            Scope::Global => Target::Global,
            Scope::Playlist => Target::playlist("pl-1"),
            Scope::Group => Target::group("pl-1", "grp-1"),
        };
        let value = other_value(s);
        assert_ne!(value, s.default_value(), "{key}");

        let mut st = Settings::new();
        let changed = vec![Change {
            key,
            target: deepest.clone(),
        }];
        assert_eq!(st.set_in(key, &deepest, value.clone()), Ok(changed.clone()));
        assert_eq!(st.get_in(key, &deepest), Ok(value.clone()), "{key}");
        assert_eq!(st.stored(key, &deepest), Some(&value), "{key}");
        assert_eq!(st.source(key, &deepest), Ok(Some(deepest.clone())));
        if deepest != Target::Global {
            assert_eq!(st.get(key), Ok(s.default_value()), "{key}: global");
        }

        // A level below the scope is refused but reads through.
        let below = match &deepest {
            Target::Global => Some(Target::playlist("pl-1")),
            Target::Playlist(_) => Some(Target::group("pl-1", "grp-1")),
            Target::Group(..) => None,
        };
        if let Some(below) = below {
            let before = st.clone();
            assert_eq!(
                st.set_in(key, &below, value.clone()),
                Err(SettingsError::ScopeNotAllowed {
                    key,
                    target: below.clone()
                })
            );
            assert_eq!(st, before);
            assert_eq!(st.get_in(key, &below), Ok(value.clone()), "{key}");
        }

        let (restored, report) = Settings::from_json(&st.to_json_string()).unwrap();
        assert!(report.is_clean(), "{key}: {report:?}");
        assert_eq!(restored, st, "{key}");

        assert_eq!(st.reset(key), Ok(changed), "{key}");
        assert_eq!(st, Settings::new(), "{key}");
    }
}

#[test]
fn the_documented_settings_have_conditions() {
    let found: Vec<(&str, String)> = catalogue::settings()
        .filter(|s| !s.depends.is_empty())
        .map(|s| (s.key, describe(s)))
        .collect();
    let documented: Vec<(&str, String)> = CONDITIONS
        .iter()
        .map(|(k, d)| (*k, (*d).to_owned()))
        .collect();
    assert_eq!(found, documented);
}

/// Each dependency hides (or locks) its setting exactly while none of its
/// conditions holds; any single condition is enough.
#[test]
fn conditions_show_and_enable_their_settings() {
    for s in catalogue::settings().filter(|s| !s.depends.is_empty()) {
        let key = s.key;
        let g = &Target::Global;
        let mut all_hold = Settings::new();
        satisfy(&mut all_hold, s);
        assert_eq!(all_hold.is_visible(key, g), Ok(true), "{key}");
        assert_eq!(all_hold.is_enabled(key, g), Ok(true), "{key}");

        for d in s.depends {
            let mut none = all_hold.clone();
            for w in d.any_of {
                none.set(w.key, condition_value(w, false)).unwrap();
            }
            let visible = d.effect == iptv_settings::schema::Effect::Enabled;
            assert_eq!(none.is_visible(key, g), Ok(visible), "{key}: {d:?}");
            assert_eq!(none.is_enabled(key, g), Ok(false), "{key}: {d:?}");
            // A hidden or locked setting still keeps and accepts values.
            none.set(key, other_value(s)).unwrap();
            assert_eq!(none.get(key), Ok(other_value(s)), "{key}");

            for w in d.any_of {
                let mut one = none.clone();
                one.set(w.key, condition_value(w, true)).unwrap();
                assert_eq!(one.is_enabled(key, g), Ok(true), "{key} via {}", w.key);
            }
        }
    }
}

#[test]
fn settings_without_conditions_are_always_shown_and_editable() {
    let mut st = Settings::new();
    // Flip every switch: none of them may hide an unconditional setting.
    for s in catalogue::settings() {
        if let ValueType::Bool { default } = s.ty {
            st.set(s.key, Value::Bool(!default)).unwrap();
        }
    }
    for s in catalogue::settings().filter(|s| s.depends.is_empty()) {
        assert_eq!(st.is_visible(s.key, &Target::Global), Ok(true), "{}", s.key);
        assert_eq!(st.is_enabled(s.key, &Target::Global), Ok(true), "{}", s.key);
    }
}

#[test]
fn unknown_keys_are_not_in_the_catalogue() {
    assert_eq!(catalogue::setting("general.no_such_setting"), None);
    assert_eq!(catalogue::setting(""), None);
    // Commands are not settings, and settings are not commands.
    assert_eq!(catalogue::setting("general.backup"), None);
    assert_eq!(catalogue::command("general.language"), None);
    assert_eq!(catalogue::command("general.no_such_command"), None);
    let st = Settings::new();
    let unknown = SettingsError::UnknownKey("general.backup".into());
    assert_eq!(st.get("general.backup"), Err(unknown.clone()));
    assert_eq!(
        st.is_visible("general.backup", &Target::Global),
        Err(unknown.clone())
    );
    assert_eq!(
        st.is_enabled("general.backup", &Target::Global),
        Err(unknown)
    );
}

#[test]
fn commands_are_the_documented_ones() {
    let found: Vec<(&str, String, &[&str])> = catalogue::commands()
        .map(|c| {
            let flags = [(c.confirm, 'c'), (c.requires_restart, 'r')]
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, f)| *f)
                .collect();
            (c.key, flags, c.resets)
        })
        .collect();
    let documented: Vec<(&str, String, &[&str])> = COMMANDS
        .iter()
        .map(|(k, f, r)| (*k, (*f).to_owned(), *r))
        .collect();
    assert_eq!(found, documented);
    let restore = catalogue::command("remote.restore_defaults").unwrap();
    assert_eq!(restore.label_key(), "settings.remote.restore_defaults");
    assert_eq!(restore.help_key(), "settings.remote.restore_defaults.help");
    assert_eq!(restore.subsection, Some("mapping"));
    assert_eq!(
        catalogue::command("general.backup").unwrap().subsection,
        None
    );
}

#[test]
fn a_reset_command_restores_what_it_names() {
    let mut st = Settings::new();
    let bound = other_value(catalogue::setting("remote.key_overrides").unwrap());
    st.set("remote.key_overrides", bound).unwrap();
    st.set("remote.long_press", Value::ms(600)).unwrap();
    let command = catalogue::command("remote.restore_defaults").unwrap();
    for key in command.resets {
        st.reset(key).unwrap();
    }
    assert_eq!(
        st.get("remote.key_overrides"),
        Ok(Value::KeyBindings(vec![]))
    );
    assert_eq!(st.get("remote.long_press"), Ok(Value::ms(600)), "not named");
}
