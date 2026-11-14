//! The settings screen's wording (iptv-rs's own): names of sections,
//! subsections, settings, commands and choices, and how values read. The
//! catalogue's summaries serve as the rows' descriptions.

use crate::i18n::{tr, trf};

use iptv_settings::schema::{IntUnit, TimeUnit, ValueType};
use iptv_settings::{Section, Setting, Value};

/// A failed save before publication differs from an update whose durability
/// could not be confirmed. OS details are already recorded by the runtime.
pub fn save_failure(failure: crate::runtime::settings::SaveFailure) -> &'static str {
    match failure {
        crate::runtime::settings::SaveFailure::NotPublished => {
            tr("Settings could not be saved. Check storage permissions and try again.")
        }
        crate::runtime::settings::SaveFailure::DurabilityUnconfirmed => tr(
            "Settings updated, but we could not confirm they were saved safely. Check storage and try again.",
        ),
    }
}

/// The name of a settings page (`settings_view::PAGES`).
pub fn page(key: &str) -> &'static str {
    match key {
        "general" => tr("General"),
        "playlists" => tr("Playlists"),
        "epg" => tr("TV guide data"),
        "appearance" => tr("Appearance"),
        "playback" => tr("Playback"),
        "remote" => tr("Remote control"),
        "parental" => tr("Parental control"),
        "other" => tr("Other"),
        "about" => tr("About"),
        _ => "",
    }
}

/// A heading: a subsection's name, or (on a page of several sections)
/// the section's.
pub fn heading(section: Section, subsection: Option<&str>) -> &'static str {
    match subsection {
        Some(sub) => self::subsection(section, sub),
        None => self::section(section),
    }
}

pub fn section(section: Section) -> &'static str {
    match section {
        Section::General => tr("General"),
        Section::Playlists => tr("Playlists"),
        Section::Channels => tr("Channels"),
        Section::Epg => tr("Guide data"),
        Section::Appearance => tr("Appearance"),
        Section::Guide => tr("Guide"),
        Section::Logos => tr("Logos"),
        Section::Player => tr("Player"),
        Section::Playback => tr("Playback"),
        Section::Catchup => tr("Replay"),
        Section::History => tr("History"),
        Section::Search => tr("Search"),
        Section::Recordings => tr("Recordings"),
        Section::Parental => tr("Parental control"),
        Section::Remote => tr("Remote control"),
        Section::Vod => tr("Movies and series"),
    }
}

pub fn subsection(section: Section, subsection: &str) -> &'static str {
    match (section, subsection) {
        (Section::General, "startup") => tr("Starting and leaving"),
        (Section::General, "language") => tr("Language and keyboard"),
        (Section::Guide, "browsing") => tr("Guide layout"),
        (Section::Guide, "content") => tr("In the guide"),
        (Section::Playback, "decoding") => tr("Picture"),
        (Section::Catchup, "seeking") => tr("Replay keys"),
        (Section::Catchup, "steps") => tr("Replay jumps"),
        (Section::Vod, "catalogue") => tr("Movies and series"),
        (Section::Vod, "playback") => tr("Series playback"),
        (_, "network") => tr("Network"),
        (_, "privacy") => tr("Privacy"),
        (_, "updates") => tr("Updates"),
        (_, "content") => tr("Content"),
        (_, "sorting") => tr("Order"),
        (_, "categories") => tr("Categories"),
        (_, "names") => tr("Channel names"),
        (_, "storage") => tr("Storage"),
        (_, "matching") => tr("Matching channels"),
        (_, "colours") => tr("Colours"),
        (_, "text") => tr("Text"),
        (_, "motion") => tr("Motion"),

        (_, "emphasis") => tr("Emphasis"),
        (_, "switching") => tr("Changing channels"),
        (_, "panels") => tr("Panels"),
        (_, "info") => tr("Stream information"),
        (_, "clock") => tr("Clock"),
        (_, "buttons") => tr("Buttons"),
        (_, "pip") => tr("Picture in picture"),
        (_, "decoding") => "Decoding",
        (_, "audio") => tr("Audio"),
        (_, "buffering") => tr("Buffering"),
        (_, "frame_rate") => tr("Frame rate"),
        (_, "external") => tr("Other players"),
        (_, "source") => tr("Replay source"),

        (_, "history") => tr("Watched"),
        (_, "recent") => tr("Recent channels"),
        (_, "recording") => tr("Recording"),
        (_, "reminders") => tr("Reminders"),
        (_, "scope") => tr("Protection"),
        (_, "unlock") => tr("Unlocking"),
        (_, "timing") => tr("Timing"),
        (_, "mapping") => tr("Keys"),

        _ => "",
    }
}

/// The name of a setting or command.
pub fn label(key: &str) -> &'static str {
    match key {
        "general.start_on_boot" => "Start when the device turns on",
        "general.start_on_wake" => "Start when the device wakes",
        "general.resume_last_channel" => tr("Play the last channel on start"),
        "general.confirm_exit" => tr("Press Back twice to leave"),
        "general.confirm_exit_window" => "Time for the second Back",
        "general.language" => tr("Language"),
        "general.clock" => tr("Clock"),
        "general.keyboard" => tr("On-screen keyboard"),
        "general.user_agent" => "User-Agent",
        "general.udp_proxy" => "UDP relay",
        "general.usage_statistics" => "Share anonymous usage counts",
        "general.auto_update" => "Update by itself",
        "general.beta_updates" => "Offer test versions",
        "general.backup" => tr("Back up"),
        "general.restore" => tr("Restore a backup"),
        "general.reset_watch_time" => "Forget viewing time",
        "general.restart" => tr("Restart the app"),
        "general.send_logs" => "Send a diagnostic log",
        "general.check_updates" => "Look for a new version",
        "general.about" => tr("About this app"),
        "playlists.include_live" => tr("Live channels"),
        "playlists.include_vod" => tr("Movies and series"),
        "playlists.max_connections" => tr("Streams at once"),
        "playlists.sort" => tr("Playlist order"),
        "playlists.update_interval" => "Refresh playlists every",
        "playlists.update_on_start" => "Refresh when the app starts",
        "playlists.manage" => tr("Manage playlists"),
        "playlists.add" => tr("Add a playlist"),
        "playlists.reorder" => tr("Arrange playlists"),
        "playlists.update_all" => "Refresh all playlists now",
        "channels.group_sort" => "Group order",
        "channels.channel_sort" => "Channel order",
        "channels.show_new_groups" => "Show groups that appear later",
        "channels.show_all_channels" => "All channels category",
        "channels.show_favorites" => "Favourites category",
        "channels.show_all_playlists" => "All playlists category",
        "channels.group_all_playlists_by_playlist" => "Keep each playlist together",
        "channels.playlist_opens" => "A playlist opens on",
        "channels.show_hidden_group_channels" => "Channels of hidden groups",
        "channels.strip_prefixes" => "Remove from the start of names",
        "channels.strip_suffixes" => "Remove from the end of names",
        "channels.manage_groups" => "Manage groups",
        "epg.update_interval" => tr("Download the guide every"),
        "epg.update_on_start" => tr("Update when the app starts"),
        "epg.update_on_playlist_change" => tr("Update after a playlist changes"),
        "epg.keep_past" => tr("Keep finished programmes for"),
        "epg.store_descriptions" => tr("Keep programme descriptions"),
        "epg.full_scan" => "Thorough channel matching",
        "epg.time_offset" => "Shift guide times by",
        "epg.add_source" => tr("Add a guide source"),
        "epg.manage_sources" => tr("Guide sources"),
        "epg.update_now" => tr("Update the guide now"),
        "epg.clear" => tr("Delete stored guide data"),
        "appearance.accent" => tr("Accent colour"),
        "appearance.background" => tr("Background colour"),
        "appearance.selection_style" => "Focus fill",
        "appearance.ui_transparency" => tr("Video showing through screens"),
        "appearance.panel_transparency" => "See-through player panels",
        "appearance.font_size" => tr("Text size"),
        "appearance.two_line_channel_names" => "Channel names on two lines",
        "appearance.two_line_program_titles" => "Programme titles on two lines",
        "appearance.animated_scrolling" => "Animated scrolling",
        "appearance.animated_transitions" => "Animated transitions",
        "guide.browse_mode" => tr("Guide layout"),
        "guide.list_browse_mode" => tr("Channel list layout"),
        "guide.visible_minutes" => tr("Visible guide minutes"),
        "guide.show_logos" => tr("Channel logos"),
        "guide.show_channel_names" => tr("Channel names"),
        "guide.show_channel_numbers" => tr("Channel numbers"),
        "guide.show_now_playing" => "Programme on air in the list",
        "guide.show_programmes" => "Programme grid",
        "guide.show_description" => "Programme description",
        "guide.show_source_labels" => "Playlist and group of channels",
        "guide.show_catchup_badge" => "Replay mark",
        "guide.dim_past" => "Dim finished programmes",
        "guide.highlight_playing" => "Colour the playing channel",
        "guide.highlight_now" => "Emphasise programmes on air",
        "guide.now_highlight_style" => "On-air emphasis",
        "guide.now_line_full_height" => "Time line across the grid",
        "guide.autoplay" => tr("Play the focused channel by itself"),
        "guide.switch_delay" => tr("Wait before tuning"),
        "guide.fullscreen_timeout" => tr("Go full screen after"),
        "guide.stay_on_list" => tr("Keep the channel list after choosing"),
        "guide.stay_on_guide" => tr("Keep the guide after choosing"),
        "guide.stay_on_guide_overlay" => tr("Keep the guide over video after choosing"),
        "guide.back_returns_to_now" => tr("Back returns to the programmes on air"),
        "logos.source" => "Preferred logos",
        "logos.folder" => "Logo folder",
        "logos.inexact_match" => "Near matches of file names",
        "logos.clear_cache" => "Forget downloaded logos",
        "player.info_panel_timeout" => tr("Hide the info panel after"),
        "player.controls_panel_timeout" => tr("Hide the controls after"),
        "player.info_on_switch" => tr("Show info on a channel change"),
        "player.switch_info_at_bottom" => "That info along the bottom",
        "player.description_on_switch_only" => "Description only on a channel change",
        "player.show_media_info" => "Stream properties",
        "player.numeric_resolution" => "Resolution in pixels",
        "player.clock" => tr("Clock"),
        "player.clock_date" => "Date with the clock",
        "player.clock_position" => "Clock corner",
        "player.clock_size" => "Clock size",
        "player.history_button" => "History button",
        "player.guide_button" => "Guide button",
        "player.pip_on_leave" => tr("Picture in picture when leaving"),
        "playback.video_decoder" => "Video decoding",
        "playback.audio_decoder" => "Audio decoding",
        "playback.tunneling" => "Tunnelled video",
        "playback.amlogic_workaround" => "Amlogic workaround",
        "playback.black_frame_on_switch" => "Black picture while tuning",
        "playback.passthrough" => "Surround audio to the receiver",
        "playback.prefer_surround" => "Prefer surround audio tracks",
        "playback.audio_offset" => tr("Audio delay"),
        "playback.buffer" => "Buffer",
        "playback.afr" => "Match the content's frame rate",
        "playback.afr_refresh_rate" => "May change the refresh rate",
        "playback.afr_resolution" => "May change the resolution",
        "playback.afr_50_60_only" => "Only for 50 and 60 fps",
        "playback.external_player" => "Play in another app",
        "catchup.scheme" => "Replay addresses",
        "catchup.archive_days" => "Replay window",
        "catchup.start_offset" => "Correct replay start by",
        "catchup.rewind_live_key" => "Rewind live channels with",
        "catchup.seek_keys" => tr("Seek in replays with"),
        "catchup.forward_step" => tr("Fast-forward jumps"),
        "catchup.rewind_step" => tr("Rewind jumps"),
        "catchup.timebar_step" => tr("Time bar jumps"),
        "history.keep_days" => "Keep history for",
        "history.add_after" => "Add to history after",
        "history.recent_count" => "Recent channels kept",
        "history.recent_add_after" => "Add to recent channels after",
        "history.clear" => "Clear history",
        "search.prefer_voice" => "Start with voice input",
        "search.favorites_first" => tr("Favourites first"),
        "search.show_history" => tr("Show recent searches"),
        "search.include_past_without_catchup" => tr("Finished programmes without replay"),
        "search.stay_on_search" => tr("Back to search after watching"),
        "search.clear_history" => "Clear recent searches",
        "recordings.folder" => "Recording folder",
        "recordings.format" => "File type",
        "recordings.lead_padding" => "Start early by",
        "recordings.trail_padding" => "Stop late by",
        "recordings.reminder_lead" => "Remind me before the start",
        "recordings.reminder_action" => "When a reminder is due",
        "recordings.reminder_popup_timeout" => "Reminder closes after",
        "recordings.reminder_wake" => "Wake the device for reminders",
        "parental.live_enabled" => "Protect live channels",
        "parental.vod_enabled" => "Protect movies and series",
        "parental.require_pin_for" => "The PIN guards",
        "parental.pin_input" => "Enter the PIN with",
        "parental.unlock_window" => "Stay unlocked",
        "parental.change_pin" => "Set the PIN",
        "remote.long_press" => tr("Long press after"),
        "remote.digit_timeout" => tr("Channel number pause"),
        "remote.player_up_down" => tr("Up and Down in the player"),
        "remote.key_overrides" => "Changed keys",
        "remote.customize" => tr("Edit remote bindings"),
        "remote.restore_defaults" => "Restore the original keys",
        "vod.movies_by_category" => "Movies by category",
        "vod.shows_by_category" => "Series by category",
        "vod.card_style" => "Tile shape",
        "vod.sort" => "Movie and series order",
        "vod.autoplay_next_episode" => "Play the next episode",
        _ => "",
    }
}

/// What a command row does, where the catalogue's summary says more than
/// this version does.
pub fn description(key: &str) -> Option<&'static str> {
    Some(match key {
        "remote.customize" => {
            "Choose a key, press and action per screen; capture keys or restore defaults."
        }
        "general.about" => {
            "The version of this app. Text is set in Roboto and Roboto Condensed (Google, \
             SIL Open Font License 1.1)."
        }
        "playlists.manage" => {
            "Add, choose, refresh, rename and delete playlists, and arrange their guide sources."
        }
        "epg.add_source" => "Add an XMLTV guide link to the playlist on screen.",
        "epg.manage_sources" => {
            "The playlist's guide sources: switch on or off, order (the first with a channel \
             wins), time shift, how each last downloaded."
        }
        "epg.update_now" => "Download every guide source of the playlist on screen now.",
        _ => return None,
    })
}

/// The name of a choice (enum variant or palette key).
pub fn choice(family: &str, variant: &str) -> String {
    if family == "language" {
        return crate::i18n::language_name(variant).to_owned();
    }
    let name = match (family, variant) {
        ("sort", "manual") => tr("Arranged by hand"),
        ("sort", "name") => tr("By name"),
        ("sort", "playlist_order") => tr("As in the playlist"),
        ("sort", "watch_time") => tr("Most watched first"),
        ("sort", "date_added") => tr("Newest first"),
        ("sort", "rating") => tr("Best rated first"),
        ("sort", "last_modified") => tr("Recently changed first"),
        (_, "small") => tr("Small"),
        (_, "medium") => tr("Medium"),
        (_, "large") => tr("Large"),
        (_, "extra_large") => tr("Extra large"),
        ("level", "none") => tr("None"),
        ("level", "low") => tr("Low"),
        ("level", "high") => tr("High"),
        ("selection_style", "accent_fill") => tr("Accent colour"),
        ("selection_style", "neutral_fill") => tr("Neutral"),
        ("browse_mode", "preview") => tr("Video beside the list"),
        ("browse_mode", "overlay") => tr("List over the video"),
        ("now_highlight", "plain") => tr("Plain"),
        ("now_highlight", "colour") => tr("Tinted"),
        ("now_highlight", "progress_only") => tr("Progress bar only"),
        ("corner", "top_left") => tr("Top left"),
        ("corner", "top_right") => tr("Top right"),
        ("corner", "bottom_left") => tr("Bottom left"),
        ("corner", "bottom_right") => tr("Bottom right"),
        ("logo_source", "playlist") => "From the playlist",
        ("logo_source", "epg") => "From the guide data",
        ("logo_source", "folder") => "From the logo folder",
        ("decoder", "hardware") => tr("Hardware"),
        ("decoder", "software") => tr("Software"),
        ("external_player", "never") => tr("Never"),
        ("external_player", "vod_only") => "Movies and series only",
        ("external_player", "always") => tr("Always"),
        ("pip_on_leave", "auto") => tr("On phones and tablets"),
        ("pip_on_leave", "always") => tr("Always"),
        ("pip_on_leave", "never") => tr("Never"),
        ("catchup_scheme", "auto") => "As the playlist says",
        ("catchup_scheme", "shift") => "Time shift",
        ("catchup_scheme", "append") => "Appended",
        ("catchup_scheme", "flussonic") => "Flussonic",
        ("catchup_scheme", "xtream") => "Xtream Codes",
        ("catchup_scheme", "disabled") => "No replay",
        ("rewind_live_key", "off") => "Nothing",
        ("rewind_live_key", "left") => "Left",
        ("rewind_live_key", "down") => "Down",
        ("rewind_live_key", "rewind") => "The rewind key",
        ("seek_keys", "left_right") => "Left and Right",
        ("seek_keys", "down_up") => "Down and Up",
        ("seek_keys", "transport") => "Rewind and fast-forward keys",
        ("player_up_down", "switch_channels") => tr("Switch channels"),
        ("player_up_down", "channel_list") => tr("Open the channel list"),
        ("recording_format", "ts") => "MPEG-TS",
        ("recording_format", "mkv") => "Matroska",
        ("reminder_action", "ask") => "Ask",
        ("reminder_action", "switch_channel") => "Switch to the channel",
        ("reminder_action", "notify_only") => "Only tell me",
        ("pin_scope", "blocked_content") => "Blocked content",
        ("pin_scope", "blocked_content_and_settings") => "Blocked content and settings",
        ("unlock_window", "every_time") => "Ask every time",
        ("unlock_window", "min_10") => "10 minutes",
        ("unlock_window", "min_30") => "30 minutes",
        ("unlock_window", "hour_1") => "1 hour",
        ("unlock_window", "hour_2") => "2 hours",
        ("unlock_window", "until_restart") => "Until the app restarts",
        ("unlock_window", "until_unblocked_channel") => "Until an open channel plays",
        ("pin_input", "keypad") => "On-screen keypad",
        ("pin_input", "number_keys") => "Number keys",
        ("clock", "auto") => tr("As the language writes it"),
        ("clock", "h12") => tr("12-hour"),
        ("clock", "h24") => tr("24-hour"),
        ("keyboard", "system") => tr("The device's"),
        ("keyboard", "built_in") => tr("The app's own"),
        ("playlist_opens", "all_channels") => tr("All channels"),
        ("playlist_opens", "favorites") => tr("Favourites"),
        ("card_style", "poster") => tr("Poster"),
        ("card_style", "landscape") => tr("Wide"),
        ("card_style", "compact") => tr("Compact"),
        // Palettes: the theme's names (`blue_grey` → "Blue grey").
        ("accent" | "background", v) => return title(v),
        _ => "",
    };
    name.to_owned()
}

fn title(key: &str) -> String {
    let words = key.replace('_', " ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// How a zero reads where it means something else than an amount.
fn zero(key: &str) -> Option<&'static str> {
    Some(match key {
        "epg.update_interval" | "playlists.update_interval" => tr("Only on request"),
        "guide.fullscreen_timeout" => tr("Never"),
        "catchup.archive_days" => "As the provider says",
        "history.add_after" | "history.recent_add_after" => tr("At once"),
        "recordings.reminder_lead" => tr("At the start"),
        "epg.keep_past" => tr("None"),
        _ => return None,
    })
}

/// How an empty text reads.
fn empty(key: &str) -> &'static str {
    match key {
        "general.user_agent" => tr("The app's own"),
        "recordings.folder" => tr("App storage"),
        "channels.strip_prefixes" | "channels.strip_suffixes" | "logos.folder" => tr("None"),
        _ => tr("Not set"),
    }
}

/// A number beside its unit: `message` names where `{value}` goes.
fn with_unit(message: &str, value: impl std::fmt::Display) -> String {
    trf(message, &[("value", (&value.to_string()).into())])
}

/// A signed offset as `h:mm` or `h:mm:ss`.
fn offset(ms: i64, seconds: bool) -> String {
    let sign = if ms < 0 {
        "−"
    } else if ms > 0 {
        "+"
    } else {
        ""
    };
    let s = ms.abs() / 1000;
    if seconds {
        format!("{sign}{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{sign}{}:{:02}", s / 3600, s / 60 % 60)
    }
}

/// A number of a setting's type, in its unit.
pub fn amount(ty: &ValueType, n: i64) -> String {
    match *ty {
        ValueType::Int { unit, .. } => match unit {
            IntUnit::Percent => with_unit("{value} %", n),
            IntUnit::Count | IntUnit::None => n.to_string(),
        },
        ValueType::Duration { unit, min_ms, .. } => match unit {
            TimeUnit::Millis if min_ms < 0 && n > 0 => with_unit("{value} ms", format!("+{n}")),
            TimeUnit::Millis if n < 0 => with_unit("{value} ms", format!("−{}", -n)),
            TimeUnit::Millis => with_unit("{value} ms", n),
            TimeUnit::Seconds => {
                let secs = format!("{:.3}", n as f64 / 1000.0);
                with_unit(
                    "{value} s",
                    secs.trim_end_matches('0').trim_end_matches('.'),
                )
            }
            TimeUnit::Minutes => with_unit("{value} min", n / 60_000),
            TimeUnit::Hours => with_unit("{value} h", n / 3_600_000),
            TimeUnit::Days => trf(
                "{count, plural, one {# day} other {# days}}",
                &[("count", (n / 86_400_000).unsigned_abs().into())],
            ),
            TimeUnit::HoursMinutes => offset(n, false),
            TimeUnit::HoursMinutesSeconds => offset(n, true),
        },
        _ => n.to_string(),
    }
}

/// How `value` of `setting` reads.
pub fn value(setting: &Setting, value: &Value) -> String {
    match value {
        Value::Bool(true) => tr("On").to_owned(),
        Value::Bool(false) => tr("Off").to_owned(),
        Value::Int(0) | Value::Duration(0) if zero(setting.key).is_some() => {
            zero(setting.key).unwrap_or_default().to_owned()
        }
        Value::Int(n) | Value::Duration(n) => amount(&setting.ty, *n),
        Value::Enum(v) | Value::Colour(v) => setting
            .ty
            .choices()
            .map(|c| choice(c.family, v))
            .unwrap_or_else(|| v.clone()),
        Value::Text(t) if t.is_empty() => empty(setting.key).to_owned(),
        Value::Text(t) if setting.key == "general.language" => {
            let name = crate::i18n::language_name(t);
            if name.is_empty() {
                t.clone()
            } else {
                name.to_owned()
            }
        }
        Value::Text(t) => t.clone(),
        Value::KeyBindings(list) if list.is_empty() => tr("None").to_owned(),
        Value::KeyBindings(list) => trf(
            "{count, plural, one {# change} other {# changes}}",
            &[("count", list.len().into())],
        ),
    }
}

#[cfg(test)]
mod tests {
    use iptv_settings::catalogue;

    use super::*;

    #[test]
    fn everything_has_a_name() {
        for sec in Section::ALL {
            assert!(!heading(sec, None).is_empty(), "{sec:?}");
            for sub in sec.subsections() {
                assert!(!heading(sec, Some(sub)).is_empty(), "{sec:?}.{sub}");
            }
        }
        for s in catalogue::settings() {
            assert!(!label(s.key).is_empty(), "{}", s.key);
            if let Some(choices) = s.ty.choices() {
                for v in choices.variants {
                    assert!(!choice(choices.family, v).is_empty(), "{}: {v}", s.key);
                }
            }
        }
        for c in catalogue::commands() {
            assert!(!label(c.key).is_empty(), "{}", c.key);
        }
    }

    #[test]
    fn values_read_in_their_units() {
        let read = |key: &str, v: Value| value(catalogue::setting(key).unwrap(), &v);
        assert_eq!(read("general.resume_last_channel", Value::Bool(true)), "On");
        assert_eq!(
            read("epg.update_interval", Value::Duration(0)),
            "Only on request"
        );
        assert_eq!(read("epg.update_interval", Value::secs(24 * 3600)), "24 h");
        assert_eq!(read("epg.keep_past", Value::secs(86_400)), "1 day");
        assert_eq!(read("epg.keep_past", Value::secs(7 * 86_400)), "7 days");
        assert_eq!(read("catchup.forward_step", Value::secs(30)), "30 s");
        assert_eq!(read("catchup.timebar_step", Value::secs(300)), "5 min");
        assert_eq!(read("guide.switch_delay", Value::ms(500)), "0.5 s");
        assert_eq!(read("guide.switch_delay", Value::ms(1250)), "1.25 s");
        assert_eq!(read("remote.long_press", Value::ms(400)), "400 ms");
        assert_eq!(read("playback.audio_offset", Value::ms(-120)), "−120 ms");
        assert_eq!(read("playback.audio_offset", Value::ms(40)), "+40 ms");
        assert_eq!(read("epg.time_offset", Value::secs(-5400)), "−1:30");
        assert_eq!(read("catchup.start_offset", Value::secs(75)), "+0:01:15");
        assert_eq!(read("appearance.ui_transparency", Value::Int(20)), "20 %");
        assert_eq!(
            read("appearance.accent", Value::colour("blue_grey")),
            "Blue grey"
        );
        assert_eq!(
            read("catchup.seek_keys", Value::choice("down_up")),
            "Down and Up"
        );
        assert_eq!(read("general.user_agent", Value::text("")), "The app's own");
        assert_eq!(
            read("remote.key_overrides", Value::KeyBindings(Vec::new())),
            "None"
        );
    }
}

#[cfg(test)]
mod save_failure_tests {
    use super::save_failure;
    use crate::runtime::settings::SaveFailure;

    #[test]
    fn publication_distinctions_are_translated_in_every_interface_language() {
        let unsaved = save_failure(SaveFailure::NotPublished).to_owned();
        let updated = save_failure(SaveFailure::DurabilityUnconfirmed).to_owned();
        assert_ne!(unsaved, updated);
        for language in iptv_core::i18n::LANGUAGES.iter().skip(1) {
            crate::i18n::in_language(language.tag, || {
                let not_published = save_failure(SaveFailure::NotPublished);
                let durability = save_failure(SaveFailure::DurabilityUnconfirmed);
                assert_ne!(not_published, unsaved, "{}", language.tag);
                assert_ne!(durability, updated, "{}", language.tag);
                assert_ne!(not_published, durability, "{}", language.tag);
            });
        }
    }
}
