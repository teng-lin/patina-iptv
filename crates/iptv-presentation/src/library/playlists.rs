//! The playlists screen's state: the stored playlists, "Refresh all" and
//! "Add playlist" rows, a sheet of actions for one playlist, its provider
//! details form (`details.rs`), a delete confirmation, and text entry
//! (rename, User-Agent, a detail), and the playlists' manual order
//! (the shared [`Reorder`] mode). Focus is kept by identity (a
//! playlist's id), never by list position. Pure: the view feeds it remote
//! keys and taps and carries out the [`Effect`]s it returns.
//!
//! Opened for one task from elsewhere (a playlist's actions from the
//! settings, its rename or the reorder from the groups column's menu:
//! [`Only`]), the screen is [`Screen::finished`] once that task ends,
//! instead of falling back to its list.

use iptv_library::SourceId;
use iptv_providers::model::{Source, SourceKind};

use super::details::{Form, Param};
use iptv_library::Playlist;

use crate::reorder::{self, Exit, LeaveChoice, Outcome, Reorder};

/// A focusable row of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Playlist(SourceId),
    /// Shown with two playlists or more.
    RefreshAll,
    AllPlaylists,
    Add,
}

/// What can be done to one playlist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Show,
    /// Browse every enabled playlist together (with two or more).
    AllPlaylists,
    Enabled,
    RefreshInterval,
    RefreshOnStart,
    /// Its movie and series import, on or off (not offered for M3U, which
    /// has none).
    OnDemand,
    Channels,
    Groups,
    CustomGroups,
    Refresh,
    Rename,
    /// Link, server and login, or portal and MAC address.
    Details,
    UserAgent,
    /// Its streams-at-once ceiling: Automatic (the allowance its provider
    /// reports) or a number; each OK moves to the next ([`next_streams`]).
    Streams,
    /// Its guide sources (the guide sources screen).
    Guides,
    Delete,
}

/// The streams-at-once values the action steps through after Automatic
/// (`None`); `playlists.max_connections` allows 1 to 20.
pub const STREAMS: [u32; 10] = [1, 2, 3, 4, 5, 6, 8, 10, 15, 20];

/// The value after `current`: Automatic, then each of [`STREAMS`], then
/// Automatic again. A value between two steps goes on to the larger.
pub fn next_streams(current: Option<u32>) -> Option<u32> {
    match current {
        None => Some(STREAMS[0]),
        Some(n) => STREAMS.into_iter().find(|step| *step > n),
    }
}

/// The one task the screen was opened for, if not its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Only {
    /// One playlist's actions (the settings' playlist rows).
    Actions,
    /// Renaming one playlist.
    Rename,
    /// Arranging the playlists by hand.
    Reorder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    List,
    /// The playlists in their manual order, being arranged
    /// ([`Screen::reorder`]). Leaving with changes asks first: `save`
    /// is the focused answer.
    Reorder {
        confirming: bool,
        save: bool,
    },
    Actions {
        id: SourceId,
        focus: Act,
    },
    /// Delete `id`? Keeping it is focused first.
    Confirm {
        id: SourceId,
        delete: bool,
    },
    /// The provider details form of `id` ([`Screen::details`]).
    Details {
        id: SourceId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Rename,
    UserAgent,
    Param(Param),
}

/// Text entry over the actions sheet.
#[derive(Clone, PartialEq, Eq)]
pub struct Editing {
    pub id: SourceId,
    pub kind: EditKind,
    pub text: String,
    /// The text started as the current value. A detail carrying
    /// credentials starts empty instead (it is never shown).
    pub prefilled: bool,
}

/// Why the last entry changed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    NameEmpty,
    /// "Check and save" with nothing edited.
    Unchanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    /// Reorder mode only: the first item, or move it to the top.
    Left,
    /// Reorder mode only: the last item, or move it to the bottom.
    Right,
    Ok,
    Back,
    Menu,
}

/// What the screen asks the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Open the add-playlist wizard.
    Add,
    Show(SourceId),
    Policy(SourceId, iptv_library::SourcePolicy),
    /// Switch the movie and series import of `id` (its own setting).
    ToggleOnDemand(SourceId),
    Organize(SourceId, bool),
    CustomGroups(SourceId),
    Refresh(SourceId),
    Rename(SourceId, String),
    /// Open the guide sources screen of `id`.
    Guides(SourceId),
    Delete(SourceId),
    RefreshAll,
    AllPlaylists,
    /// Load the stored details of `id` for [`Screen::open_details`].
    OpenDetails(SourceId),
    /// Import these new details; they replace the stored ones if they work.
    TryDetails(SourceId, Box<Source>),
    SetUserAgent(SourceId, Option<String>),
    /// Move `id`'s streams-at-once ceiling on ([`next_streams`] of its own
    /// value, which the app reads).
    CycleStreams(SourceId),
    /// Keep this manual order of the playlists.
    Reorder(Vec<SourceId>),
    Close,
}

/// Whether `link` can be shown as it is: no user info, no query (where
/// accounts and tokens go).
pub fn plain_link(link: &str) -> bool {
    let rest = link.split_once("://").map_or(link, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    !authority.contains('@') && !link.contains('?')
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    rows: Vec<Playlist>,
    focus: Item,
    layer: Layer,
    editing: Option<Editing>,
    notice: Option<Notice>,
    details: Option<Form>,
    order: Option<Reorder<SourceId>>,
    only: Option<Only>,
}

impl Screen {
    /// The list with the playlist on screen (`shown`) focused.
    pub fn new(rows: Vec<Playlist>, shown: Option<SourceId>) -> Self {
        let focus = shown
            .filter(|id| rows.iter().any(|r| r.id == *id))
            .or_else(|| rows.first().map(|r| r.id))
            .map_or(Item::Add, Item::Playlist);
        Self {
            rows,
            focus,
            layer: Layer::List,
            editing: None,
            notice: None,
            details: None,
            order: None,
            only: None,
        }
    }

    /// The screen opened for one task ([`Only`]): `id`'s actions, its
    /// rename, or the reorder (focused on `id`). `None` when `id` is not
    /// one of `rows` (the reorder needs two playlists or more).
    pub fn only(rows: Vec<Playlist>, id: SourceId, only: Only) -> Option<Self> {
        let mut screen = Self::new(rows, Some(id));
        let opened = match only {
            Only::Actions => screen.open_actions(id, Act::Show),
            Only::Rename => {
                let opened = screen.open_actions(id, Act::Rename);
                if opened {
                    screen.act(id, Act::Rename);
                }
                opened
            }
            Only::Reorder => screen.open_reorder(Some(id)),
        };
        screen.only = Some(only);
        opened.then_some(screen)
    }

    /// What the screen was opened for, if not its list.
    pub fn opened_for(&self) -> Option<Only> {
        self.only
    }

    /// Opened for one task ([`Screen::only`]) and that task is over: the
    /// app closes the screen.
    pub fn finished(&self) -> bool {
        let Some(only) = self.only else {
            return false;
        };
        if self.editing.is_some() || self.notice.is_some() {
            return false;
        }
        self.layer == Layer::List || (only == Only::Rename && self.details.is_none())
    }

    /// Starts arranging the playlists by hand, focused on `focus`.
    /// Whether it could (two playlists or more).
    pub fn open_reorder(&mut self, focus: Option<SourceId>) -> bool {
        if self.rows.len() < 2 {
            return false;
        }
        let ids: Vec<SourceId> = self.rows.iter().map(|r| r.id).collect();
        self.order = Some(Reorder::new(ids, focus.as_ref()));
        self.editing = None;
        self.layer = Layer::Reorder {
            confirming: false,
            save: true,
        };
        true
    }

    /// The playlists being arranged ([`Layer::Reorder`]).
    pub fn reorder(&self) -> Option<&Reorder<SourceId>> {
        self.order.as_ref()
    }

    fn reorder_key(&mut self, key: Key, confirming: bool, save: bool) -> Option<Effect> {
        let order = self.order.as_mut()?;
        let outcome = if confirming {
            match key {
                Key::Up | Key::Down | Key::Left | Key::Right => {
                    self.layer = Layer::Reorder {
                        confirming,
                        save: !save,
                    };
                    return None;
                }
                Key::Ok => order.resolve_leave(if save {
                    LeaveChoice::Save
                } else {
                    LeaveChoice::Discard
                }),
                Key::Back | Key::Menu => order.resolve_leave(LeaveChoice::Stay),
            }
        } else {
            let key = match key {
                Key::Up => reorder::Key::Up,
                Key::Down => reorder::Key::Down,
                Key::Left => reorder::Key::Left,
                Key::Right => reorder::Key::Right,
                Key::Ok => reorder::Key::Ok,
                Key::Back | Key::Menu => reorder::Key::Back,
            };
            order.key(key)
        };
        self.reorder_outcome(outcome)
    }

    fn reorder_outcome(&mut self, outcome: Outcome<SourceId>) -> Option<Effect> {
        match outcome {
            Outcome::ConfirmLeave => {
                self.layer = Layer::Reorder {
                    confirming: true,
                    save: true,
                };
                None
            }
            Outcome::Done(exit) => {
                let order = self.order.take()?;
                if let Some(id) = order.focused() {
                    self.focus = Item::Playlist(*id);
                }
                self.layer = Layer::List;
                match exit {
                    Exit::Saved(ids) => {
                        let at = |id: SourceId| ids.iter().position(|o| *o == id);
                        self.rows.sort_by_key(|r| at(r.id));
                        Some(Effect::Reorder(ids))
                    }
                    Exit::Unchanged | Exit::Discarded => None,
                }
            }
            _ => {
                if self.order.as_ref().is_some_and(|o| !o.is_confirming()) {
                    self.layer = Layer::Reorder {
                        confirming: false,
                        save: true,
                    };
                }
                None
            }
        }
    }

    /// A tap on a playlist while arranging: focus it, or (focused) pick it
    /// up or put it down.
    pub fn tap_reorder(&mut self, id: SourceId) -> Option<Effect> {
        let order = self.order.as_mut()?;
        if order.is_confirming() {
            return None;
        }
        let outcome = if order.focused() == Some(&id) {
            order.toggle_selection()
        } else {
            order.focus_on(&id)
        };
        self.reorder_outcome(outcome)
    }

    /// A tap on the leave confirmation's answer while arranging.
    pub fn resolve_reorder(&mut self, choice: LeaveChoice) -> Option<Effect> {
        let order = self.order.as_mut()?;
        let outcome = if order.is_confirming() {
            order.resolve_leave(choice)
        } else {
            match choice {
                LeaveChoice::Save => order.commit(),
                LeaveChoice::Discard => order.cancel(),
                LeaveChoice::Stay => return None,
            }
        };
        self.reorder_outcome(outcome)
    }

    /// Opens the actions of `id` with `act` focused (from another screen,
    /// such as an empty Movies screen's playlist settings). Whether it could.
    pub fn open_actions(&mut self, id: SourceId, act: Act) -> bool {
        let acts = self.acts(id);
        if acts.is_empty() {
            return false;
        }
        let focus = if acts.contains(&act) { act } else { acts[0] };
        self.focus = Item::Playlist(id);
        self.layer = Layer::Actions { id, focus };
        true
    }

    pub fn focus_all(&mut self) {
        if self.items().contains(&Item::AllPlaylists) {
            self.focus = Item::AllPlaylists;
        }
    }

    pub fn rows(&self) -> &[Playlist] {
        &self.rows
    }

    pub fn row(&self, id: SourceId) -> Option<&Playlist> {
        self.rows.iter().find(|r| r.id == id)
    }

    pub fn focus(&self) -> Item {
        self.focus
    }

    pub fn layer(&self) -> Layer {
        self.layer
    }

    pub fn editing(&self) -> Option<&Editing> {
        self.editing.as_ref()
    }

    pub fn notice(&self) -> Option<Notice> {
        self.notice
    }

    pub fn details(&self) -> Option<&Form> {
        self.details.as_ref()
    }

    /// The list's rows in order: the playlists, "Refresh all" (with two or
    /// more), then "Add playlist".
    pub fn items(&self) -> Vec<Item> {
        let mut items: Vec<Item> = self.rows.iter().map(|r| Item::Playlist(r.id)).collect();
        if !self.rows.is_empty() {
            items.push(Item::AllPlaylists);
        }
        if self.rows.len() > 1 {
            items.push(Item::RefreshAll);
        }
        items.push(Item::Add);
        items
    }

    /// The actions offered for `id`.
    pub fn acts(&self, id: SourceId) -> Vec<Act> {
        let Some(row) = self.row(id) else {
            return Vec::new();
        };
        let mut acts = Vec::new();
        if row.policy.enabled {
            acts.push(Act::Show);
            if self.rows.iter().filter(|r| r.policy.enabled).count() > 1 {
                acts.push(Act::AllPlaylists);
            }
        }
        acts.extend([
            Act::Refresh,
            Act::Rename,
            Act::Enabled,
            Act::RefreshInterval,
            Act::RefreshOnStart,
        ]);
        if row.kind != SourceKind::M3u {
            acts.push(Act::OnDemand);
        }
        acts.extend([Act::Channels, Act::Groups]);
        // A file has no link to change (adding it again picks a new one).
        if !row.from_file {
            acts.push(Act::Details);
        }
        if row.kind != SourceKind::Stalker {
            acts.push(Act::UserAgent);
        }
        acts.push(Act::Streams);
        acts.push(Act::Guides);
        acts.push(Act::CustomGroups);
        acts.push(Act::Delete);
        acts
    }

    /// New rows from the store: focus stays on the same playlist; when it
    /// is gone, on the playlist that took its place (else the one before
    /// it, else "Add playlist").
    pub fn set_rows(&mut self, rows: Vec<Playlist>) {
        let at = self.items().iter().position(|i| *i == self.focus);
        self.rows = rows;
        if !self.items().contains(&self.focus) {
            self.focus = match (self.focus, self.rows.len()) {
                (Item::Playlist(_), n) if n > 0 => {
                    Item::Playlist(self.rows[at.unwrap_or(0).min(n - 1)].id)
                }
                _ => Item::Add,
            };
        }
        if self.order.is_some() && self.rows.len() < 2 {
            self.order = None;
            self.layer = Layer::List;
        }
        if let Some((mut was, focus)) = self
            .order
            .as_ref()
            .map(|o| (o.original().to_vec(), o.focused().copied()))
        {
            // A playlist added or deleted meanwhile: arrange again from
            // the new list, keeping the focus.
            let mut now: Vec<SourceId> = self.rows.iter().map(|r| r.id).collect();
            now.sort();
            was.sort();
            if now != was {
                let layer = self.layer;
                self.open_reorder(focus);
                self.layer = match layer {
                    Layer::Reorder { .. } => Layer::Reorder {
                        confirming: false,
                        save: true,
                    },
                    other => other,
                };
            }
        }
        let gone = |id| !self.rows.iter().any(|r| r.id == id);
        match self.layer {
            Layer::Actions { id, .. } | Layer::Confirm { id, .. } | Layer::Details { id }
                if gone(id) =>
            {
                self.layer = Layer::List;
                self.editing = None;
                self.details = None;
            }
            Layer::Actions { id, focus } if !self.acts(id).contains(&focus) => {
                self.layer = Layer::Actions {
                    id,
                    focus: self.acts(id).first().copied().unwrap_or(Act::Enabled),
                };
            }
            _ => {}
        }
    }

    pub fn key(&mut self, key: Key) -> Option<Effect> {
        self.notice = None;
        if self.editing.is_some() {
            return matches!(key, Key::Ok | Key::Back)
                .then(|| self.commit())
                .flatten();
        }
        match self.layer {
            Layer::List => self.list_key(key),
            Layer::Reorder { confirming, save } => self.reorder_key(key, confirming, save),
            Layer::Actions { id, focus } => self.actions_key(id, focus, key),
            Layer::Confirm { id, delete } => match key {
                Key::Up | Key::Down => {
                    self.layer = Layer::Confirm {
                        id,
                        delete: !delete,
                    };
                    None
                }
                Key::Ok => self.confirm(id, delete),
                Key::Back | Key::Menu => self.confirm(id, false),
                Key::Left | Key::Right => None,
            },
            Layer::Details { id } => self.details_key(id, key),
        }
    }

    fn list_key(&mut self, key: Key) -> Option<Effect> {
        let items = self.items();
        let at = items.iter().position(|i| *i == self.focus).unwrap_or(0);
        match (key, self.focus) {
            (Key::Up, _) => self.focus = items[at.saturating_sub(1)],
            (Key::Down, _) => self.focus = items[(at + 1).min(items.len() - 1)],
            (Key::Ok, Item::Add) => return Some(Effect::Add),
            (Key::Ok, Item::RefreshAll) => return Some(Effect::RefreshAll),
            (Key::Ok, Item::AllPlaylists) => return Some(Effect::AllPlaylists),
            (Key::Ok | Key::Menu, Item::Playlist(id)) => {
                self.layer = Layer::Actions {
                    id,
                    focus: self.acts(id).first().copied().unwrap_or(Act::Enabled),
                };
            }
            (Key::Back, _) => return Some(Effect::Close),
            (Key::Menu | Key::Left | Key::Right, _) => {}
        }
        None
    }

    fn actions_key(&mut self, id: SourceId, focus: Act, key: Key) -> Option<Effect> {
        let acts = self.acts(id);
        let at = acts.iter().position(|a| *a == focus).unwrap_or(0);
        let to = match key {
            Key::Up => acts[at.saturating_sub(1)],
            Key::Down => acts[(at + 1).min(acts.len() - 1)],
            Key::Ok => return self.act(id, focus),
            Key::Back | Key::Menu => {
                self.layer = Layer::List;
                return None;
            }
            Key::Left | Key::Right => return None,
        };
        self.layer = Layer::Actions { id, focus: to };
        None
    }

    /// OK (or a tap) on `act` of playlist `id`.
    pub fn act(&mut self, id: SourceId, act: Act) -> Option<Effect> {
        let row = self.row(id)?.clone();
        self.notice = None;
        self.layer = Layer::Actions { id, focus: act };
        let to_list = |s: &mut Self, effect| {
            s.layer = Layer::List;
            s.focus = Item::Playlist(id);
            Some(effect)
        };
        match act {
            Act::Enabled | Act::RefreshInterval | Act::RefreshOnStart => {
                let mut policy = row.policy;
                match act {
                    Act::Enabled => policy.enabled = !policy.enabled,
                    Act::RefreshOnStart => policy.refresh_on_start = !policy.refresh_on_start,
                    _ => {
                        let values = [0, 3600, 21600, 43200, 86400];
                        let at = values
                            .iter()
                            .position(|v| *v == policy.refresh_seconds)
                            .unwrap_or(0);
                        policy.refresh_seconds = values[(at + 1) % values.len()];
                    }
                }
                Some(Effect::Policy(id, policy))
            }
            Act::OnDemand => Some(Effect::ToggleOnDemand(id)),
            Act::Streams => Some(Effect::CycleStreams(id)),
            Act::CustomGroups => Some(Effect::CustomGroups(id)),
            Act::Channels | Act::Groups => Some(Effect::Organize(id, act == Act::Groups)),
            Act::Show => to_list(self, Effect::Show(id)),
            Act::AllPlaylists => to_list(self, Effect::AllPlaylists),
            Act::Refresh => to_list(self, Effect::Refresh(id)),
            // The guide sources screen opens over the actions.
            Act::Guides => Some(Effect::Guides(id)),
            Act::Rename => {
                self.editing = Some(Editing {
                    id,
                    kind: EditKind::Rename,
                    text: row.name,
                    prefilled: true,
                });
                None
            }
            Act::Delete => {
                self.layer = Layer::Confirm { id, delete: false };
                None
            }
            Act::Details => Some(Effect::OpenDetails(id)),
            Act::UserAgent => {
                self.editing = Some(Editing {
                    id,
                    kind: EditKind::UserAgent,
                    text: row.user_agent.unwrap_or_default(),
                    prefilled: true,
                });
                None
            }
        }
    }

    /// The stored details of the playlist whose actions are open, loaded:
    /// the form opens on them.
    pub fn open_details(&mut self, source: Source) {
        if let Layer::Actions { id, .. } = self.layer {
            self.details = Some(Form::new(id, source));
            self.layer = Layer::Details { id };
        }
    }

    fn details_key(&mut self, id: SourceId, key: Key) -> Option<Effect> {
        match key {
            Key::Up | Key::Down => {
                let step = if key == Key::Up { -1 } else { 1 };
                self.details.iter_mut().for_each(|f| f.move_focus(step));
                None
            }
            Key::Ok => match self.details.as_ref()?.focus {
                Some(p) => self.edit_param(p),
                None => self.check(),
            },
            Key::Back | Key::Menu => {
                self.details = None;
                self.layer = Layer::Actions {
                    id,
                    focus: Act::Details,
                };
                None
            }
            Key::Left | Key::Right => None,
        }
    }

    /// Text entry for detail `p`: started with its value when it can be
    /// shown, else empty (left empty, the value stays).
    pub fn edit_param(&mut self, p: Param) -> Option<Effect> {
        let form = self.details.as_mut()?;
        if form.checking {
            return None;
        }
        form.focus = Some(p);
        let prefill = form.prefill(p);
        self.editing = Some(Editing {
            id: form.id,
            kind: EditKind::Param(p),
            prefilled: prefill.is_some(),
            text: prefill.unwrap_or_default(),
        });
        None
    }

    /// "Check and save": imports the edited details (once at a time).
    pub fn check(&mut self) -> Option<Effect> {
        let form = self.details.as_mut()?;
        form.focus = None;
        if form.checking {
            return None;
        }
        if !form.changed() {
            self.notice = Some(Notice::Unchanged);
            return None;
        }
        form.checking = true;
        Some(Effect::TryDetails(form.id, Box::new(form.edited().clone())))
    }

    /// The check of `id`'s new details ended: saved, the form closes;
    /// refused, it stays with the edits for another try.
    pub fn details_checked(&mut self, id: SourceId, saved: bool) {
        let Some(form) = self.details.as_mut().filter(|f| f.id == id && f.checking) else {
            return;
        };
        form.checking = false;
        if saved {
            self.details = None;
            self.layer = Layer::List;
            self.focus = Item::Playlist(id);
        }
    }

    /// The confirmation's answer: delete, or back to the actions.
    pub fn confirm(&mut self, id: SourceId, delete: bool) -> Option<Effect> {
        if delete {
            self.layer = Layer::List;
            self.focus = Item::Playlist(id);
            return Some(Effect::Delete(id));
        }
        self.layer = Layer::Actions {
            id,
            focus: Act::Delete,
        };
        None
    }

    /// A tap on a list row: focus it and OK.
    pub fn tap(&mut self, item: Item) -> Option<Effect> {
        self.notice = None;
        self.editing = None;
        self.layer = Layer::List;
        self.focus = item;
        self.list_key(Key::Ok)
    }

    /// A tap beside an open sheet: as Back (the details form stays: its
    /// edits would be lost).
    pub fn dismiss(&mut self) -> Option<Effect> {
        if matches!(self.layer, Layer::Details { .. }) {
            return None;
        }
        match self.layer {
            Layer::List => None,
            _ => self.key(Key::Back),
        }
    }

    pub fn set_text(&mut self, text: &str) {
        if let Some(e) = &mut self.editing {
            e.text = text.to_owned();
        }
    }

    pub fn cancel_edit(&mut self) {
        self.editing = None;
    }

    /// Ends text entry and decides what the text changes.
    pub fn commit(&mut self) -> Option<Effect> {
        let editing = self.editing.take()?;
        let id = editing.id;
        let row = self.row(id)?.clone();
        let text = editing.text.trim();
        let effect = match editing.kind {
            // Left empty when it started empty: the value stays.
            EditKind::Param(_) if text.is_empty() && !editing.prefilled => None,
            EditKind::Param(p) => {
                if let Some(form) = &mut self.details {
                    // An error stays in the form, beside the detail.
                    let _ = form.set(p, &editing.text);
                }
                return None;
            }
            EditKind::UserAgent => {
                let agent = (!text.is_empty()).then(|| text.to_owned());
                (agent != row.user_agent).then_some(Effect::SetUserAgent(id, agent))
            }
            EditKind::Rename if text.is_empty() => {
                self.notice = Some(Notice::NameEmpty);
                None
            }
            EditKind::Rename if text == row.name => None,
            EditKind::Rename => Some(Effect::Rename(id, text.to_owned())),
        };
        if effect.is_some() {
            self.layer = Layer::List;
            self.focus = Item::Playlist(id);
        }
        effect
    }
}

#[cfg(test)]
mod tests {
    use iptv_providers::model::SourceKind;

    use super::*;

    const A: SourceId = SourceId(1);
    const B: SourceId = SourceId(2);
    const C: SourceId = SourceId(3);

    fn row(id: SourceId, name: &str) -> Playlist {
        Playlist {
            id,
            name: name.to_owned(),
            kind: SourceKind::M3u,
            channels: 10,
            has_movies: false,
            has_series: false,
            movies_failed: None,
            series_failed: None,
            updated_at: Some(1_800_000_000),
            failed: None,
            guides: Vec::new(),
            guide_failed: false,
            user_agent: None,
            from_file: false,
            policy: Default::default(),
        }
    }

    fn rows() -> Vec<Playlist> {
        vec![row(A, "First"), row(B, "Second"), row(C, "Third")]
    }

    #[test]
    fn opens_on_the_shown_playlist_and_moves_by_identity() {
        let mut s = Screen::new(rows(), Some(B));
        assert_eq!(s.focus(), Item::Playlist(B));
        s.key(Key::Down);
        s.key(Key::Down);
        assert_eq!(s.focus(), Item::AllPlaylists);
        assert_eq!(s.key(Key::Ok), Some(Effect::AllPlaylists));
        s.key(Key::Down);
        assert_eq!(s.focus(), Item::RefreshAll);
        assert_eq!(s.key(Key::Ok), Some(Effect::RefreshAll));
        s.key(Key::Down);
        assert_eq!(s.focus(), Item::Add);
        s.key(Key::Down);
        assert_eq!(s.focus(), Item::Add, "no wrap");
        assert_eq!(s.key(Key::Ok), Some(Effect::Add));
        assert_eq!(Screen::new(vec![], None).focus(), Item::Add);
        assert_eq!(
            Screen::new(rows(), Some(SourceId(9))).focus(),
            Item::Playlist(A)
        );
        assert_eq!(s.key(Key::Back), Some(Effect::Close));
    }

    #[test]
    fn new_rows_keep_the_focused_playlist() {
        let mut s = Screen::new(rows(), Some(C));
        // B deleted elsewhere: C keeps the focus though its index moved.
        s.set_rows(vec![row(A, "First"), row(C, "Third")]);
        assert_eq!(s.focus(), Item::Playlist(C));
        // The focused playlist is deleted: the row that took its place.
        let mut s = Screen::new(rows(), Some(B));
        s.set_rows(vec![row(A, "First"), row(C, "Third")]);
        assert_eq!(s.focus(), Item::Playlist(C));
        let mut s = Screen::new(rows(), Some(C));
        s.set_rows(vec![row(A, "First"), row(B, "Second")]);
        assert_eq!(s.focus(), Item::Playlist(B), "the one before the last");
        s.set_rows(vec![]);
        assert_eq!(s.focus(), Item::Add);
        // One playlist left: no "Refresh all" row.
        assert_eq!(
            Screen::new(vec![row(A, "First")], None).items(),
            [Item::Playlist(A), Item::AllPlaylists, Item::Add]
        );
    }

    /// #282: each playlist sets its own streams at once, every kind.
    #[test]
    fn streams_at_once_step_from_automatic_through_the_values() {
        let mut s = Screen::new(rows(), Some(A));
        assert!(s.acts(A).contains(&Act::Streams));
        assert_eq!(s.act(A, Act::Streams), Some(Effect::CycleStreams(A)));
        assert_eq!(
            s.layer(),
            Layer::Actions {
                id: A,
                focus: Act::Streams
            },
            "the sheet stays on the action"
        );
        let mut seen = vec![None];
        let mut at = None;
        loop {
            at = next_streams(at);
            if at.is_none() {
                break;
            }
            seen.push(at);
        }
        assert_eq!(seen.len(), STREAMS.len() + 1);
        assert_eq!(next_streams(Some(7)), Some(8), "between two steps");
        assert_eq!(next_streams(Some(20)), None, "back to Automatic");
    }

    #[test]
    fn ok_opens_the_actions_and_show_closes_them() {
        let mut s = Screen::new(rows(), Some(A));
        s.key(Key::Down);
        assert_eq!(s.key(Key::Ok), None);
        assert_eq!(
            s.layer(),
            Layer::Actions {
                id: B,
                focus: Act::Show
            }
        );
        assert_eq!(
            s.acts(B),
            [
                Act::Show,
                Act::AllPlaylists,
                Act::Refresh,
                Act::Rename,
                Act::Enabled,
                Act::RefreshInterval,
                Act::RefreshOnStart,
                Act::Channels,
                Act::Groups,
                Act::Details,
                Act::UserAgent,
                Act::Streams,
                Act::Guides,
                Act::CustomGroups,
                Act::Delete
            ]
        );
        assert_eq!(s.key(Key::Ok), Some(Effect::Show(B)));
        assert_eq!(s.layer(), Layer::List);
        // Menu opens them too; Back closes them, keeping the focus.
        s.key(Key::Menu);
        s.key(Key::Down);
        s.key(Key::Down);
        assert_eq!(s.key(Key::Ok), Some(Effect::Refresh(B)));
        s.key(Key::Ok);
        s.key(Key::Back);
        assert_eq!((s.layer(), s.focus()), (Layer::List, Item::Playlist(B)));
    }

    #[test]
    fn delete_asks_first_and_keeping_is_the_default() {
        let mut s = Screen::new(rows(), Some(A));
        s.key(Key::Ok);
        for _ in 0..100 {
            s.key(Key::Down);
        }
        assert_eq!(
            s.layer(),
            Layer::Actions {
                id: A,
                focus: Act::Delete
            }
        );
        s.key(Key::Ok);
        assert_eq!(
            s.layer(),
            Layer::Confirm {
                id: A,
                delete: false
            }
        );
        // OK on the default keeps it; Back too.
        assert_eq!(s.key(Key::Ok), None);
        s.key(Key::Ok);
        assert_eq!(s.key(Key::Back), None);
        assert!(matches!(s.layer(), Layer::Actions { .. }));
        s.key(Key::Ok);
        s.key(Key::Down);
        assert_eq!(s.key(Key::Ok), Some(Effect::Delete(A)));
        assert_eq!(s.layer(), Layer::List);
    }

    #[test]
    fn rename_ignores_an_empty_or_unchanged_name() {
        let mut s = Screen::new(rows(), Some(A));
        s.act(A, Act::Rename);
        assert_eq!(s.editing().map(|e| e.text.as_str()), Some("First"));
        s.set_text("  ");
        assert_eq!(s.key(Key::Ok), None);
        assert_eq!(s.notice(), Some(Notice::NameEmpty));
        assert!(matches!(s.layer(), Layer::Actions { .. }));
        s.act(A, Act::Rename);
        assert_eq!(s.commit(), None, "unchanged");
        s.act(A, Act::Rename);
        s.set_text(" Evening ");
        assert_eq!(s.key(Key::Back), Some(Effect::Rename(A, "Evening".into())));
        assert_eq!(s.editing(), None);
        // Cancel changes nothing.
        s.act(A, Act::Rename);
        s.set_text("Other");
        s.cancel_edit();
        assert_eq!(s.editing(), None);
    }

    #[test]
    fn guide_sources_open_their_screen() {
        let mut s = Screen::new(rows(), Some(B));
        s.key(Key::Ok);
        assert_eq!(s.act(B, Act::Guides), Some(Effect::Guides(B)));
        // The actions stay under the screen: Back returns to them.
        assert_eq!(
            s.layer(),
            Layer::Actions {
                id: B,
                focus: Act::Guides
            }
        );
        assert!(plain_link("https://guide.example.test/a.xml.gz"));
        assert!(!plain_link("http://user:pass@guide.example.test/a.xml"));
        assert!(!plain_link("http://guide.example.test/a?token=x"));
    }

    #[test]
    fn taps_and_dismissals() {
        let mut s = Screen::new(rows(), Some(A));
        assert_eq!(s.tap(Item::Add), Some(Effect::Add));
        assert_eq!(s.tap(Item::Playlist(C)), None);
        assert_eq!(
            s.layer(),
            Layer::Actions {
                id: C,
                focus: Act::Show
            }
        );
        assert_eq!(s.dismiss(), None);
        assert_eq!((s.layer(), s.focus()), (Layer::List, Item::Playlist(C)));
        assert_eq!(s.dismiss(), None, "nothing to dismiss");
        // The playlist of an open sheet was deleted elsewhere.
        s.act(C, Act::Delete);
        s.set_rows(vec![row(A, "First")]);
        assert_eq!((s.layer(), s.focus()), (Layer::List, Item::Playlist(A)));
    }

    fn xtream_source() -> Source {
        use iptv_providers::Secret;
        use iptv_providers::model::{SourceConfig, XtreamCredentials};
        Source {
            name: "Second".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: "http://panel.example.test".into(),
                username: Secret::new("synthetic-user"),
                password: Secret::new("synthetic-pass"),
                user_agent: None,
            }),
        }
    }

    #[test]
    fn provider_details_are_checked_before_they_replace_the_stored_ones() {
        let mut rows = rows();
        rows[1].kind = SourceKind::Xtream;
        let mut s = Screen::new(rows, Some(B));
        s.key(Key::Ok);
        assert_eq!(s.act(B, Act::Details), Some(Effect::OpenDetails(B)));
        s.open_details(xtream_source());
        assert_eq!(s.layer(), Layer::Details { id: B });
        // Nothing changed: nothing to check.
        assert_eq!(s.check(), None);
        assert_eq!(s.notice(), Some(Notice::Unchanged));
        // The check button is last; above it, the password, which starts
        // empty; left empty, it stays.
        assert_eq!(s.details().unwrap().focus, None);
        s.key(Key::Up);
        s.key(Key::Ok);
        let e = s.editing().unwrap();
        assert_eq!((e.text.as_str(), e.prefilled), ("", false));
        assert_eq!(s.commit(), None);
        assert!(!s.details().unwrap().changed());
        s.edit_param(Param::Password);
        s.set_text("another-pass");
        s.key(Key::Ok);
        assert!(s.details().unwrap().changed());
        s.key(Key::Down);
        let Some(Effect::TryDetails(id, tried)) = s.key(Key::Ok) else {
            panic!("no check");
        };
        assert_eq!(id, B);
        assert_ne!(*tried, xtream_source());
        // While it is checked, a second check does nothing.
        assert_eq!(s.check(), None);
        // Refused: the form stays with the edits; saved: it closes.
        s.details_checked(B, false);
        assert!(s.details().unwrap().changed());
        assert!(s.check().is_some());
        s.details_checked(B, true);
        assert_eq!((s.layer(), s.details()), (Layer::List, None));
        // Back leaves the form without trying anything.
        s.act(B, Act::Details);
        s.open_details(xtream_source());
        assert_eq!(s.dismiss(), None, "a tap beside it keeps the edits");
        assert_eq!(s.key(Key::Back), None);
        assert_eq!(
            s.layer(),
            Layer::Actions {
                id: B,
                focus: Act::Details
            }
        );
    }

    #[test]
    fn user_agent_and_which_actions_a_playlist_has() {
        let mut rows = rows();
        rows[0].from_file = true;
        rows[1].kind = SourceKind::Stalker;
        rows[2].user_agent = Some("Synthetic/1".into());
        let mut s = Screen::new(rows, Some(A));
        assert!(!s.acts(A).contains(&Act::Details), "a file has no link");
        assert!(!s.acts(B).contains(&Act::UserAgent), "portals send none");
        s.act(C, Act::UserAgent);
        assert_eq!(s.editing().map(|e| e.text.as_str()), Some("Synthetic/1"));
        assert_eq!(s.commit(), None, "unchanged");
        s.act(C, Act::UserAgent);
        s.set_text(" ");
        assert_eq!(s.commit(), Some(Effect::SetUserAgent(C, None)));
        s.act(A, Act::UserAgent);
        s.set_text("Other/2");
        assert_eq!(
            s.commit(),
            Some(Effect::SetUserAgent(A, Some("Other/2".into())))
        );
    }

    #[test]
    fn only_playlists_with_movies_and_series_offer_their_import_switch() {
        let mut rows = rows();
        rows[1].kind = SourceKind::Xtream;
        rows[2].kind = SourceKind::Stalker;
        let mut s = Screen::new(rows, Some(A));
        assert!(!s.acts(A).contains(&Act::OnDemand), "M3U has none");
        assert!(s.acts(B).contains(&Act::OnDemand));
        assert!(s.acts(C).contains(&Act::OnDemand));
        assert_eq!(s.act(B, Act::OnDemand), Some(Effect::ToggleOnDemand(B)));
        // Another screen opens the actions at it; the actions stay open.
        let mut s = Screen::new(s.rows().to_vec(), Some(A));
        assert!(s.open_actions(C, Act::OnDemand));
        assert_eq!(s.focus(), Item::Playlist(C));
        assert_eq!(
            s.layer(),
            Layer::Actions {
                id: C,
                focus: Act::OnDemand
            }
        );
        assert_eq!(s.key(Key::Ok), Some(Effect::ToggleOnDemand(C)));
        assert_eq!(
            s.layer(),
            Layer::Actions {
                id: C,
                focus: Act::OnDemand
            }
        );
        // An act a playlist lacks focuses its first; a gone playlist, nothing.
        assert!(s.open_actions(A, Act::OnDemand));
        assert_eq!(
            s.layer(),
            Layer::Actions {
                id: A,
                focus: Act::Show
            }
        );
        assert!(!s.open_actions(SourceId(9), Act::OnDemand));
    }
    #[test]
    fn all_playlists_is_offered_with_another_enabled_one() {
        let mut s = Screen::new(rows(), Some(A));
        assert!(s.acts(A).contains(&Act::AllPlaylists));
        assert_eq!(s.act(A, Act::AllPlaylists), Some(Effect::AllPlaylists));
        assert_eq!(s.layer(), Layer::List);
        let s = Screen::new(vec![row(A, "First")], Some(A));
        assert!(!s.acts(A).contains(&Act::AllPlaylists));
    }

    fn ids(screen: &Screen) -> Vec<SourceId> {
        screen.reorder().unwrap().items().to_vec()
    }

    #[test]
    fn reorder_moves_a_playlist_and_saves_the_new_order() {
        let mut s = Screen::new(rows(), Some(A));
        assert!(s.open_reorder(Some(B)));
        assert_eq!(s.reorder().unwrap().focused(), Some(&B));
        // OK picks it up, Up moves it, OK puts it down, Back leaves.
        assert_eq!(s.key(Key::Ok), None);
        assert_eq!(s.key(Key::Up), None);
        assert_eq!(ids(&s), [B, A, C]);
        assert_eq!(s.key(Key::Ok), None);
        assert_eq!(s.key(Key::Back), None);
        assert_eq!(
            s.layer(),
            Layer::Reorder {
                confirming: true,
                save: true
            }
        );
        assert_eq!(s.key(Key::Ok), Some(Effect::Reorder(vec![B, A, C])));
        assert_eq!(s.layer(), Layer::List);
        assert_eq!(s.focus(), Item::Playlist(B));
        let names: Vec<SourceId> = s.rows().iter().map(|r| r.id).collect();
        assert_eq!(names, [B, A, C]);
    }

    #[test]
    fn reorder_can_be_discarded_or_left_unchanged() {
        let mut s = Screen::new(rows(), Some(A));
        s.open_reorder(Some(A));
        s.key(Key::Ok);
        s.key(Key::Right);
        assert_eq!(ids(&s), [B, C, A]);
        s.key(Key::Back);
        // Down chooses Discard; OK confirms it.
        s.key(Key::Down);
        assert_eq!(s.key(Key::Ok), None);
        assert_eq!(s.layer(), Layer::List);
        let names: Vec<SourceId> = s.rows().iter().map(|r| r.id).collect();
        assert_eq!(names, [A, B, C]);
        // Nothing moved: Back leaves without asking.
        s.open_reorder(None);
        assert_eq!(s.key(Key::Back), None);
        assert_eq!(s.layer(), Layer::List);
    }

    #[test]
    fn reorder_needs_two_playlists() {
        let mut s = Screen::new(vec![row(A, "First")], Some(A));
        assert!(!s.open_reorder(Some(A)));
        assert!(Screen::only(vec![row(A, "First")], A, Only::Reorder).is_none());
    }

    #[test]
    fn taps_focus_then_pick_up_while_arranging() {
        let mut s = Screen::new(rows(), Some(A));
        s.open_reorder(Some(A));
        s.tap_reorder(C);
        assert_eq!(s.reorder().unwrap().focused(), Some(&C));
        s.tap_reorder(C);
        assert!(s.reorder().unwrap().selected().is_some());
        s.key(Key::Left);
        assert_eq!(ids(&s), [C, A, B]);
        assert_eq!(
            s.resolve_reorder(LeaveChoice::Save),
            Some(Effect::Reorder(vec![C, A, B]))
        );
    }

    #[test]
    fn a_screen_opened_for_one_task_finishes_with_it() {
        // Actions from the settings: Back ends them.
        let mut s = Screen::only(rows(), B, Only::Actions).unwrap();
        assert!(matches!(s.layer(), Layer::Actions { id: B, .. }));
        assert!(!s.finished());
        s.key(Key::Back);
        assert!(s.finished());
        // Rename: the entry is open at once; keeping a new name ends it.
        let mut s = Screen::only(rows(), B, Only::Rename).unwrap();
        assert_eq!(s.editing().map(|e| e.kind), Some(EditKind::Rename));
        s.set_text("Renamed");
        assert_eq!(s.commit(), Some(Effect::Rename(B, "Renamed".into())));
        assert!(s.finished());
        // An empty name says why and stays.
        let mut s = Screen::only(rows(), B, Only::Rename).unwrap();
        s.set_text("  ");
        assert_eq!(s.commit(), None);
        assert!(!s.finished());
        // Reorder: leaving ends it.
        let mut s = Screen::only(rows(), C, Only::Reorder).unwrap();
        assert_eq!(s.reorder().unwrap().focused(), Some(&C));
        s.key(Key::Back);
        assert!(s.finished());
        // The full screen never finishes on its own.
        let mut s = Screen::new(rows(), Some(A));
        s.key(Key::Ok);
        s.key(Key::Back);
        assert!(!s.finished());
    }

    #[test]
    fn new_rows_while_arranging_keep_the_mode_and_its_focus() {
        let mut s = Screen::new(rows(), Some(A));
        s.open_reorder(Some(B));
        s.set_rows(vec![row(A, "First"), row(B, "Second")]);
        assert_eq!(ids(&s), [A, B]);
        assert_eq!(s.reorder().unwrap().focused(), Some(&B));
        s.set_rows(vec![row(A, "First")]);
        assert!(s.reorder().is_none());
        assert_eq!(s.layer(), Layer::List);
    }
}

impl std::fmt::Debug for Editing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editing")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("text", &"[redacted]")
            .field("prefilled", &self.prefilled)
            .finish()
    }
}

#[cfg(test)]
mod editing_diagnostic_regression {
    use super::*;
    #[test]
    fn raw_edit_text_is_redacted() {
        let editing = Editing {
            id: SourceId(1),
            kind: EditKind::Rename,
            text: "synthetic-private-marker".into(),
            prefilled: false,
        };
        assert!(!format!("{editing:?}").contains("synthetic-private-marker"));
    }
}
