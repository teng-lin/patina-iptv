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
//! (`playlists.include_vod`) and streams-at-once ceiling
//! (`playlists.max_connections`), which its actions on the playlists
//! screen set (`Runtime::set_playlist_setting`,
//! `Runtime::set_connection_limit`). Other playlist-scoped settings read
//! their global value; a deleted playlist's own values go with it. The
//! catalog's Sort keeps `vod.sort` itself.
//!
//! [`offer`] says what the app does with each setting and command: the
//! ones it acts on ([`WIRED`], [`COMMANDS`]), those it keeps with the
//! advanced settings on this device, what comes later, and what it does
//! not offer at all (device-level features of TV boxes, and the keys'
//! settings while no keys are connected: [`keys`]). Some settings apply here ([`bind`]: colors,
//! panel timeouts, [`network_config`]); the others are read where they act
//! (guide data, catch-up keys and steps, search, startup channel, Back,
//! channel-change info).

use std::sync::Arc;

use iptv_core::capabilities::FormFactor;
use iptv_presentation::capabilities::{Device, Keys};
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
    "parental.unlock_window",
    "playlists.include_vod",
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

/// Not shown: the changed keys (`remote.key_overrides`), which the
/// bindings editor's row sums up (`settings_view::command_value`); the
/// value stays stored and in backups.
const SUMMED_UP: &[&str] = &["remote.key_overrides"];

/// Guide preview and zapping timing (`browse_playback`): moving the focus
/// with keys tunes after the delay. On a phone or a tablet they apply
/// only while keys are connected ([`keys`]); a desktop and a TV keep them
/// with the advanced settings.
const ZAPPING: &[&str] = &["guide.autoplay", "guide.switch_delay"];

/// What the connected keys are set up as. iPadOS reports changes through
/// GCKeyboard, so keyboard settings follow attachment without a rebuild.
pub fn keys(device: Device) -> Option<Keys> {
    iptv_presentation::capabilities::keys(device.host)
}

/// [`keys`] now (reactive).
pub fn keys_now() -> Option<Keys> {
    keys(crate::device::current())
}

/// Whether `key` is set up with the keyboard or remote settings: the
/// remote section, which also holds the keys that seek in replays.
fn on_keys_page(key: &str) -> bool {
    key.starts_with("remote.") || key == keys::SEEK_KEYS.as_str()
}

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
/// hides them, `Some(Offer::Advanced)` where they go to the Advanced page;
/// `None` leaves them to the other rules.
///
/// - `general.clock`: a phone or a tablet follows the system's 24-hour
///   setting ([`crate::i18n::clock_style`]).
/// - `appearance.ui_transparency`, `guide.browse_mode`,
///   `guide.list_browse_mode`: in a compact window the layout decides (the
///   guide shows its preview, [`browse_mode`]; the channel list has no
///   room for the video beside it), so the guide never draws over video.
/// - `guide.fullscreen_timeout`: a TV's only, on the Advanced page. A
///   phone, a tablet or a desktop never goes full screen by itself
///   ([`fullscreen_timeout_ms`]).
/// - `catchup.forward_step`, `catchup.rewind_step`: on the Advanced page
///   on a desktop or a TV. A phone or a tablet has no Advanced page
///   (#280): they stay with the replay jumps there.
/// - `guide.after_choosing`: a phone or a tablet always goes full screen
///   ([`stays_after_choosing`]).
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
        "catchup.forward_step" | "catchup.rewind_step" if !handheld(device.form_factor()) => {
            return Some(Offer::Advanced);
        }
        "guide.fullscreen_timeout" if device.form_factor() == FormFactor::Tv => {
            return Some(Offer::Advanced);
        }
        _ => {}
    }
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

thread_local! {
    /// The host can open a picture-in-picture window (`pip.rs`): what
    /// [`offer`] reads for `player.pip_on_leave`.
    static PIP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the host can open a picture-in-picture window, as it last
/// reported (`pip.rs`). Settings screens that list rows read `ui.pip` too,
/// so they follow a later report.
pub(crate) fn offer_pip(offered: bool) {
    PIP.set(offered);
}

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
/// (Android), and picture in picture on leaving only on an Android phone
/// or tablet whose host reported the window ([`offer_pip`]): never on a TV.
pub fn offer(device: Device, key: &str) -> Offer {
    // Commands belong to each playlist’s Guide data screen.
    if matches!(key, "epg.add_source" | "epg.update_now" | "epg.clear") {
        return Offer::Hidden;
    }
    let host = device.host;
    // Recording and backups run on blocking workers; a host without them
    // offers neither. A backup action also needs its document picker
    // (`backup::offered`; none on the iPad yet). Imports and guide
    // downloads run where the host can import (`host::IMPORTS`).
    if !crate::host::HAS_WORKERS && key.starts_with("recordings.") {
        return Offer::Hidden;
    }
    if matches!(key, "general.backup" | "general.restore")
        && !(crate::host::HAS_WORKERS && crate::backup::offered(key))
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
    // Leaving the app into a picture-in-picture window: Android phones and
    // tablets only, once the host says it has the window.
    let pip = host.automatic_pip() && PIP.get();
    if NOT_OFFERED.contains(&key) || SUMMED_UP.contains(&key) || RETIRED.contains(&key) {
        return Offer::Hidden;
    }
    if let Some(offer) = by_device(device, key) {
        return offer;
    }
    // No enabled playlist can supply movies or series: their settings go
    // as the rail's Movies and Shows do (`lists::live_only`).
    if key.starts_with("vod.") && crate::lists::live_only() {
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
    if on_keys_page(key) && keys(device).is_none() {
        return Offer::Hidden;
    }
    if ZAPPING.contains(&key) {
        return match device.form_factor() {
            FormFactor::Desktop | FormFactor::Tv => Offer::Advanced,
            FormFactor::Phone | FormFactor::Tablet if keys(device).is_some() => Offer::Active,
            FormFactor::Phone | FormFactor::Tablet => Offer::Hidden,
        };
    }
    // Swiping over full-screen video: where a finger may touch it (a TV's
    // remote has Ch± instead). Outside a running app (the unit tests) no
    // device is known: it counts as offered.
    if key == keys::SWIPE_TO_ZAP.as_str() {
        return if try_os().is_none() || crate::play_bar::pointer_available() {
            Offer::Active
        } else {
            Offer::Hidden
        };
    }
    if WIRED.contains(&key)
        || COMMANDS.contains(&key)
        || (key == keys::LONG_PRESS.as_str() && crate::input::long_press())
        || (key == keys::PIP_ON_LEAVE.as_str() && pip)
    {
        Offer::Active
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

/// The guide opened over full-screen video (the Guide key): its overlay
/// layout, except in a compact window, where the guide shows its preview
/// as [`browse_mode`] does, so it never draws over the video there.
pub fn guide_over_video() -> crate::ViewMode {
    guide_layout("overlay", crate::device::window_class())
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

/// The player panels' auto-hide delays: fixed defaults except for TV
/// Advanced settings, never shorter than the system recommends to this viewer
/// ([`Recommended`], Android's accessibility timeout): the banner and the
/// info card by its time to read, the card's actions, the control strip
/// and the mouse's card by its time to act.
pub fn timeouts_of(s: &Settings, recommended: Recommended, form: FormFactor) -> Timeouts {
    let ms = |key| u64::try_from(s.duration_ms(key, &Target::Global)).unwrap_or(0);
    let fixed = Timeouts::default();
    Timeouts {
        banner_ms: fixed.banner_ms.max(recommended.text_ms),
        info_ms: (if form == FormFactor::Tv {
            ms(keys::INFO_PANEL_TIMEOUT)
        } else {
            fixed.info_ms
        })
        .max(recommended.text_ms),
        controls_ms: (if form == FormFactor::Tv {
            ms(keys::CONTROLS_PANEL_TIMEOUT)
        } else {
            fixed.controls_ms
        })
        .max(recommended.controls_ms),
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
    // A window that turns compact under the guide's overlay (opened over
    // full-screen video) shows it beside its preview too.
    effect(move || {
        if crate::device::window_class() == Class::Compact
            && ui.mode.get_untracked() == crate::ViewMode::Overlay
        {
            untracked(|| ui.mode.set(crate::ViewMode::Guide));
        }
    });
    effect(move || {
        let s = read(ui);
        let theme = theme_of(&s);
        let timeouts = timeouts_of(
            &s,
            crate::device::recommended(),
            crate::device::host().form_factor,
        );
        let span = s.int(iptv_settings::keys::GUIDE_VISIBLE_MINUTES, &Target::Global) as i32;
        untracked(|| {
            ui.app.theme.set(theme);
            if ui.focus.with_untracked(|f| f.visible) != span {
                let view = crate::lists::view(ui);
                ui.focus.update(|f| f.set_visible(span, &view));
            }
            if ui.panels.with(|p| p.timeouts) != timeouts {
                let now = ui.now_ms();
                ui.panels.update(|p| p.set_timeouts(timeouts, now));
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

    use iptv_core::capabilities::FormFactor;

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
        // What no signal changes. The rows that follow the connected
        // input are checked below (`keys_and_zapping_follow_the_connected_input`,
        // `the_device_decides_guide_player_and_display_rows`).
        // Picture in picture on leaving: not before the host says it has
        // the window, never off Android (nor on a TV, whose host reports
        // none).
        for device in devices() {
            assert_eq!(offer(device, "player.pip_on_leave"), Offer::Hidden);
        }
        offer_pip(true);
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
            // On the Advanced page on a desktop or a TV; a phone or a
            // tablet has none (#281, #280).
            let steps = match device.form_factor() {
                FormFactor::Phone | FormFactor::Tablet => Offer::Active,
                FormFactor::Desktop | FormFactor::Tv => Offer::Advanced,
            };
            for key in ["catchup.forward_step", "catchup.rewind_step"] {
                assert_eq!(offer(key), steps, "{key}");
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
        offer_pip(false);
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
            let hold = if has_keys && crate::input::long_press() {
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
            assert_eq!(keys(device).is_some(), has_keys, "{device:?}");
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
        assert_eq!(timeouts_of(&s, none, FormFactor::Tv), Timeouts::default());
        s.set("player.info_panel_timeout", Value::secs(9)).unwrap();
        s.set("player.controls_panel_timeout", Value::secs(20))
            .unwrap();
        assert_eq!(
            timeouts_of(&s, none, FormFactor::Tv),
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
            timeouts_of(&s, recommended, FormFactor::Tv),
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
        assert_eq!(
            timeouts_of(&s, recommended, FormFactor::Tv).controls_ms,
            60_000
        );
    }

    #[test]
    fn hidden_panel_timeout_values_are_ignored_after_loading_or_restoring() {
        let mut s = Settings::new();
        s.set("player.info_panel_timeout", Value::secs(9)).unwrap();
        s.set("player.controls_panel_timeout", Value::secs(60))
            .unwrap();
        for form in [FormFactor::Phone, FormFactor::Tablet, FormFactor::Desktop] {
            assert_eq!(
                timeouts_of(&s, Recommended::default(), form),
                Timeouts::default()
            );
            let recommended = Recommended {
                controls_ms: 30_000,
                text_ms: 10_000,
            };
            let actual = timeouts_of(&s, recommended, form);
            assert_eq!(actual.controls_ms, 30_000);
            assert_eq!(actual.info_ms, 10_000);
        }
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
