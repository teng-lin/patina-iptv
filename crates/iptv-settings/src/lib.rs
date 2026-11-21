#![forbid(unsafe_code)]
//! The settings layer of iptv-rs: one declarative schema that drives
//! storage, backup/restore, the settings UI and validation.
//!
//! - [`schema`]: what a setting is — stable key, section/subsection, value
//!   type with its default, [`Scope`], [`Flags`], dependencies, and
//!   i18n label keys derived from the key.
//! - [`catalog`]: every setting and command of iptv-rs, plus the UI
//!   [`tree`](catalog::tree).
//! - [`Settings`]: the store — validated, scoped (group → playlist →
//!   global → default) reads and writes, resets, change lists.
//! - [`backup`]: versioned JSON with migrations; unknown keys survive.
//! - [`keys`]: typed keys for settings read in code.
//! - [`screen`]: the settings screen's state (sections, rows, editors).
//!
//! Plain Rust: no NSH, no platform APIs, no I/O (callers persist the JSON
//! text), and it builds for `wasm32-unknown-unknown`.
//!
//! ```
//! use iptv_settings::{keys, Settings, Target, Value};
//!
//! let mut settings = Settings::new();
//! assert_eq!(settings.duration_ms(keys::LONG_PRESS, &Target::Global), 400);
//!
//! let changes = settings.set("player.info_panel_timeout", Value::secs(6)).unwrap();
//! assert_eq!(changes[0].key, "player.info_panel_timeout");
//!
//! // A group override beats the playlist's, which beats the global value.
//! let news = Target::group("pl-1", "news");
//! settings.set_in("channels.channel_sort", &Target::playlist("pl-1"), Value::choice("name")).unwrap();
//! settings.set_in("channels.channel_sort", &news, Value::choice("manual")).unwrap();
//! assert_eq!(settings.choice(keys::CHANNEL_SORT, &news), "manual");
//! assert_eq!(settings.choice(keys::CHANNEL_SORT, &Target::group("pl-1", "films")), "name");
//!
//! let text = settings.to_json_string();
//! let (restored, report) = Settings::from_json(&text).unwrap();
//! assert!(report.is_clean());
//! assert_eq!(restored, settings);
//! ```

pub mod backup;
pub mod catalog;
mod error;
pub mod keys;
pub mod schema;
mod scope;
pub mod screen;
mod store;
pub mod validate;
mod value;

pub use backup::{Entry, LoadReport};
pub use error::{LoadError, SettingsError};
pub use schema::{Flags, Section, Setting, ValueType};
pub use scope::{Scope, Target};
pub use store::{Change, Settings};
pub use value::{KeyBinding, PressKind, Value};
