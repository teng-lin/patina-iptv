//! Assembles the settings rows and commands offered by the current features and device.

use iptv_core::capabilities::FormFactor;
use iptv_presentation::capabilities::Device;
use iptv_presentation::layout::Class;
use iptv_presentation::settings::screen::Offer;
use iptv_settings::keys;

/// The settings the app acts on.
pub const WIRED: &[&str] = &[
    "remote.digit_timeout",
    "parental.live_enabled",
    "parental.vod_enabled",
    "parental.require_pin_for",
    "parental.unlock_window",
    "playlists.include_vod",
    "vod.autoplay_next_episode",
    "general.language",
    "general.double_back_to_exit",
    "channels.show_favorites",
    "channels.show_recent",
    "channels.show_all_channels",
    "general.clock",
    "general.app_update_frequency",
    "guide.autoplay",
    "guide.switch_delay",
    "guide.fullscreen_timeout",
    "guide.after_choosing",
    "guide.browse_mode",
    "guide.list_browse_mode",
    "appearance.font_size",
    "guide.show_channel_numbers",
    "guide.show_logos",
    "guide.visible_minutes",
    "general.resume_last_channel",
    "general.user_agent",
    "playlists.sort",
    "epg.update",
    "appearance.accent",
    "appearance.background",
    "appearance.ui_transparency",
    "player.info_panel_timeout",
    "player.controls_panel_timeout",
    "player.info_on_switch",
    "player.sound_follows_tile",
    "catchup.seek_keys",
    "remote.player_up_down",
    "catchup.forward_step",
    "catchup.rewind_step",
    "catchup.timebar_step",
    "search.show_history",
];

/// The commands the app runs (`settings_view::run`).
pub const COMMANDS: &[&str] = &[
    "history.clear",
    "search.clear_history",
    "parental.change_pin",
    "general.check_updates",
    "about.privacy_policy",
    "general.backup",
    "general.restore",
    "epg.add_source",
    "epg.manage_sources",
    "epg.update_now",
    "epg.clear",
    "remote.customize",
    "playlists.manage",
    "playlists.show_all",
    "playlists.add",
    "playlists.update_all",
    "playlists.reorder",
];

/// Not shown: the changed keys (`remote.key_overrides`), which the
/// bindings editor's row sums up (`settings_view::command_value`); the
/// value stays stored and in backups.
const SUMMED_UP: &[&str] = &["remote.key_overrides"];

/// Whether `key` is set up with the keyboard or remote settings: the
/// remote section, which also holds the keys that seek in replays.
fn on_keys_page(key: &str) -> bool {
    key.starts_with("remote.") || key == keys::SEEK_KEYS.as_str()
}

/// Not offered: device-level features of TV boxes (start with the device,
/// frame-rate switching, decoders, audio passthrough, multicast relay,
/// voice input, self-update, usage statistics). Also `general.about`,
/// which the About page shows on its own, and the double-Back confirmation
/// before leaving (`general.confirm_exit` and its window), retired in #277.
/// Their stored values are kept but ignored. Android TV uses a separate
/// opt-in preference at the navigation rail.
const NOT_OFFERED: &[&str] = &[
    "general.confirm_exit",
    "general.confirm_exit_window",
    "general.start_on_boot",
    "general.start_on_wake",
    "general.udp_proxy",
    "general.usage_statistics",
    "general.auto_update",
    "general.beta_updates",
    "general.about",
    "playback.video_decoder",
    "playback.audio_decoder",
    "playback.tunneling",
    "playback.amlogic_workaround",
    "playback.passthrough",
    "playback.afr",
    "playback.afr_refresh_rate",
    "playback.afr_resolution",
    "playback.afr_50_60_only",
    "search.prefer_voice",
    // Retired by #282. Their stored values stay (a backup keeps them) and
    // are ignored, except as noted: the guide's interval and start switch
    // became `epg.update` (settings version 6); a guide always updates
    // after its playlist changes, keeps descriptions wherever the host
    // stores it on disk, and keeps finished programs for the longest
    // catch-up archive (at least a week); search always ranks favorites first, offers
    // finished programs without replay as a filter in its results and
    // returns to them after watching; OK in the PIN prompt follows one
    // rule. `vod.sort` is the catalog's own Sort, which keeps it, and a
    // playlist's streams at once are among its actions on the playlists
    // screen (a global value moves to each playlist at startup).
    "epg.update_interval",
    "epg.update_on_start",
    "epg.update_on_playlist_change",
    "epg.keep_past",
    "epg.store_descriptions",
    "search.favorites_first",
    "search.include_past_without_catchup",
    "search.stay_on_search",
    "parental.pin_input",
    "vod.sort",
    "playlists.max_connections",
];

/// Retired (#281): fixed behavior replaced them. They stay in the catalog,
/// so stored values and backups from any device still load, and are never
/// offered. `guide.stay_on_guide` and `guide.stay_on_list` are read once
/// more when the settings load, into `guide.after_choosing` while that is
/// not stored (`iptv_runtime::Runtime::settings`); `guide.back_returns_to_now` is
/// always on.
const RETIRED: &[&str] = &[
    "guide.stay_on_guide",
    "guide.stay_on_list",
    "guide.back_returns_to_now",
];

/// The rows the device decides (#281): `Some(Offer::Hidden)` where it
/// hides them, `Some(Offer::Advanced)` where they go to the Advanced page;
/// `None` leaves them to the other rules.
///
/// - `general.clock`: a phone or a tablet follows the system's 24-hour
///   setting ([`crate::chrome::i18n::clock_style`]).
/// - `appearance.ui_transparency`, `guide.browse_mode`,
///   `guide.list_browse_mode`: in a compact window the layout decides (the
///   guide shows its preview; the channel list has no
///   room for the video beside it), so the guide never draws over video.
/// - `guide.fullscreen_timeout`: a TV's only, on the Advanced page. A
///   phone, a tablet or a desktop never goes full screen by itself.
/// - `catchup.forward_step`, `catchup.rewind_step`: on the Advanced page
///   on every device. The touch play bar uses these jumps too.
/// - `guide.after_choosing`: a phone or a tablet always goes full screen.
/// - `player.info_on_switch`: it applies to zapping with keys, which a
///   phone or a tablet has only with a keyboard or a D-pad connected. A
///   connected iPad keyboard brings the row back immediately.
fn by_device(device: Device, key: &str) -> Option<Offer> {
    let input = device.input();
    // Advanced settings: on the Advanced page (#281).
    if matches!(
        key,
        "player.info_panel_timeout" | "player.controls_panel_timeout"
    ) {
        return Some(if device.form_factor() == FormFactor::Tv {
            Offer::Advanced
        } else {
            Offer::Hidden
        });
    }
    match key {
        "catchup.forward_step" | "catchup.rewind_step" => {
            return Some(Offer::Advanced);
        }
        "guide.fullscreen_timeout" if device.form_factor() == FormFactor::Tv => {
            return Some(Offer::Advanced);
        }
        _ => {}
    }
    let hidden = match key {
        "general.double_back_to_exit" => {
            !crate::chrome::input_kit::double_back_offered(device.host)
        }
        "general.clock" | "guide.after_choosing" => {
            crate::wiring::device::handheld(device.form_factor())
        }
        "appearance.ui_transparency" | "guide.browse_mode" | "guide.list_browse_mode" => {
            device.window == Class::Compact
        }
        "guide.fullscreen_timeout" => device.form_factor() != FormFactor::Tv,
        "player.info_on_switch" => {
            crate::wiring::device::handheld(device.form_factor()) && !input.keyboard && !input.dpad
        }
        _ => return None,
    };
    hidden.then_some(Offer::Hidden)
}

/// What the app does with the setting or command `key` on `device` (as it
/// is now: [`crate::wiring::device::current`], so a row can come and go when the
/// form factor, the window or the connected input changes). The
/// long-press time matters only where keys report their release
/// (Android), and picture in picture on leaving only on an Android phone
/// or tablet whose host reported the window (`pip::offers_on_leave`): never on a TV.
pub fn offer(device: Device, key: &str) -> Offer {
    // Commands belong to each playlist’s Guide data screen.
    if matches!(key, "epg.add_source" | "epg.update_now" | "epg.clear") {
        return Offer::Hidden;
    }
    let host = device.host;
    if matches!(
        key,
        "general.check_updates" | "general.app_update_frequency"
    ) && (crate::host::is_tui_mode()
        || matches!(
            host.platform,
            iptv_core::capabilities::Platform::Ipad | iptv_core::capabilities::Platform::Unknown
        ))
    {
        return Offer::Hidden;
    }
    // Recording needs blocking workers. Backup and restore check their
    // own worker and document-picker support (`backup::offered`).
    if !crate::host::HAS_WORKERS && key.starts_with("recordings.") {
        return Offer::Hidden;
    }
    if matches!(key, "general.backup" | "general.restore")
        && !crate::library_ui::backup::offered(key)
    {
        return Offer::Hidden;
    }
    if !crate::host::IMPORTS
        && matches!(
            key,
            "playlists.manage"
                | "playlists.add"
                | "playlists.update_all"
                | "epg.add_source"
                | "epg.update_now"
        )
    {
        return Offer::Hidden;
    }
    if key == "playlists.show_all" {
        let owner = crate::runtime();
        if !crate::host::IMPORTS
            || crate::wiring::data::combined::active(&owner)
            || !crate::runtime::data::has_multiple_enabled(&owner)
        {
            return Offer::Hidden;
        }
    }
    if key == "playlists.manage"
        && !crate::runtime()
            .playlists()
            .is_ok_and(|rows| !rows.is_empty())
    {
        return Offer::Hidden;
    }
    // Arranging and ordering playlists needs two of them; sorted by name,
    // there is no hand order to arrange.
    if matches!(key, "playlists.reorder" | "playlists.sort")
        && crate::wiring::runtime_owner::runtime()
            .playlists()
            .map_or(0, |playlists| playlists.len())
            < 2
    {
        return Offer::Hidden;
    }
    if key == "playlists.reorder" && !crate::wiring::settings::playlists_by_hand() {
        return Offer::Hidden;
    }
    if NOT_OFFERED.contains(&key) || SUMMED_UP.contains(&key) || RETIRED.contains(&key) {
        return Offer::Hidden;
    }
    if let Some(offer) = by_device(device, key) {
        return offer;
    }
    // No enabled playlist can supply movies or series: their settings go
    // as the rail's Movies and Shows do (`lists::live_only`).
    if key.starts_with("vod.") && crate::guide::lists::live_only() {
        return Offer::Hidden;
    }
    // Sound follows the selected tile: not on a phone, nor where multiview
    // never exists (#279). The stored value is kept, unused.
    if key == keys::SOUND_FOLLOWS_TILE.as_str()
        && !iptv_presentation::capabilities::sound_follows_tile(host)
    {
        return Offer::Hidden;
    }
    // The keyboard or remote settings: only while there are keys (#280).
    if on_keys_page(key) && crate::chrome::input_kit::keys_for(host).is_none() {
        return Offer::Hidden;
    }
    if matches!(key, "guide.autoplay" | "guide.switch_delay") {
        return if !crate::playback::browse_playback::autoplay_supported(host) {
            Offer::Hidden
        } else if crate::wiring::device::handheld(host.form_factor) {
            Offer::Active
        } else {
            Offer::Advanced
        };
    }
    if key == keys::SWIPE_TO_ZAP.as_str() {
        return if crate::playback::touch_zap::swipe_offered() {
            Offer::Active
        } else {
            Offer::Hidden
        };
    }
    if WIRED.contains(&key)
        || COMMANDS.contains(&key)
        || (key == keys::LONG_PRESS.as_str() && crate::chrome::input_kit::long_press())
        || (key == keys::PIP_ON_LEAVE.as_str() && crate::playback::pip::offers_on_leave(host))
    {
        Offer::Active
    } else {
        Offer::Hidden
    }
}

/// [`offer`] on the device as it is now (reactive: a closure that asks
/// runs again when a signal changes).
pub fn offer_now(key: &str) -> Offer {
    offer(crate::wiring::device::current(), key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_settings::catalog;

    #[test]
    fn listed_keys_exist() {
        for key in WIRED.iter().chain(SUMMED_UP) {
            assert!(catalog::setting(key).is_some(), "{key}");
        }
        for key in COMMANDS {
            assert!(catalog::command(key).is_some(), "{key}");
        }
        for key in NOT_OFFERED {
            assert!(
                catalog::setting(key).is_some() || catalog::command(key).is_some(),
                "{key}"
            );
        }
    }

    /// Every combination of the three signals on this build.
    fn devices() -> Vec<Device> {
        use iptv_core::capabilities::{FormFactor, Host, Input};
        use iptv_presentation::layout::Class;
        let mut all = Vec::new();
        for form_factor in [
            FormFactor::Phone,
            FormFactor::Tablet,
            FormFactor::Tv,
            FormFactor::Desktop,
        ] {
            for window in [Class::Compact, Class::Medium, Class::Expanded] {
                for (keyboard, dpad) in [(false, false), (true, false), (false, true), (true, true)]
                {
                    all.push(Device {
                        host: Host {
                            form_factor,
                            input: Input { keyboard, dpad },
                            ..crate::host::BUILT
                        },
                        window,
                    });
                }
            }
        }
        all
    }

    #[test]
    fn what_the_app_offers() {
        // What no signal changes. The rows that follow the connected
        // input are checked below (`keys_and_zapping_follow_the_connected_input`,
        // `the_device_decides_guide_player_and_display_rows`).
        // Picture in picture on leaving: not before the host says it has
        // the window, never off Android (nor on a TV, whose host reports
        // none).
        for device in devices() {
            assert_eq!(offer(device, "player.pip_on_leave"), Offer::Hidden);
        }
        crate::playback::pip::set_supported(true);
        let pip = if crate::host::ANDROID {
            Offer::Active
        } else {
            Offer::Hidden
        };
        for device in devices() {
            let offer = |key| offer(device, key);
            assert_eq!(offer("epg.update"), Offer::Active);
            // Retired by #282.
            for key in [
                "epg.update_interval",
                "epg.update_on_start",
                "epg.update_on_playlist_change",
                "epg.keep_past",
                "epg.store_descriptions",
                "search.favorites_first",
                "search.include_past_without_catchup",
                "search.stay_on_search",
                "parental.pin_input",
                "vod.sort",
                "playlists.max_connections",
            ] {
                assert_eq!(offer(key), Offer::Hidden, "{key}");
            }
            assert_eq!(offer("general.user_agent"), Offer::Active);
            assert_eq!(offer("epg.update_now"), Offer::Hidden);
            assert_eq!(offer("epg.manage_sources"), Offer::Active);
            assert_eq!(offer("epg.add_source"), Offer::Hidden);
            assert_eq!(offer("playback.afr"), Offer::Hidden);
            assert_eq!(offer("parental.live_enabled"), Offer::Active);
            assert_eq!(offer("parental.change_pin"), Offer::Active);
            assert_eq!(offer("history.keep_days"), Offer::Hidden);
            // The bindings editor's row sums the changed keys up.
            assert_eq!(offer("remote.key_overrides"), Offer::Hidden);
            assert_eq!(offer("player.pip_on_leave"), pip);
            // Scheduled recording is retired: its padding and reminder
            // wake keys stay readable but have no row.
            for key in [
                "recordings.lead_padding",
                "recordings.trail_padding",
                "recordings.reminder_wake",
            ] {
                assert_eq!(offer(key), Offer::Hidden, "{key}");
            }
            // Retired by fixed behavior (#281), on every device.
            for key in [
                "guide.stay_on_guide",
                "guide.stay_on_list",
                "guide.back_returns_to_now",
            ] {
                assert_eq!(offer(key), Offer::Hidden, "{key}");
            }
            for key in ["player.info_panel_timeout", "player.controls_panel_timeout"] {
                assert_eq!(
                    offer(key),
                    if device.form_factor() == FormFactor::Tv {
                        Offer::Advanced
                    } else {
                        Offer::Hidden
                    }
                );
            }
            for key in ["catchup.timebar_step", "guide.visible_minutes"] {
                assert_eq!(offer(key), Offer::Active, "{key}");
            }
            // The touch play bar uses these too: Advanced on every host.
            for key in ["catchup.forward_step", "catchup.rewind_step"] {
                assert_eq!(offer(key), Offer::Advanced, "{key}");
            }
            // Sound follows the selected tile: never on a phone (#279);
            // elsewhere where this build has multiview.
            let follows = device.form_factor() != iptv_core::capabilities::FormFactor::Phone
                && iptv_presentation::capabilities::multiview_exists(device.host);
            assert_eq!(
                offer("player.sound_follows_tile"),
                if follows {
                    Offer::Active
                } else {
                    Offer::Hidden
                },
                "{device:?}"
            );
        }
        crate::playback::pip::set_supported(false);
        assert_eq!(offer_now("player.pip_on_leave"), Offer::Hidden);
    }

    /// What the device decides (#281), for every combination of the
    /// signals.
    #[test]
    fn the_device_decides_guide_player_and_display_rows() {
        for device in devices() {
            let shown = |key| offer(device, key) == Offer::Active;
            let form = device.form_factor();
            let held = matches!(form, FormFactor::Phone | FormFactor::Tablet);
            let compact = device.window == Class::Compact;
            let keys = device.input().keyboard || device.input().dpad;
            let what = format!("{device:?}");
            assert_eq!(shown("general.clock"), !held, "{what}");
            for key in [
                "appearance.ui_transparency",
                "guide.browse_mode",
                "guide.list_browse_mode",
            ] {
                assert_eq!(shown(key), !compact, "{key}: {what}");
            }
            assert_eq!(
                offer(device, "guide.fullscreen_timeout"),
                if form == FormFactor::Tv {
                    Offer::Advanced
                } else {
                    Offer::Hidden
                },
                "{what}"
            );
            assert_eq!(shown("guide.after_choosing"), !held, "{what}");
            assert_eq!(shown("player.info_on_switch"), !held || keys, "{what}");
        }
        // An iPad follows the live connected-keyboard report.
        let ipad = Device {
            host: iptv_core::capabilities::Host::of(iptv_core::capabilities::Platform::Ipad),
            window: Class::Expanded,
        };
        assert_eq!(offer(ipad, "player.info_on_switch"), Offer::Hidden);
        let mut attached = ipad;
        attached.host.input.keyboard = true;
        assert_eq!(offer(attached, "player.info_on_switch"), Offer::Active);
    }

    /// #280: the keyboard or remote settings exist while there are keys;
    /// guide autoplay and its delay too on a phone or a tablet, and with
    /// the advanced settings on a desktop or a TV.
    #[test]
    fn keys_and_zapping_follow_the_connected_input() {
        let rows = [
            "remote.digit_timeout",
            "remote.player_up_down",
            "remote.customize",
            "catchup.seek_keys",
        ];
        for device in devices() {
            let offer = |key| offer(device, key);
            let input = device.input();
            let has_keys = match device.form_factor() {
                FormFactor::Tv | FormFactor::Desktop => true,
                FormFactor::Phone | FormFactor::Tablet => input.keyboard || input.dpad,
            };
            let page = if has_keys {
                Offer::Active
            } else {
                Offer::Hidden
            };
            for key in rows {
                assert_eq!(offer(key), page, "{key} on {device:?}");
            }
            // A hold only where keys report their release.
            let hold = if has_keys && crate::chrome::input_kit::long_press() {
                Offer::Active
            } else {
                Offer::Hidden
            };
            assert_eq!(offer("remote.long_press"), hold, "{device:?}");
            let zapping = match device.form_factor() {
                FormFactor::Tv | FormFactor::Desktop => Offer::Advanced,
                _ => page,
            };
            for key in ["guide.autoplay", "guide.switch_delay"] {
                assert_eq!(offer(key), zapping, "{key} on {device:?}");
            }
            assert_eq!(
                crate::chrome::input_kit::keys_for(device.host).is_some(),
                has_keys,
                "{device:?}"
            );
        }
    }
}
