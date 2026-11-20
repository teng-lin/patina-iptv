//! Versioned JSON for backup and restore.
//!
//! ```json
//! {
//!   "format": "iptv-rs.settings",
//!   "version": 3,
//!   "global":    { "player.info_panel_timeout": 5000 },
//!   "playlists": { "<playlist id>": { "channels.group_sort": "name" } },
//!   "groups":    { "<playlist id>": { "<group id>": { "channels.channel_sort": "manual" } } }
//! }
//! ```
//!
//! Values: bool → boolean; integer → number; duration → whole
//! milliseconds; enum, colour and text → string; key bindings → array of
//! `{"context"?, "key", "press": "short"|"long", "action"}`. Only explicit
//! choices are written (never defaults), and every object is written with
//! sorted keys, so equal settings serialise to identical text.
//!
//! Loading rules (forward and backward compatibility):
//!
//! 1. Older documents are migrated step by step through [`MIGRATIONS`]
//!    (`from` → `from + 1`) before anything is read; a missing step fails
//!    the load. Newer documents load best-effort and are flagged.
//! 2. An unknown key — at any level — is kept verbatim, reported in
//!    [`LoadReport::unknown`] and written back by the next save.
//!    Unknown top-level members are kept the same way.
//! 3. A number outside the setting's current range, or off its step grid,
//!    is **clamped** into range and down onto the grid, and reported in
//!    [`LoadReport::clamped`].
//! 4. Any other invalid value (wrong JSON type, unknown enum/colour
//!    variant, text too long or malformed, bad binding list, or an override
//!    at a level the setting no longer allows) is **rejected**: the setting
//!    keeps its inherited value, the raw entry is kept and written back
//!    until the key is set or reset, and it is reported in
//!    [`LoadReport::rejected`].
//!
//! Loading is pure and repeatable: loading a saved document again yields
//! the same settings and no migrations.

use std::collections::BTreeMap;

use serde_json::{Map, Value as Json};

use crate::catalogue;
use crate::error::{LoadError, SettingsError};
use crate::schema::{Setting, ValueType};
use crate::scope::Target;
use crate::store::{Layer, Settings};
use crate::validate::{self, clamp_to_grid};
use crate::value::{KeyBinding, PressKind, Value};

pub const FORMAT: &str = "iptv-rs.settings";
/// The version [`Settings::to_json`] writes.
pub const VERSION: u32 = 3;

const RESERVED: [&str; 5] = ["format", "version", "global", "playlists", "groups"];

/// One upgrade step of the document, applied before parsing.
pub struct Migration {
    /// The version this step upgrades from (to `from + 1`).
    pub from: u32,
    pub summary: &'static str,
    pub apply: fn(&mut Map<String, Json>),
}

/// Every migration, in order. Steps must be contiguous from version 1 to
/// [`VERSION`] (checked by the test suite).
pub static MIGRATIONS: &[Migration] = &[
    Migration {
        from: 1,
        summary: "player.panel_timeout_secs (whole seconds) → player.controls_panel_timeout (ms)",
        apply: v1_panel_timeout_to_ms,
    },
    Migration {
        from: 2,
        summary: "guide.stay_on_guide_overlay folds into guide.stay_on_guide (both layouts)",
        apply: v2_one_stay_on_guide,
    },
];

/// One "keep the guide open" choice now covers both guide layouts. The
/// choice made for the layout in use wins; otherwise an explicit choice
/// for the other layout carries over; otherwise the default (off) applies.
/// The overlay layout's old default (on) was never written, so it is not
/// carried over. Only a boolean counts as a choice: an invalid old value is
/// dropped and an invalid shared value is treated as absent.
fn v2_one_stay_on_guide(doc: &mut Map<String, Json>) {
    let old = "guide.stay_on_guide_overlay";
    let new = "guide.stay_on_guide";
    let mode = "guide.browse_mode";
    let global_overlay = doc
        .get("global")
        .and_then(|g| g.get(mode))
        .and_then(Json::as_str)
        == Some("overlay");
    for_each_layer(doc, &mut |layer| {
        let Some(overlay_choice) = layer.remove(old).as_ref().and_then(Json::as_bool) else {
            return;
        };
        let overlay = match layer.get(mode).and_then(Json::as_str) {
            Some(m) => m == "overlay",
            None => global_overlay,
        };
        let shared_choice = layer.get(new).and_then(Json::as_bool);
        if overlay || shared_choice.is_none() {
            layer.insert(new.to_owned(), Json::Bool(overlay_choice));
        }
    });
}

fn v1_panel_timeout_to_ms(doc: &mut Map<String, Json>) {
    for_each_layer(doc, &mut |layer| {
        let old = "player.panel_timeout_secs";
        let new = "player.controls_panel_timeout";
        let Some(secs) = layer.get(old).and_then(Json::as_i64) else {
            return;
        };
        layer.remove(old);
        if !layer.contains_key(new) {
            layer.insert(new.to_owned(), Json::from(secs.saturating_mul(1_000)));
        }
    });
}

/// Calls `f` on every layer object (global, each playlist, each group).
pub fn for_each_layer(doc: &mut Map<String, Json>, f: &mut dyn FnMut(&mut Map<String, Json>)) {
    if let Some(Json::Object(global)) = doc.get_mut("global") {
        f(global);
    }
    if let Some(Json::Object(playlists)) = doc.get_mut("playlists") {
        playlists
            .values_mut()
            .filter_map(Json::as_object_mut)
            .for_each(&mut *f);
    }
    if let Some(Json::Object(groups)) = doc.get_mut("groups") {
        for playlist in groups.values_mut().filter_map(Json::as_object_mut) {
            playlist
                .values_mut()
                .filter_map(Json::as_object_mut)
                .for_each(&mut *f);
        }
    }
}

/// One entry of a loaded document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub target: Target,
    pub key: String,
}

/// What a load did besides applying values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// The document's version as found.
    pub version: u32,
    /// `from` of every migration applied, in order.
    pub migrations: Vec<u32>,
    /// The document is newer than this build ([`VERSION`]).
    pub newer: bool,
    pub unknown: Vec<Entry>,
    /// Unknown top-level members.
    pub unknown_members: Vec<String>,
    pub clamped: Vec<Entry>,
    pub rejected: Vec<(Entry, String)>,
}

impl LoadReport {
    /// Nothing was migrated, adjusted, rejected or left unread.
    pub fn is_clean(&self) -> bool {
        self.migrations.is_empty()
            && !self.newer
            && self.unknown.is_empty()
            && self.unknown_members.is_empty()
            && self.clamped.is_empty()
            && self.rejected.is_empty()
    }
}

enum Decoded {
    Exact(Value),
    Clamped(Value),
    Invalid(String),
}

impl Settings {
    /// The backup document.
    pub fn to_json(&self) -> Json {
        let mut top: BTreeMap<String, Json> = self.extra.clone();
        top.insert("format".into(), Json::from(FORMAT));
        top.insert("version".into(), Json::from(VERSION));
        let mut global = Json::Object(Map::new());
        let mut playlists: BTreeMap<String, Json> = BTreeMap::new();
        let mut groups: BTreeMap<String, BTreeMap<String, Json>> = BTreeMap::new();
        for (target, layer) in &self.layers {
            let obj = layer_json(layer);
            match target {
                Target::Global => global = obj,
                Target::Playlist(p) => {
                    playlists.insert(p.clone(), obj);
                }
                Target::Group(p, g) => {
                    groups.entry(p.clone()).or_default().insert(g.clone(), obj);
                }
            }
        }
        top.insert("global".into(), global);
        top.insert("playlists".into(), sorted(playlists));
        let groups = groups.into_iter().map(|(p, gs)| (p, sorted(gs))).collect();
        top.insert("groups".into(), sorted(groups));
        sorted(top)
    }

    /// The backup document as pretty-printed text.
    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(&self.to_json()).expect("settings JSON always serialises")
    }

    /// Loads a backup document (see the module docs for the rules).
    pub fn from_json(text: &str) -> Result<(Settings, LoadReport), LoadError> {
        let doc: Json =
            serde_json::from_str(text).map_err(|e| LoadError::NotADocument(e.to_string()))?;
        Settings::from_json_value(doc)
    }

    pub fn from_json_value(doc: Json) -> Result<(Settings, LoadReport), LoadError> {
        let Json::Object(mut doc) = doc else {
            return Err(LoadError::NotADocument("not an object".into()));
        };
        if doc.get("format").and_then(Json::as_str) != Some(FORMAT) {
            return Err(LoadError::WrongFormat);
        }
        let version = doc
            .get("version")
            .and_then(Json::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v >= 1)
            .ok_or(LoadError::BadVersion)?;
        let mut report = LoadReport {
            version,
            newer: version > VERSION,
            ..LoadReport::default()
        };
        let mut at = version;
        while at < VERSION {
            let step = MIGRATIONS
                .iter()
                .find(|m| m.from == at)
                .ok_or(LoadError::NoMigration { from: at })?;
            (step.apply)(&mut doc);
            report.migrations.push(at);
            at += 1;
        }

        let mut settings = Settings::new();
        for (name, value) in entries(&doc) {
            if !RESERVED.contains(&name.as_str()) {
                settings.extra.insert(name.clone(), value.clone());
                report.unknown_members.push(name.clone());
            }
        }
        if let Some(global) = doc.get("global") {
            let layer = object(global, "global")?;
            settings.load_layer(Target::Global, layer, &mut report);
        }
        if let Some(playlists) = doc.get("playlists") {
            for (p, layer) in object(playlists, "playlists")? {
                let layer = object(layer, &format!("playlists.{p}"))?;
                settings.load_layer(Target::playlist(p.as_str()), layer, &mut report);
            }
        }
        if let Some(groups) = doc.get("groups") {
            for (p, gs) in object(groups, "groups")? {
                for (g, layer) in object(gs, &format!("groups.{p}"))? {
                    let layer = object(layer, &format!("groups.{p}.{g}"))?;
                    settings.load_layer(Target::group(p.as_str(), g.as_str()), layer, &mut report);
                }
            }
        }
        Ok((settings, report))
    }

    fn load_layer(&mut self, target: Target, obj: Fields<'_>, report: &mut LoadReport) {
        let mut layer = Layer::default();
        for (key, json) in obj {
            let entry = Entry {
                target: target.clone(),
                key: key.clone(),
            };
            let Some(s) = catalogue::setting(key) else {
                layer.raw.insert(key.clone(), json.clone());
                report.unknown.push(entry);
                continue;
            };
            let decoded = if s.scope.allows(&target) {
                decode(s, json)
            } else {
                Decoded::Invalid(format!("{} cannot be overridden for {target}", s.key))
            };
            match decoded {
                Decoded::Exact(v) => {
                    layer.values.insert(s.key, v);
                }
                Decoded::Clamped(v) => {
                    layer.values.insert(s.key, v);
                    report.clamped.push(entry);
                }
                Decoded::Invalid(why) => {
                    layer.raw.insert(key.clone(), json.clone());
                    report.rejected.push((entry, why));
                }
            }
        }
        if !(layer.values.is_empty() && layer.raw.is_empty()) {
            self.layers.insert(target, layer);
        }
    }
}

/// An object's members in key order, so loads (and their reports) are
/// deterministic whatever `serde_json`'s map ordering.
type Fields<'a> = Vec<(&'a String, &'a Json)>;

fn entries(map: &Map<String, Json>) -> Fields<'_> {
    let mut fields: Fields<'_> = map.iter().collect();
    fields.sort_by(|a, b| a.0.cmp(b.0));
    fields
}

fn object<'a>(json: &'a Json, path: &str) -> Result<Fields<'a>, LoadError> {
    json.as_object()
        .map(entries)
        .ok_or_else(|| LoadError::BadLayer(path.to_owned()))
}

/// A `Map` filled in sorted order (stable even with `preserve_order`).
fn sorted(entries: BTreeMap<String, Json>) -> Json {
    Json::Object(entries.into_iter().collect())
}

fn layer_json(layer: &Layer) -> Json {
    let mut entries: BTreeMap<String, Json> = layer.raw.clone();
    for (key, value) in &layer.values {
        entries.insert((*key).to_owned(), value_json(value));
    }
    sorted(entries)
}

pub(crate) fn value_json(value: &Value) -> Json {
    match value {
        Value::Bool(b) => Json::from(*b),
        Value::Int(n) | Value::Duration(n) => Json::from(*n),
        Value::Enum(s) | Value::Text(s) | Value::Colour(s) => Json::from(s.as_str()),
        Value::KeyBindings(list) => Json::Array(list.iter().map(binding_json).collect()),
    }
}

fn binding_json(b: &KeyBinding) -> Json {
    let mut m: BTreeMap<String, Json> = BTreeMap::new();
    if let Some(c) = &b.context {
        m.insert("context".into(), Json::from(c.as_str()));
    }
    m.insert("key".into(), Json::from(b.key.as_str()));
    m.insert("press".into(), Json::from(b.press.key()));
    m.insert("action".into(), Json::from(b.action.as_str()));
    sorted(m)
}

fn decode(s: &Setting, json: &Json) -> Decoded {
    let typed = match (&s.ty, json) {
        (ValueType::Bool { .. }, Json::Bool(b)) => Some(Value::Bool(*b)),
        (ValueType::Int { .. }, Json::Number(n)) => n.as_i64().map(Value::Int),
        (ValueType::Duration { .. }, Json::Number(n)) => n.as_i64().map(Value::Duration),
        (ValueType::Enum { .. }, Json::String(v)) => Some(Value::Enum(v.clone())),
        (ValueType::Text { .. }, Json::String(v)) => Some(Value::Text(v.clone())),
        (ValueType::Colour { .. }, Json::String(v)) => Some(Value::Colour(v.clone())),
        (ValueType::KeyBindings { .. }, Json::Array(items)) => items
            .iter()
            .map(binding_from_json)
            .collect::<Option<Vec<_>>>()
            .map(Value::KeyBindings),
        _ => None,
    };
    let Some(value) = typed else {
        return Decoded::Invalid(format!("{}: expected {}", s.key, s.ty.type_name()));
    };
    match validate::check(s, &value) {
        Ok(()) => Decoded::Exact(value),
        Err(SettingsError::OutOfRange { .. } | SettingsError::OffStep { .. }) => {
            Decoded::Clamped(clamp(s, value))
        }
        Err(e) => Decoded::Invalid(e.to_string()),
    }
}

/// `value` moved onto its setting's grid. Only numbers are out of range
/// or off their step.
fn clamp(s: &Setting, mut value: Value) -> Value {
    if let (Value::Int(n) | Value::Duration(n), Some((min, max, step))) =
        (&mut value, crate::screen::grid(&s.ty))
    {
        *n = clamp_to_grid(*n, min, max, step).expect("setting grid was validated before clamping");
    }
    value
}

fn binding_from_json(json: &Json) -> Option<KeyBinding> {
    let obj = json.as_object()?;
    let context = match obj.get("context") {
        None | Some(Json::Null) => None,
        Some(c) => Some(c.as_str()?.to_owned()),
    };
    Some(KeyBinding {
        context,
        key: obj.get("key")?.as_str()?.to_owned(),
        press: PressKind::from_key(obj.get("press")?.as_str()?)?,
        action: obj.get("action")?.as_str()?.to_owned(),
    })
}
