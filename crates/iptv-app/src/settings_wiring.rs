//! Projects the app's effective settings into reactive UI state.

use std::sync::Arc;

use iptv_core::capabilities::FormFactor;
use iptv_presentation::panels::Timeouts;
use iptv_settings::{Settings, Target, keys};
use nsh_core::prelude::*;

use crate::device::Recommended;
use crate::{Ui, settings};

/// [`settings::get`], subscribing the calling reactive closure to changes.
pub fn read(ui: Ui) -> Arc<Settings> {
    ui.app.settings.get();
    settings::get()
}

/// Applies the settings that live in `Ui` state: colors and panel
/// timeouts, now and after every change.
pub fn bind(ui: Ui) {
    settings::bind_revision(ui.app.settings);
    if crate::host::NOTIFICATIONS_FOLLOW_LANGUAGE {
        effect(move || crate::host::language_changed(crate::i18n::language().tag));
    }
    effect(move || {
        let s = read(ui);
        let theme = crate::theme::theme_of(&s);
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

/// The player panels' auto-hide delays: fixed defaults except for TV
/// Advanced settings, never shorter than the system recommends to this viewer
/// ([`Recommended`], Android's accessibility timeout): the banner and the
/// info card by its time to read, the card's actions, the control strip
/// and the mouse's card by its time to act.
fn timeouts_of(s: &Settings, recommended: Recommended, form: FormFactor) -> Timeouts {
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

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_settings::Value;

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
}
