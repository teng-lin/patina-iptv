//! The settings screen's state: the chosen page, the focused row (by key,
//! never by position) and the open editor. Toolkit-free and pure like the
//! rest of the crate: the app draws it, feeds it keys and taps, and
//! carries out the [`Effect`]s it answers (store a value, run a command,
//! close).
//!
//! Layout: the app's [`Page`]s (each one or more catalog sections, or
//! named rows), and the [`rows`] of the chosen one — headings, then
//! settings and commands in catalog order. Rows whose visibility
//! conditions fail, advanced settings, and rows the app does not offer
//! ([`Offer::Hidden`]) are left out. Every other setting and command
//! takes the focus, also those the app cannot act on yet ([`Status`]), so
//! the UI can say why OK does nothing there. Rows the app moves to the
//! advanced settings on this device ([`Offer::Advanced`]) leave their
//! section for [`Part::Advanced`]. The pages list leaves out a page
//! without rows ([`Screen::refresh`]), so a page can come and go with
//! what the app offers (a keyboard attached).
//!
//! Keys. Pages: Up/Down choose (the rows follow), Right or OK enter the
//! rows, Back closes. Rows: Up/Down move, Page keys move five, Left or
//! Back return to the pages, OK acts — a switch flips at once, a choice
//! opens its list, a number or duration opens a stepper (Left/Down less,
//! Right/Up more, Page keys ten steps, OK keeps, Back cancels), text opens
//! a text editor (OK keeps a valid text, Back cancels), a command runs
//! (after a confirmation when the catalog asks for one).

pub use iptv_settings::validate::grid;

use iptv_settings::Settings;
use iptv_settings::Target;
use iptv_settings::Value;
use iptv_settings::catalog::{self, Command};
use iptv_settings::schema::{Section, Setting, ValueType};
use iptv_settings::validate;

/// Rows (or list entries) one Page key moves.
const PAGE: usize = 5;
/// Steps one Page key moves in the stepper.
const BIG_STEP: i64 = 10;

/// What the app does with a setting or command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    /// It acts on the value, or runs the command.
    Active,
    /// As [`Offer::Active`], shown with the advanced settings
    /// ([`Part::Advanced`]) instead of in its section.
    Advanced,
    /// It shows the value (or runs nothing); there is nothing to change.
    ReadOnly,
    /// Not done yet.
    NotYet,
    /// Planned for a later version.
    Later,
    /// Not offered: left out of the sections. A page that names it
    /// ([`Part::Keys`]) shows it read-only.
    Hidden,
}

/// What OK on a row can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Active,
    ReadOnly,
    /// Another setting switches it off (an `enabled_if` condition fails).
    Disabled,
    NotYet,
    Later,
}

/// One part of a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// A catalog section's rows.
    Section(Section),
    /// Settings or commands by key, in this order (shown whatever their
    /// [`Offer`]).
    Keys(&'static [&'static str]),
    /// Every setting and command the app offers as advanced
    /// ([`Offer::Advanced`]), under its section's name, in catalog order.
    Advanced,
}

/// One entry of the pages list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    pub key: &'static str,
    pub parts: &'static [Part],
}

/// One line of the rows pane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row {
    /// A title over the rows below it; never focused. Without a
    /// subsection: the section's name (on pages of several sections).
    Heading {
        section: Section,
        subsection: Option<&'static str>,
    },
    Setting(&'static Setting),
    Command(&'static Command),
}

impl Row {
    /// The key of a setting or command row.
    pub fn key(&self) -> Option<&'static str> {
        match self {
            Row::Heading { .. } => None,
            Row::Setting(s) => Some(s.key),
            Row::Command(c) => Some(c.key),
        }
    }
}

/// What the screen reads from the app.
pub struct Ctx<'a> {
    pub settings: &'a Settings,
    pub offer: &'a dyn Fn(&str) -> Offer,
}

impl Ctx<'_> {
    pub fn status(&self, row: &Row) -> Status {
        let Some(key) = row.key() else {
            return Status::ReadOnly;
        };
        match (self.offer)(key) {
            Offer::ReadOnly | Offer::Hidden => Status::ReadOnly,
            Offer::NotYet => Status::NotYet,
            Offer::Later => Status::Later,
            Offer::Active | Offer::Advanced => match row {
                Row::Setting(s) if matches!(s.ty, ValueType::KeyBindings { .. }) => {
                    Status::ReadOnly
                }
                Row::Setting(s)
                    if !self
                        .settings
                        .is_enabled(s.key, &Target::Global)
                        .unwrap_or(false) =>
                {
                    Status::Disabled
                }
                _ => Status::Active,
            },
        }
    }

    /// Shown in its section: offered, and not with the advanced settings.
    fn offered(&self, key: &str) -> bool {
        !matches!((self.offer)(key), Offer::Hidden | Offer::Advanced)
    }

    fn advanced(&self, key: &str) -> bool {
        (self.offer)(key) == Offer::Advanced
    }

    fn value(&self, s: &Setting) -> Value {
        self.settings
            .get(s.key)
            .unwrap_or_else(|_| s.default_value())
    }
}

/// The rows of `page`. On a page of several sections, rows without a
/// subsection get their section's name as heading.
pub fn rows(page: &Page, ctx: &Ctx) -> Vec<Row> {
    let several = page.parts.len() > 1;
    let tree = catalog::tree(false);
    let mut rows = Vec::new();
    for part in page.parts {
        let section = match part {
            Part::Advanced => {
                rows.extend(advanced_rows(ctx));
                continue;
            }
            Part::Keys(keys) => {
                rows.extend(keys.iter().filter_map(|k| {
                    catalog::setting(k)
                        .map(Row::Setting)
                        .or_else(|| catalog::command(k).map(Row::Command))
                }));
                continue;
            }
            Part::Section(section) => *section,
        };
        // The tree has a node for every section.
        let groups = tree
            .iter()
            .filter(|n| n.section == section)
            .flat_map(|n| &n.groups);
        for group in groups {
            let visible = |s: &&&Setting| {
                ctx.offered(s.key)
                    && ctx
                        .settings
                        .is_visible(s.key, &Target::Global)
                        .unwrap_or(false)
            };
            let items: Vec<Row> = group
                .settings
                .iter()
                .filter(visible)
                .map(|s| Row::Setting(s))
                .chain(
                    group
                        .commands
                        .iter()
                        .filter(|c| ctx.offered(c.key))
                        .map(|c| Row::Command(c)),
                )
                .collect();
            if items.is_empty() {
                continue;
            }
            if group.subsection.is_some() || several {
                rows.push(Row::Heading {
                    section,
                    subsection: group.subsection,
                });
            }
            rows.extend(items);
        }
    }
    rows
}

/// The rows of [`Part::Advanced`]: each section's advanced rows under
/// its name.
fn advanced_rows(ctx: &Ctx) -> Vec<Row> {
    let mut rows = Vec::new();
    for node in catalog::tree(true) {
        let visible = |s: &&&Setting| {
            ctx.advanced(s.key)
                && ctx
                    .settings
                    .is_visible(s.key, &Target::Global)
                    .unwrap_or(false)
        };
        let items: Vec<Row> = node
            .groups
            .iter()
            .flat_map(|group| {
                group
                    .settings
                    .iter()
                    .filter(visible)
                    .map(|s| Row::Setting(s))
                    .chain(
                        group
                            .commands
                            .iter()
                            .filter(|c| ctx.advanced(c.key))
                            .map(|c| Row::Command(c)),
                    )
            })
            .collect();
        if !items.is_empty() {
            rows.push(Row::Heading {
                section: node.section,
                subsection: None,
            });
            rows.extend(items);
        }
    }
    rows
}

/// Whether `page` has a row that takes the focus.
fn has_rows(page: &Page, ctx: &Ctx) -> bool {
    rows(page, ctx).iter().any(|r| r.key().is_some())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Pages,
    Rows,
}

/// An open editor or dialog.
#[derive(Clone, Debug, PartialEq)]
pub enum Editor {
    /// An enum, color or locale: its suggestions and focused/current entry.
    Choice {
        setting: &'static Setting,
        focus: usize,
        /// `usize::MAX` when a stored locale is not one of the suggestions.
        selected: usize,
    },
    /// An integer or duration being moved by whole steps (not stored yet).
    Number {
        setting: &'static Setting,
        value: i64,
    },
    /// Text being typed; `invalid` after a rejected OK.
    Text {
        setting: &'static Setting,
        text: String,
        invalid: bool,
    },
    /// "Really?" before a command the catalog wants confirmed.
    Confirm { command: &'static Command },
}

impl Editor {
    /// The key the editor is about.
    pub fn key(&self) -> &'static str {
        match self {
            Editor::Choice { setting, .. }
            | Editor::Number { setting, .. }
            | Editor::Text { setting, .. } => setting.key,
            Editor::Confirm { command } => command.key,
        }
    }

    /// The setting being edited.
    pub fn setting(&self) -> Option<&'static Setting> {
        match self {
            Editor::Choice { setting, .. }
            | Editor::Number { setting, .. }
            | Editor::Text { setting, .. } => Some(setting),
            Editor::Confirm { .. } => None,
        }
    }
}

/// A remote key, as the screen sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
    PageUp,
    PageDown,
}

/// What the app should do after a key or tap.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    None,
    /// Store `value` for `key` (globally).
    Set {
        key: &'static str,
        value: Value,
    },
    /// Run the command `key`.
    Run(&'static str),
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    /// Every page the app has.
    all: &'static [Page],
    /// The pages shown: those of `all` with rows ([`Screen::refresh`]).
    pages: Vec<Page>,
    page: usize,
    pane: Pane,
    focus: Option<&'static str>,
    editor: Option<Editor>,
}

fn moved(at: usize, by: isize, len: usize) -> usize {
    (at as isize + by).clamp(0, len.saturating_sub(1) as isize) as usize
}

impl Screen {
    /// Opens on the page `key` (else the first), focus in the pages list,
    /// with every page shown until [`Screen::refresh`] leaves out those
    /// without rows.
    pub fn new(pages: &'static [Page], key: &str) -> Screen {
        Screen {
            all: pages,
            pages: pages.to_vec(),
            page: pages.iter().position(|p| p.key == key).unwrap_or(0),
            pane: Pane::Pages,
            focus: None,
            editor: None,
        }
    }

    /// [`Screen::new`], showing only the pages with rows now.
    pub fn opened(pages: &'static [Page], key: &str, ctx: &Ctx) -> Screen {
        let mut screen = Screen::new(pages, key);
        screen.refresh(ctx);
        screen
    }

    /// The pages shown.
    pub fn pages(&self) -> &[Page] {
        &self.pages
    }

    /// Shows the pages with rows under what the app offers now (when the
    /// screen opens, and when an offer changes: a keyboard attached or
    /// detached), keeping the focus on something that is still there.
    /// The page shown stays chosen; if it went, the page that took its
    /// place in the list is chosen, with the focus in the pages list. A
    /// focused row that went passes the focus to the first row the app
    /// acts on, and an editor of a row that went closes.
    pub fn refresh(&mut self, ctx: &Ctx) {
        let shown: Vec<Page> = self
            .all
            .iter()
            .filter(|page| has_rows(page, ctx))
            .copied()
            .collect();
        if !shown.is_empty() && shown != self.pages {
            let current = self.page().key;
            if let Some(at) = shown.iter().position(|p| p.key == current) {
                self.page = at;
            } else {
                // The pages before the one that went keep their place.
                let before = self.all.iter().position(|p| p.key == current);
                self.page = before
                    .map(|end| self.all[..end].iter().filter(|p| shown.contains(p)).count())
                    .unwrap_or(0)
                    .min(shown.len() - 1);
                self.pane = Pane::Pages;
                self.focus = None;
                self.editor = None;
            }
            self.pages = shown;
        }
        self.keep_focus(ctx);
    }

    /// A focused row or an open editor whose row went (see
    /// [`Screen::refresh`]).
    fn keep_focus(&mut self, ctx: &Ctx) {
        let rows = self.rows(ctx);
        let there = |key: &str| rows.iter().any(|r| r.key() == Some(key));
        if self.editor.as_ref().is_some_and(|e| !there(e.key())) {
            self.editor = None;
        }
        if self.pane == Pane::Rows && self.focus.is_some_and(|k| !there(k)) {
            self.focus = None;
            self.enter_rows(ctx);
            if self.focus.is_none() {
                self.pane = Pane::Pages;
            }
        }
    }

    pub fn page(&self) -> &Page {
        &self.pages[self.page]
    }

    pub fn page_index(&self) -> usize {
        self.page
    }

    pub fn pane(&self) -> Pane {
        self.pane
    }

    /// The focused row's key (meaningful in [`Pane::Rows`]).
    pub fn focus(&self) -> Option<&'static str> {
        self.focus
    }

    pub fn editor(&self) -> Option<&Editor> {
        self.editor.as_ref()
    }

    pub fn rows(&self, ctx: &Ctx) -> Vec<Row> {
        rows(self.page(), ctx)
    }

    /// The row with the focus, if the rows pane has it.
    pub fn focused_row(&self, ctx: &Ctx) -> Option<Row> {
        let key = self.focus.filter(|_| self.pane == Pane::Rows)?;
        self.rows(ctx).into_iter().find(|r| r.key() == Some(key))
    }

    pub fn key(&mut self, key: Key, ctx: &Ctx) -> Effect {
        if self.editor.is_some() {
            return self.editor_key(key);
        }
        match self.pane {
            Pane::Pages => self.pages_key(key, ctx),
            Pane::Rows => self.rows_key(key, ctx),
        }
    }

    fn pages_key(&mut self, key: Key, ctx: &Ctx) -> Effect {
        let to = match key {
            Key::Up => moved(self.page, -1, self.pages.len()),
            Key::Down => moved(self.page, 1, self.pages.len()),
            Key::PageUp => moved(self.page, -(PAGE as isize), self.pages.len()),
            Key::PageDown => moved(self.page, PAGE as isize, self.pages.len()),
            Key::Right | Key::Ok => {
                self.enter_rows(ctx);
                return Effect::None;
            }
            Key::Back => return Effect::Close,
            Key::Left => self.page,
        };
        if to != self.page {
            self.page = to;
            self.focus = None;
        }
        Effect::None
    }

    /// Focus moves into the rows: the remembered row, else the first one
    /// the app acts on, else the first row.
    fn enter_rows(&mut self, ctx: &Ctx) {
        let rows = self.rows(ctx);
        let keys: Vec<&'static str> = rows.iter().filter_map(Row::key).collect();
        if keys.is_empty() {
            return;
        }
        if self.focus.is_none_or(|k| !keys.contains(&k)) {
            let active = rows
                .iter()
                .find(|r| r.key().is_some() && ctx.status(r) == Status::Active);
            self.focus = active.and_then(Row::key).or(Some(keys[0]));
        }
        self.pane = Pane::Rows;
    }

    fn rows_key(&mut self, key: Key, ctx: &Ctx) -> Effect {
        // The rows that take the focus (not headings), and their keys.
        let rows: Vec<Row> = self
            .rows(ctx)
            .into_iter()
            .filter(|r| r.key().is_some())
            .collect();
        let keys: Vec<&'static str> = rows.iter().filter_map(Row::key).collect();
        if keys.is_empty() {
            self.pane = Pane::Pages;
            return Effect::None;
        }
        // A focused row that left (its visibility changed) counts as the first.
        let at = match self.focus.and_then(|k| keys.iter().position(|x| *x == k)) {
            Some(at) => at,
            // Nothing is highlighted: OK shows the focus on the first row
            // rather than acting on a row nobody saw focused.
            None if key == Key::Ok => {
                self.focus = Some(keys[0]);
                return Effect::None;
            }
            None => 0,
        };
        let step = |by: isize| Some(keys[moved(at, by, keys.len())]);
        match key {
            Key::Up => self.focus = step(-1),
            Key::Down => self.focus = step(1),
            Key::PageUp => self.focus = step(-(PAGE as isize)),
            Key::PageDown => self.focus = step(PAGE as isize),
            Key::Left | Key::Back => self.pane = Pane::Pages,
            Key::Right => {}
            Key::Ok => return self.activate(rows[at], ctx),
        }
        Effect::None
    }

    /// OK (or a tap) on `row`.
    fn activate(&mut self, row: Row, ctx: &Ctx) -> Effect {
        let active = ctx.status(&row) == Status::Active;
        let s = match row {
            Row::Setting(s) if active => s,
            Row::Command(c) if active && c.confirm => {
                self.editor = Some(Editor::Confirm { command: c });
                return Effect::None;
            }
            Row::Command(c) if active => return Effect::Run(c.key),
            // Rows the app does not act on; headings (never focused or
            // tapped) are read-only.
            _ => return Effect::None,
        };
        self.editor = match ctx.value(s) {
            Value::Bool(b) => {
                return Effect::Set {
                    key: s.key,
                    value: Value::Bool(!b),
                };
            }
            Value::Enum(v) | Value::Color(v) => {
                let variants = s.ty.editor_choices().map_or(&[][..], |c| c.variants);
                let selected = variants.iter().position(|x| *x == v).unwrap_or(0);
                Some(Editor::Choice {
                    setting: s,
                    focus: selected,
                    selected,
                })
            }
            Value::Int(value) | Value::Duration(value) => {
                Some(Editor::Number { setting: s, value })
            }
            Value::Text(text) if s.ty.editor_choices().is_some() => {
                let variants = s.ty.editor_choices().map_or(&[][..], |c| c.variants);
                let selected = variants
                    .iter()
                    .position(|v| v.eq_ignore_ascii_case(&text))
                    .unwrap_or(usize::MAX);
                Some(Editor::Choice {
                    setting: s,
                    focus: if selected == usize::MAX { 0 } else { selected },
                    selected,
                })
            }
            Value::Text(text) => Some(Editor::Text {
                setting: s,
                text,
                invalid: false,
            }),
            // Needed for the match over `Value`: key bindings are read-only
            // (`Ctx::status`), so their rows never get here.
            Value::KeyBindings(_) => None,
        };
        Effect::None
    }

    fn editor_key(&mut self, key: Key) -> Effect {
        if key == Key::Back {
            self.cancel();
            return Effect::None;
        }
        let by = match key {
            Key::Up => -1,
            Key::Down => 1,
            Key::PageUp => -(PAGE as isize),
            Key::PageDown => PAGE as isize,
            _ => 0,
        };
        match &mut self.editor {
            Some(Editor::Choice { setting, focus, .. }) => {
                if key == Key::Ok {
                    let at = *focus;
                    return self.pick(at);
                }
                // Left/Right do nothing in a list (as in menus).
                let len = setting.ty.editor_choices().map_or(0, |c| c.variants.len());
                *focus = moved(*focus, by, len);
            }
            Some(Editor::Number { .. }) => match key {
                Key::Left | Key::Down => self.step(-1),
                Key::Right | Key::Up => self.step(1),
                Key::PageDown => self.step(-BIG_STEP),
                Key::PageUp => self.step(BIG_STEP),
                _ => return self.commit(),
            },
            // Typing belongs to the text field; OK keeps the text.
            Some(Editor::Text { .. } | Editor::Confirm { .. }) if key == Key::Ok => {
                return self.commit();
            }
            _ => {}
        }
        Effect::None
    }

    /// A tap on page `index`.
    pub fn tap_page(&mut self, index: usize) {
        self.editor = None;
        self.pane = Pane::Pages;
        if index != self.page && index < self.pages.len() {
            self.page = index;
            self.focus = None;
        }
    }

    /// A tap on a row: it takes the focus and acts as OK.
    pub fn tap_row(&mut self, key: &str, ctx: &Ctx) -> Effect {
        self.editor = None;
        let Some(row) = self.rows(ctx).into_iter().find(|r| r.key() == Some(key)) else {
            return Effect::None;
        };
        self.pane = Pane::Rows;
        self.focus = row.key();
        self.activate(row, ctx)
    }

    /// Picks entry `index` of an open choice list.
    pub fn pick(&mut self, index: usize) -> Effect {
        let Some(Editor::Choice { setting, .. }) = self.editor else {
            return Effect::None;
        };
        let Some(variant) = setting
            .ty
            .editor_choices()
            .and_then(|c| c.variants.get(index).copied())
        else {
            return Effect::None;
        };
        self.editor = None;
        let value = match setting.ty {
            ValueType::Color { .. } => Value::color(variant),
            ValueType::Text { .. } => Value::text(variant),
            _ => Value::choice(variant),
        };
        Effect::Set {
            key: setting.key,
            value,
        }
    }

    /// Moves an open stepper by `steps` (negative: less), within range.
    pub fn step(&mut self, steps: i64) {
        if let Some(Editor::Number { setting, value }) = &mut self.editor
            && let Some((min, max, step)) = validate::grid(&setting.ty)
        {
            let next = i128::from(*value) + i128::from(steps) * i128::from(step);
            if let Ok(next) = validate::clamp_wide_to_grid(next, min, max, step) {
                *value = next;
            }
        }
    }

    pub fn set_text(&mut self, text: &str) {
        if let Some(Editor::Text {
            text: t, invalid, ..
        }) = &mut self.editor
        {
            text.clone_into(t);
            *invalid = false;
        }
    }

    /// OK in an editor: keeps the stepper's value or the typed text, picks
    /// the focused choice, confirms a command. A text that does not
    /// validate keeps the editor open, marked invalid.
    pub fn commit(&mut self) -> Effect {
        let (setting, value) = match &self.editor {
            Some(Editor::Number { setting, value }) => {
                let v = match setting.ty {
                    ValueType::Int { .. } => Value::Int(*value),
                    _ => Value::Duration(*value),
                };
                (*setting, v)
            }
            Some(Editor::Text { setting, text, .. }) => (*setting, Value::Text(text.clone())),
            Some(Editor::Choice { focus, .. }) => return self.pick(*focus),
            Some(Editor::Confirm { command }) => {
                let key = command.key;
                self.editor = None;
                return Effect::Run(key);
            }
            None => {
                self.editor = None;
                return Effect::None;
            }
        };
        if validate::check(setting, &value).is_err() {
            if let Some(Editor::Text { invalid, .. }) = &mut self.editor {
                *invalid = true;
            }
            return Effect::None;
        }
        self.editor = None;
        Effect::Set {
            key: setting.key,
            value,
        }
    }

    /// Closes the editor without storing or running anything.
    pub fn cancel(&mut self) {
        self.editor = None;
    }
}
