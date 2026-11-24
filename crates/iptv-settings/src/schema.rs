//! The schema model: what a setting *is*, independent of any stored value.
//!
//! Every setting is one [`Setting`] row in a static table (see
//! [`crate::catalog`]). Storage, backup, the settings UI and validation
//! all read that table; nothing else declares settings.
//!
//! Label keys are i18n keys derived from stable keys, never display text:
//!
//! | Item | Label key | Help key |
//! | --- | --- | --- |
//! | Section | `settings.section.<section>` | — |
//! | Subsection | `settings.section.<section>.<subsection>` | — |
//! | Setting or command | `settings.<key>` | `settings.<key>.help` |
//! | Choice variant | `settings.choice.<family>.<variant>` | — |

use crate::scope::Scope;
use crate::value::Value;

/// Top-level sections of the settings tree, in UI order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Section {
    General,
    Playlists,
    Channels,
    Epg,
    Appearance,
    Guide,
    Logos,
    Player,
    Playback,
    Catchup,
    History,
    Search,
    Recordings,
    Parental,
    Remote,
    Vod,
    About,
}

impl Section {
    pub const ALL: [Section; 17] = [
        Section::General,
        Section::Playlists,
        Section::Channels,
        Section::Epg,
        Section::Appearance,
        Section::Guide,
        Section::Logos,
        Section::Player,
        Section::Playback,
        Section::Catchup,
        Section::History,
        Section::Search,
        Section::Recordings,
        Section::Parental,
        Section::Remote,
        Section::Vod,
        Section::About,
    ];

    /// Stable section key (also the conventional first segment of its
    /// settings' keys; UI placement may change without renaming keys).
    pub const fn key(self) -> &'static str {
        match self {
            Section::General => "general",
            Section::Playlists => "playlists",
            Section::Channels => "channels",
            Section::Epg => "epg",
            Section::Appearance => "appearance",
            Section::Guide => "guide",
            Section::Logos => "logos",
            Section::Player => "player",
            Section::Playback => "playback",
            Section::Catchup => "catchup",
            Section::History => "history",
            Section::Search => "search",
            Section::Recordings => "recordings",
            Section::Parental => "parental",
            Section::Remote => "remote",
            Section::Vod => "vod",
            Section::About => "about",
        }
    }

    pub fn from_key(key: &str) -> Option<Section> {
        Section::ALL.into_iter().find(|s| s.key() == key)
    }

    /// Subsection keys in UI order. Settings without a subsection come
    /// first in the section.
    pub const fn subsections(self) -> &'static [&'static str] {
        match self {
            Section::General => &["startup", "language", "network", "privacy", "updates"],
            Section::Playlists => &["content", "network", "updates"],
            Section::Channels => &["sorting", "categories", "names"],
            Section::Epg => &["updates", "storage", "matching"],
            Section::Appearance => &["colors", "text", "motion"],
            Section::Guide => &["browsing", "content", "emphasis", "switching"],
            Section::Logos => &[],
            Section::Player => &[
                "panels",
                "info",
                "clock",
                "buttons",
                "pip",
                "multiview",
                "accessibility",
            ],
            Section::Playback => &[
                "switching",
                "decoding",
                "audio",
                "buffering",
                "frame_rate",
                "external",
            ],
            Section::Catchup => &["source", "seeking", "steps"],
            Section::History => &["history", "recent"],
            Section::Search => &[],
            Section::Recordings => &["recording", "reminders"],
            Section::Parental => &["scope", "unlock"],
            Section::Remote => &["timing", "mapping"],
            Section::Vod => &["catalog", "playback"],
            Section::About => &[],
        }
    }

    pub fn label_key(self) -> String {
        format!("settings.section.{}", self.key())
    }

    pub fn subsection_label_key(self, subsection: &str) -> String {
        format!("settings.section.{}.{subsection}", self.key())
    }
}

/// A named family of choice variants. Families are shared between
/// settings (e.g. one `size` family), so a variant's label key depends on
/// the family, not on the setting.
#[derive(Debug, PartialEq, Eq)]
pub struct Choices {
    pub family: &'static str,
    pub variants: &'static [&'static str],
}

impl Choices {
    pub fn contains(&self, variant: &str) -> bool {
        self.variants.contains(&variant)
    }

    /// The schema's own `'static` copy of `variant`, if it is a member.
    pub fn intern(&self, variant: &str) -> Option<&'static str> {
        self.variants.iter().copied().find(|v| *v == variant)
    }

    pub fn label_key(&self, variant: &str) -> String {
        format!("settings.choice.{}.{variant}", self.family)
    }
}

/// Display unit of an integer setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntUnit {
    None,
    Count,
    Percent,
}

/// How a duration is displayed and edited. Storage and JSON always use
/// whole milliseconds (signed: offsets can be negative).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeUnit {
    Millis,
    Seconds,
    Minutes,
    Hours,
    Days,
    /// `h:mm`.
    HoursMinutes,
    /// `h:mm:ss`.
    HoursMinutesSeconds,
}

/// What a text value holds; drives the editor (guided TV forms, pickers)
/// and the format check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKind {
    Plain,
    /// Comma-separated list of fragments.
    CommaList,
    /// A folder or file location chosen with a picker.
    Path,
    /// Empty or `host:port`.
    HostPort,
    /// `system` or a BCP 47-like language tag (`en`, `pt-BR`).
    Locale,
}

/// The value type of a setting, with its default (so a default can never
/// have the wrong type).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ValueType {
    Bool {
        default: bool,
    },
    /// Valid values are `min, min + step, …, max`.
    Int {
        min: i64,
        max: i64,
        step: i64,
        unit: IntUnit,
        default: i64,
    },
    /// Milliseconds; valid values are `min_ms, min_ms + step_ms, …, max_ms`.
    Duration {
        min_ms: i64,
        max_ms: i64,
        step_ms: i64,
        unit: TimeUnit,
        default_ms: i64,
    },
    Enum {
        choices: &'static Choices,
        default: &'static str,
    },
    Text {
        kind: TextKind,
        /// Maximum length in characters.
        max_len: usize,
        default: &'static str,
    },
    /// A color bundle chosen by stable name (the palette's values live in
    /// the app's theme); rendered as swatches.
    Color {
        palette: &'static Choices,
        default: &'static str,
    },
    /// User key → action overrides; the defaults live in `iptv-nav`, so the
    /// default list is empty.
    KeyBindings {
        max_len: usize,
    },
}

impl ValueType {
    pub const fn type_name(&self) -> &'static str {
        match self {
            ValueType::Bool { .. } => "bool",
            ValueType::Int { .. } => "integer",
            ValueType::Duration { .. } => "duration",
            ValueType::Enum { .. } => "enum",
            ValueType::Text { .. } => "text",
            ValueType::Color { .. } => "color",
            ValueType::KeyBindings { .. } => "key_bindings",
        }
    }

    /// Suggestions for the choice editor, including text-valued locales.
    /// This does not restrict which valid language tags may be stored.
    pub const fn editor_choices(&self) -> Option<&'static Choices> {
        match self {
            ValueType::Text {
                kind: TextKind::Locale,
                ..
            } => Some(&crate::catalog::choices::LANGUAGE),
            _ => self.choices(),
        }
    }

    /// The variants of an enum or color type.
    pub const fn choices(&self) -> Option<&'static Choices> {
        match self {
            ValueType::Enum { choices, .. } => Some(choices),
            ValueType::Color { palette, .. } => Some(palette),
            _ => None,
        }
    }
}

/// Placeholder for feature gating (e.g. a paid tier). iptv-rs gates
/// nothing today; the field exists so the UI and backup format need no
/// change when it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Gate {
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Flags {
    /// The change takes effect after the app restarts; the UI says so.
    pub requires_restart: bool,
    /// Shown only when the user opts into advanced settings.
    pub advanced: bool,
    pub gate: Gate,
}

/// What a dependency controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Hidden from the tree unless the condition holds.
    Visible,
    /// Shown but not editable unless the condition holds.
    Enabled,
}

/// A predicate on another setting's resolved value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Test {
    IsTrue,
    /// Enum or color value is one of these variants.
    EnumIn(&'static [&'static str]),
    /// Text is not empty.
    NotEmpty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct When {
    pub key: &'static str,
    pub test: Test,
}

/// Holds when any of its conditions holds. A setting's dependencies of one
/// effect must all hold (conjunctive normal form).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub effect: Effect,
    pub any_of: &'static [When],
}

/// One setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Setting {
    /// Stable dotted key; never reused for a different meaning.
    pub key: &'static str,
    pub section: Section,
    pub subsection: Option<&'static str>,
    pub ty: ValueType,
    pub scope: Scope,
    pub flags: Flags,
    pub depends: &'static [Dependency],
    /// Developer-facing description (our wording; UI text comes from the
    /// help label key).
    pub summary: &'static str,
}

impl Setting {
    /// A global setting with no flags or dependencies. `subsection` is
    /// `""` for none.
    pub const fn new(
        section: Section,
        subsection: &'static str,
        key: &'static str,
        ty: ValueType,
        summary: &'static str,
    ) -> Setting {
        Setting {
            key,
            section,
            subsection: if subsection.is_empty() {
                None
            } else {
                Some(subsection)
            },
            ty,
            scope: Scope::Global,
            flags: Flags {
                requires_restart: false,
                advanced: false,
                gate: Gate::None,
            },
            depends: &[],
            summary,
        }
    }

    pub const fn scope(mut self, scope: Scope) -> Setting {
        self.scope = scope;
        self
    }

    pub const fn restart(mut self) -> Setting {
        self.flags.requires_restart = true;
        self
    }

    pub const fn advanced(mut self) -> Setting {
        self.flags.advanced = true;
        self
    }

    pub const fn depends(mut self, depends: &'static [Dependency]) -> Setting {
        self.depends = depends;
        self
    }

    pub fn label_key(&self) -> String {
        format!("settings.{}", self.key)
    }

    pub fn help_key(&self) -> String {
        format!("settings.{}.help", self.key)
    }

    pub fn default_value(&self) -> Value {
        match self.ty {
            ValueType::Bool { default } => Value::Bool(default),
            ValueType::Int { default, .. } => Value::Int(default),
            ValueType::Duration { default_ms, .. } => Value::Duration(default_ms),
            ValueType::Enum { default, .. } => Value::Enum(default.to_owned()),
            ValueType::Text { default, .. } => Value::Text(default.to_owned()),
            ValueType::Color { default, .. } => Value::Color(default.to_owned()),
            ValueType::KeyBindings { .. } => Value::KeyBindings(Vec::new()),
        }
    }
}

// Const constructors used by the catalog.

pub const fn boolean(default: bool) -> ValueType {
    ValueType::Bool { default }
}

pub const fn int(min: i64, max: i64, step: i64, unit: IntUnit, default: i64) -> ValueType {
    ValueType::Int {
        min,
        max,
        step,
        unit,
        default,
    }
}

/// A duration with bounds in milliseconds.
pub const fn dur(
    unit: TimeUnit,
    min_ms: i64,
    max_ms: i64,
    step_ms: i64,
    default_ms: i64,
) -> ValueType {
    ValueType::Duration {
        min_ms,
        max_ms,
        step_ms,
        unit,
        default_ms,
    }
}

const SEC: i64 = 1_000;
const MIN: i64 = 60 * SEC;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;

/// Whole seconds, edited in seconds.
pub const fn secs(min: i64, max: i64, step: i64, default: i64) -> ValueType {
    dur(
        TimeUnit::Seconds,
        min * SEC,
        max * SEC,
        step * SEC,
        default * SEC,
    )
}

/// Whole minutes, edited in minutes.
pub const fn mins(min: i64, max: i64, step: i64, default: i64) -> ValueType {
    dur(
        TimeUnit::Minutes,
        min * MIN,
        max * MIN,
        step * MIN,
        default * MIN,
    )
}

/// Whole hours, edited in hours.
pub const fn hours(min: i64, max: i64, step: i64, default: i64) -> ValueType {
    dur(
        TimeUnit::Hours,
        min * HOUR,
        max * HOUR,
        step * HOUR,
        default * HOUR,
    )
}

/// Whole days, edited in days.
pub const fn days(min: i64, max: i64, step: i64, default: i64) -> ValueType {
    dur(
        TimeUnit::Days,
        min * DAY,
        max * DAY,
        step * DAY,
        default * DAY,
    )
}

/// A signed offset in minutes, edited as `h:mm`.
pub const fn offset_hm(min: i64, max: i64, step: i64, default: i64) -> ValueType {
    dur(
        TimeUnit::HoursMinutes,
        min * MIN,
        max * MIN,
        step * MIN,
        default * MIN,
    )
}

/// A signed offset in seconds, edited as `h:mm:ss`.
pub const fn offset_hms(min: i64, max: i64, step: i64, default: i64) -> ValueType {
    dur(
        TimeUnit::HoursMinutesSeconds,
        min * SEC,
        max * SEC,
        step * SEC,
        default * SEC,
    )
}

pub const fn choice(choices: &'static Choices, default: &'static str) -> ValueType {
    ValueType::Enum { choices, default }
}

pub const fn text(kind: TextKind, max_len: usize, default: &'static str) -> ValueType {
    ValueType::Text {
        kind,
        max_len,
        default,
    }
}

pub const fn color(palette: &'static Choices, default: &'static str) -> ValueType {
    ValueType::Color { palette, default }
}

pub const fn bindings(max_len: usize) -> ValueType {
    ValueType::KeyBindings { max_len }
}

/// Visible only when any of the conditions holds.
pub const fn visible_if(any_of: &'static [When]) -> Dependency {
    Dependency {
        effect: Effect::Visible,
        any_of,
    }
}

/// Editable only when any of the conditions holds.
pub const fn enabled_if(any_of: &'static [When]) -> Dependency {
    Dependency {
        effect: Effect::Enabled,
        any_of,
    }
}

pub const fn when(key: &'static str, test: Test) -> When {
    When { key, test }
}
