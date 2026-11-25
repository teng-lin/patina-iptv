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
use std::sync::Arc;

use iptv_core::capabilities::FormFactor;
use iptv_presentation::layout::Class;
use iptv_presentation::panels::Timeouts;
use iptv_settings::keys;
use iptv_settings::{Settings, Target, Value};

use nsh_core::prelude::*;

use crate::Ui;
use crate::device::Recommended;
use crate::theme::{ACCENTS, BACKGROUNDS, Theme};

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

/// Whether choosing a channel in the guide over the video or in the
/// channel list keeps it open (`guide.after_choosing`) on a device of
/// `form_factor`. A phone or a tablet always goes full screen. The guide's
/// preview layout does not ask: there the first choice tunes the preview,
/// and choosing the playing channel goes full screen (`browse_playback`).
pub fn stays_after_choosing(s: &Settings, form_factor: FormFactor) -> bool {
    !crate::device::handheld(form_factor)
        && s.choice(keys::GUIDE_AFTER_CHOOSING, &Target::Global) == "stay"
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
