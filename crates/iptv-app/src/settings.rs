//! The app's settings: one `iptv_settings::Settings`, kept in
//! `settings.json` in [`crate::data::data_dir`] and loaded at startup.
//!
//! - [`get`]: the effective settings, from any thread (the import threads
//!   read the guide's refresh and storage settings).
//! - [`crate::settings_wiring::read`]: the same inside a reactive closure; it subscribes to
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

use iptv_settings::keys;
use iptv_settings::{Settings, Value};

use nsh_core::prelude::{Signal, font_scale};

thread_local! { static REVISION: std::cell::Cell<Option<Signal<u64>>> = const { std::cell::Cell::new(None) }; }

/// Connects reactive shell consumers to the app's settings revision.
pub(crate) fn bind_revision(revision: Signal<u64>) {
    REVISION.set(Some(revision));
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
    font_factor() * font_scale()
}

/// The effective settings of the shell's application owner.
pub fn get() -> Arc<Settings> {
    crate::runtime_owner::runtime().settings()
}

pub use crate::runtime::settings::{SaveError, add_user_agent};
/// Loads the saved settings (startup).
pub fn load() {
    crate::runtime_owner::runtime().load_settings();
}
pub fn set(key: &str, value: Value) -> Result<(), SaveError> {
    crate::runtime_owner::runtime().set_setting(key, value)
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

#[cfg(test)]
mod tests {
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
}
