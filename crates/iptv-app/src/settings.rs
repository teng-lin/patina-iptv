//! The app's settings: one `iptv_settings::Settings`, kept in
//! `settings.json` in [`crate::data::data_dir`] and loaded at startup.
//!
//! - [`get`]: the effective settings, from any thread (the import threads
//!   read the guide's refresh and storage settings).
//! - [`read`]: the same inside a reactive closure; it subscribes to
//!   `Ui::app.settings`, which [`set`] bumps after every change.
//! - [`set`]: validates and applies at once; the file is saved in the
//!   background (`file_writer`), and a save that fails is reported and
//!   undone, so the live settings never stay ahead of what a restart
//!   reads. Only the UI thread writes.
//!
//! Values are global, except a playlist's own movie and series import
//! (`playlists.include_vod`), which its actions on the playlists screen
//! set (`Runtime::set_playlist_setting`). Other playlist-scoped settings
//! read their global value; a deleted playlist's own values go with it.
//!
//! [`offer`] says what the app does with each setting and command: the
//! ones it acts on ([`WIRED`], [`COMMANDS`]), what it shows read-only,
//! what comes later, and what it does not offer at all (device-level
//! features of TV boxes). Some settings apply here ([`bind`]: colors,
//! panel timeouts, [`network_config`]); the others are read where they act
//! (guide data, catch-up keys and steps, search, startup channel, Back,
//! channel-change info).

use std::sync::Arc;

use iptv_core::capabilities::FormFactor;
use iptv_presentation::capabilities::Device;
use iptv_presentation::layout::Class;
use iptv_presentation::panels::Timeouts;
use iptv_presentation::settings::screen::Offer;
use iptv_settings::keys;
use iptv_settings::{Settings, Target, Value};

use nsh_core::prelude::*;

use crate::Ui;
use crate::device::Recommended;
use crate::theme::{ACCENTS, BACKGROUNDS, Theme};

/// The settings the app acts on.
pub const WIRED: &[&str] = &[
    "remote.digit_timeout",
    "parental.live_enabled",
    "parental.vod_enabled",
    "parental.require_pin_for",
    "parental.pin_input",
    "parental.unlock_window",
    "playlists.include_vod",
    "playlists.max_connections",
    "vod.sort",
    "vod.autoplay_next_episode",
    "general.language",
    "general.clock",
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
    "epg.update_interval",
    "epg.update_on_start",
    "epg.update_on_playlist_change",
    "epg.keep_past",
    "epg.store_descriptions",
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
    "search.favorites_first",
    "search.include_past_without_catchup",
    "search.stay_on_search",
    "search.show_history",
];

/// The commands the app runs (`settings_view::run`).
pub const COMMANDS: &[&str] = &[
    "history.clear",
    "search.clear_history",
    "parental.change_pin",
    "recordings.quit_host",
    "recordings.background_status",
    "about.privacy_policy",
    "general.backup",
    "general.restore",
    "epg.add_source",
    "epg.manage_sources",
    "epg.update_now",
    "epg.clear",
    "remote.customize",
    "playlists.manage",
];

/// Shown with their value only: the key list has no editor yet.
const READ_ONLY: &[&str] = &["remote.key_overrides"];

/// Not offered: device-level features of TV boxes (start with the device,
/// frame-rate switching, decoders, audio passthrough, multicast relay,
/// voice input, self-update, usage statistics). Also `general.about`,
/// which the About page shows on its own, and the double-Back confirmation
/// before leaving (`general.confirm_exit` and its window): each platform's
/// own Back and exit convention applies instead (#277), and stored values
/// are kept but ignored.
const NOT_OFFERED: &[&str] = &[
    "general.confirm_exit",
    "general.confirm_exit_window",
    "general.start_on_boot",
    "general.start_on_wake",
    "general.udp_proxy",
    "general.usage_statistics",
    "general.auto_update",
    "general.beta_updates",
    "general.check_updates",
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
    // They exist for scheduled recordings, which are retired (#273): login
    // startup for the background recorder and Android's exact alarms.
    // Background status and Quit stay while pending jobs can still run.
    "recordings.enable_startup",
    "recordings.disable_startup",
    "recordings.alarm_access",
];

/// Retired (#281): fixed behavior replaced them. They stay in the catalog,
/// so stored values and backups from any device still load, and are never
/// offered. `guide.stay_on_guide` and `guide.stay_on_list` are read once
/// more when the settings load, into `guide.after_choosing` while that is
/// not stored (`iptv_runtime::settings`); `guide.back_returns_to_now` is
/// always on.
const RETIRED: &[&str] = &[
    "guide.stay_on_guide",
    "guide.stay_on_list",
    "guide.back_returns_to_now",
];

/// A phone or a tablet: the device is held, and touched first.
pub fn handheld(form_factor: FormFactor) -> bool {
    matches!(form_factor, FormFactor::Phone | FormFactor::Tablet)
}

/// The rows the device decides (#281): `Some(Offer::Hidden)` where it
/// hides them; `None` leaves them to the other rules.
///
/// - `general.clock`: a phone or a tablet follows the system's 24-hour
///   setting ([`crate::i18n::clock_style`]).
/// - `appearance.ui_transparency`, `guide.browse_mode`,
///   `guide.list_browse_mode`: in a compact window the layout decides (the
///   guide shows its preview, [`browse_mode`]; the channel list has no
///   room for the video beside it), so the guide never draws over video.
/// - `guide.fullscreen_timeout`: a TV's only. A phone, a tablet or a
///   desktop never goes full screen by itself ([`fullscreen_timeout_ms`]).
/// - `guide.after_choosing`: a phone or a tablet always goes full screen
///   ([`stays_after_choosing`]).
/// - `player.info_on_switch`: it applies to zapping with keys, which a
///   phone or a tablet has only with a keyboard or a D-pad connected.
fn by_device(device: Device, key: &str) -> Option<Offer> {
    let input = device.input();
    let hidden = match key {
        "general.clock" | "guide.after_choosing" => handheld(device.form_factor()),
        "appearance.ui_transparency" | "guide.browse_mode" | "guide.list_browse_mode" => {
            device.window == Class::Compact
        }
        "guide.fullscreen_timeout" => device.form_factor() != FormFactor::Tv,
        "player.info_on_switch" => handheld(device.form_factor()) && !input.keyboard && !input.dpad,
        _ => return None,
    };
    hidden.then_some(Offer::Hidden)
}

thread_local! { static REVISION: std::cell::Cell<Option<Signal<u64>>> = const { std::cell::Cell::new(None) }; }

pub fn observe() {
    REVISION.with(|v| {
        if let Some(s) = v.get() {
            s.get();
        }
    });
}

pub fn font_factor() -> f32 {
    observe();
    match choice(keys::FONT_SIZE) {
        "small" => 0.9,
        "large" => 1.15,
        "extra_large" => 1.3,
        _ => 1.0,
    }
}

/// How much larger than the default the text draws (reactive): the app's
/// own text size times the system's (Android's font scale, iOS Dynamic
/// Type), which the platform applies to every text. The layout classes
/// and the guide header measure their room in it.
pub fn text_factor() -> f32 {
    font_factor() * nsh_core::prelude::font_scale()
}

/// What the app does with the setting or command `key` on `device` (as it
/// is now: [`crate::device::current`], so a row can come and go when the
/// form factor, the window or the connected input changes). The
/// long-press time matters only where keys report their release
/// (Android), and picture in picture on leaving only on Android.
pub fn offer(device: Device, key: &str) -> Offer {
    let host = device.host;
    // Recording and backups run on blocking workers; a host without them
    // offers neither. Imports and guide downloads run where the host can
    // import (`host::IMPORTS`).
    if !crate::host::HAS_WORKERS
        && (key.starts_with("recordings.") || matches!(key, "general.backup" | "general.restore"))
    {
        return Offer::Hidden;
    }
    if !crate::host::IMPORTS
        && matches!(
            key,
            "playlists.manage" | "epg.add_source" | "epg.update_now"
        )
    {
        return Offer::Hidden;
    }
    if key == "recordings.quit_host" && !host.startup_registration() {
        return Offer::Hidden;
    }
    // Leaving the app into a picture-in-picture window: Android only.
    let android = host.automatic_pip();
    if matches!(key, "general.backup" | "general.restore") && crate::host::IPAD {
        return Offer::NotYet;
    }
    if NOT_OFFERED.contains(&key) || RETIRED.contains(&key) {
        return Offer::Hidden;
    }
    if let Some(offer) = by_device(device, key) {
        return offer;
    }
    if WIRED.contains(&key)
        || COMMANDS.contains(&key)
        || (key == keys::LONG_PRESS.as_str() && crate::input::long_press())
        || (key == keys::PIP_ON_LEAVE.as_str() && android)
    {
        Offer::Active
    } else if key == keys::PIP_ON_LEAVE.as_str() {
        Offer::Hidden
    } else if READ_ONLY.contains(&key) {
        Offer::ReadOnly
    } else {
        Offer::Hidden
    }
}

/// [`offer`] on the device as it is now (reactive: a closure that asks
/// runs again when a signal changes).
pub fn offer_now(key: &str) -> Offer {
    offer(crate::device::current(), key)
}

/// The effective settings of the shell's application owner.
pub fn get() -> std::sync::Arc<iptv_settings::Settings> {
    crate::runtime().settings()
}

/// [`get`], subscribing the calling reactive closure to changes.
pub fn read(ui: Ui) -> Arc<Settings> {
    ui.app.settings.get();
    get()
}

pub use crate::runtime::settings::{SaveError, add_user_agent};
/// Loads the saved settings (startup).
pub fn load() {
    crate::runtime().load_settings();
}
pub fn set(_ui: Ui, key: &str, value: Value) -> Result<(), SaveError> {
    crate::runtime().set_setting(key, value)
}

pub fn flag(key: iptv_settings::keys::BoolKey) -> bool {
    crate::runtime::settings::flag(&get(), key)
}

pub fn choice(key: iptv_settings::keys::EnumKey) -> &'static str {
    crate::runtime::settings::choice(&get(), key)
}

/// `remote.long_press`, in ms.
pub fn long_press_ms() -> u64 {
    crate::runtime::settings::long_press_ms(&get())
}

/// `general.user_agent`, when set.
pub fn user_agent() -> Option<String> {
    crate::runtime::settings::user_agent(&get())
}

/// The network settings of provider requests.
pub fn network_config() -> crate::runtime::network::NetworkConfig {
    crate::runtime::settings::network_config(&get())
}

/// The guide's layout outside full screen: `guide.browse_mode`, except in
/// a compact window, where the layout decides and the guide shows its
/// preview (reactive: the window's size class).
pub fn browse_mode() -> crate::ViewMode {
    guide_layout(choice(keys::BROWSE_MODE), crate::device::window_class())
}

fn guide_layout(mode: &str, window: Class) -> crate::ViewMode {
    if mode == "overlay" && window != Class::Compact {
        crate::ViewMode::Overlay
    } else {
        crate::ViewMode::Guide
    }
}

/// Whether choosing a channel in the guide over the video or in the
/// channel list keeps it open (`guide.after_choosing`) on a device of
/// `form_factor`. A phone or a tablet always goes full screen. The guide's
/// preview layout does not ask: there the first choice tunes the preview,
/// and choosing the playing channel goes full screen (`browse_playback`).
pub fn stays_after_choosing(s: &Settings, form_factor: FormFactor) -> bool {
    !handheld(form_factor) && s.choice(keys::GUIDE_AFTER_CHOOSING, &Target::Global) == "stay"
}

/// `guide.fullscreen_timeout` on a device of `form_factor`, in ms: a
/// TV's only; elsewhere 0 (never).
pub fn fullscreen_timeout_ms(s: &Settings, form_factor: FormFactor) -> u64 {
    if form_factor == FormFactor::Tv {
        u64::try_from(s.duration_ms(keys::FULLSCREEN_TIMEOUT, &Target::Global)).unwrap_or(0)
    } else {
        0
    }
}

/// A theme bundle's palette key (`"Blue gray"` → `blue_gray`).
pub fn palette_key(name: &str) -> String {
    name.to_lowercase().replace(' ', "_")
}

/// The colors chosen in `appearance.*`.
pub fn theme_of(s: &Settings) -> Theme {
    let at = &Target::Global;
    let (accent, background) = (s.color(keys::ACCENT, at), s.color(keys::BACKGROUND, at));
    let fallback = Theme::default();
    Theme {
        accent: ACCENTS
            .iter()
            .find(|a| palette_key(a.name) == accent)
            .copied()
            .unwrap_or(fallback.accent),
        background: BACKGROUNDS
            .iter()
            .find(|b| palette_key(b.name) == background)
            .copied()
            .unwrap_or(fallback.background),
    }
}

/// The player panels' auto-hide delays: the panel timeouts under
/// Accessibility, never shorter than the system recommends to this viewer
/// ([`Recommended`], Android's accessibility timeout): the banner and the
/// info card by its time to read, the card's actions, the control strip
/// and the mouse's card by its time to act.
pub fn timeouts_of(s: &Settings, recommended: Recommended) -> Timeouts {
    let ms = |key| u64::try_from(s.duration_ms(key, &Target::Global)).unwrap_or(0);
    let fixed = Timeouts::default();
    Timeouts {
        banner_ms: fixed.banner_ms.max(recommended.text_ms),
        info_ms: ms(keys::INFO_PANEL_TIMEOUT).max(recommended.text_ms),
        controls_ms: ms(keys::CONTROLS_PANEL_TIMEOUT).max(recommended.controls_ms),
        // Not a setting: the desktop's controls hide soon after the mouse rests.
        pointer_ms: fixed.pointer_ms.max(recommended.controls_ms),
    }
}

/// Alpha of browsing surfaces drawn over full-screen video, from
/// `appearance.ui_transparency` (percent of the video showing through).
pub fn overlay_alpha(s: &Settings) -> u8 {
    let percent = s.int(keys::UI_TRANSPARENCY, &Target::Global).clamp(0, 100);
    u8::try_from(255 - percent * 255 / 100).unwrap_or(u8::MAX)
}

/// Applies the settings that live in `Ui` state: colors and panel
/// timeouts, now and after every change.
pub fn bind(ui: Ui) {
    REVISION.with(|r| r.set(Some(ui.app.settings)));
    if crate::host::NOTIFICATIONS_FOLLOW_LANGUAGE {
        effect(move || crate::host::language_changed(crate::i18n::language().tag));
    }
    // The guide's layout follows the setting and the window's class: it
    // changes when either changes it, and never in full screen.
    let layout = computed(move || {
        let s = read(ui);
        guide_layout(
            s.choice(keys::BROWSE_MODE, &Target::Global),
            crate::device::window_class(),
        )
    });
    let last_layout = std::cell::Cell::new(None);
    effect(move || {
        let layout = layout.get();
        if last_layout.replace(Some(layout)) != Some(layout)
            && ui.mode.get_untracked() != crate::ViewMode::Fullscreen
        {
            untracked(|| ui.mode.set(layout));
        }
    });
    effect(move || {
        let s = read(ui);
        let theme = theme_of(&s);
        let timeouts = timeouts_of(&s, crate::device::recommended());
        let span = s.int(iptv_settings::keys::GUIDE_VISIBLE_MINUTES, &Target::Global) as i32;
        untracked(|| {
            ui.app.theme.set(theme);
            if ui.focus.with_untracked(|f| f.visible) != span {
                let view = crate::lists::view(ui);
                ui.focus.update(|f| f.set_visible(span, &view));
            }
            if ui.panels.with(|p| p.timeouts) != timeouts {
                ui.panels.update(|p| p.timeouts = timeouts);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use iptv_settings::catalog;

    use super::*;

    #[test]
    fn listed_keys_exist() {
        for key in WIRED.iter().chain(READ_ONLY) {
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

    #[test]
    fn the_user_agent_goes_where_none_is_named() {
        let mut headers = Vec::new();
        add_user_agent(&mut headers, None);
        assert!(headers.is_empty());
        add_user_agent(&mut headers, Some("SyntheticPlayer/1.0".into()));
        assert_eq!(
            headers,
            [("User-Agent".to_owned(), "SyntheticPlayer/1.0".to_owned())]
        );
        let mut own = vec![("user-agent".to_owned(), "ChannelAgent".to_owned())];
        add_user_agent(&mut own, Some("SyntheticPlayer/1.0".into()));
        assert_eq!(own.len(), 1, "the channel's own agent wins");
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
                            ..crate::capabilities::BUILT
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
        // What no signal changes.
        let pip = if crate::host::ANDROID {
            Offer::Active
        } else {
            Offer::Hidden
        };
        for device in devices() {
            let offer = |key| offer(device, key);
            assert_eq!(offer("epg.keep_past"), Offer::Active);
            assert_eq!(offer("epg.update_now"), Offer::Active);
            assert_eq!(offer("epg.manage_sources"), Offer::Active);
            assert_eq!(offer("epg.add_source"), Offer::Active);
            assert_eq!(offer("playback.afr"), Offer::Hidden);
            assert_eq!(offer("vod.sort"), Offer::Active);
            assert_eq!(offer("parental.live_enabled"), Offer::Active);
            assert_eq!(offer("parental.change_pin"), Offer::Active);
            assert_eq!(offer("history.keep_days"), Offer::Hidden);
            assert_eq!(offer("remote.key_overrides"), Offer::ReadOnly);
            assert_eq!(offer("player.pip_on_leave"), pip);
            // Scheduled recording is retired (#273).
            for key in [
                "recordings.enable_startup",
                "recordings.disable_startup",
                "recordings.alarm_access",
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
            // Kept on every device, under Accessibility and advanced
            // Playback (#281).
            for key in [
                "player.info_panel_timeout",
                "player.controls_panel_timeout",
                "catchup.forward_step",
                "catchup.rewind_step",
                "catchup.timebar_step",
                "guide.visible_minutes",
            ] {
                assert_eq!(offer(key), Offer::Active, "{key}");
            }
        }
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
                shown("guide.fullscreen_timeout"),
                form == FormFactor::Tv,
                "{what}"
            );
            assert_eq!(shown("guide.after_choosing"), !held, "{what}");
            assert_eq!(shown("player.info_on_switch"), !held || keys, "{what}");
        }
    }

    #[test]
    fn the_stay_choice_and_the_full_screen_timeout_follow_the_device() {
        let mut s = Settings::new();
        s.set("guide.after_choosing", Value::choice("stay"))
            .unwrap();
        s.set("guide.fullscreen_timeout", Value::secs(5)).unwrap();
        for (form, stays, timeout) in [
            (FormFactor::Phone, false, 0),
            (FormFactor::Tablet, false, 0),
            (FormFactor::Desktop, true, 0),
            (FormFactor::Tv, true, 5_000),
        ] {
            assert_eq!(stays_after_choosing(&s, form), stays, "{form:?}");
            assert_eq!(fullscreen_timeout_ms(&s, form), timeout, "{form:?}");
        }
        // The default goes full screen everywhere.
        let s = Settings::new();
        assert!(!stays_after_choosing(&s, FormFactor::Tv));
        assert!(!stays_after_choosing(&s, FormFactor::Desktop));
    }

    #[test]
    fn a_compact_window_shows_the_guide_beside_its_preview() {
        use crate::ViewMode::{Guide, Overlay};
        assert_eq!(guide_layout("overlay", Class::Expanded), Overlay);
        assert_eq!(guide_layout("overlay", Class::Medium), Overlay);
        assert_eq!(guide_layout("overlay", Class::Compact), Guide);
        for window in [Class::Compact, Class::Medium, Class::Expanded] {
            assert_eq!(guide_layout("preview", window), Guide);
        }
    }

    #[test]
    fn colors_follow_the_appearance_settings() {
        let mut s = Settings::new();
        assert_eq!(theme_of(&s), Theme::default());
        s.set("appearance.accent", Value::color("red")).unwrap();
        s.set("appearance.background", Value::color("blue_gray"))
            .unwrap();
        let theme = theme_of(&s);
        assert_eq!(theme.accent.name, "Red");
        assert_eq!(theme.background.name, "Blue gray");
        // Every palette key names a bundle.
        for name in catalog::choices::ACCENT.variants {
            s.set("appearance.accent", Value::color(name)).unwrap();
            assert_eq!(palette_key(theme_of(&s).accent.name), *name);
        }
    }

    #[test]
    fn panel_timeouts_follow_the_player_settings() {
        let none = Recommended::default();
        let mut s = Settings::new();
        assert_eq!(timeouts_of(&s, none), Timeouts::default());
        s.set("player.info_panel_timeout", Value::secs(9)).unwrap();
        s.set("player.controls_panel_timeout", Value::secs(20))
            .unwrap();
        assert_eq!(
            timeouts_of(&s, none),
            Timeouts {
                info_ms: 9_000,
                controls_ms: 20_000,
                ..Timeouts::default()
            }
        );
    }

    #[test]
    fn panel_timeouts_wait_as_long_as_the_system_recommends() {
        // Android's "time to take action": 30 s to act, 10 s to read.
        let recommended = Recommended {
            controls_ms: 30_000,
            text_ms: 10_000,
        };
        let mut s = Settings::new();
        assert_eq!(
            timeouts_of(&s, recommended),
            Timeouts {
                banner_ms: 10_000,
                info_ms: 10_000,
                controls_ms: 30_000,
                pointer_ms: 30_000,
            }
        );
        // A longer timeout of the viewer's own still wins.
        s.set("player.controls_panel_timeout", Value::secs(60))
            .unwrap();
        assert_eq!(timeouts_of(&s, recommended).controls_ms, 60_000);
    }

    #[test]
    fn transparency_sets_the_overlay_alpha() {
        let mut s = Settings::new();
        assert_eq!(overlay_alpha(&s), 204, "20 % by default");
        s.set("appearance.ui_transparency", Value::Int(0)).unwrap();
        assert_eq!(overlay_alpha(&s), 255);
        s.set("appearance.ui_transparency", Value::Int(80)).unwrap();
        assert_eq!(overlay_alpha(&s), 51);
    }
}
