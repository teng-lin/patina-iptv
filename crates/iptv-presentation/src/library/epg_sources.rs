//! The guide sources screen's state: one playlist's guide sources (each
//! with its last result), "Add a guide source" and "Update the guide now"
//! rows, a sheet of actions for one source, a removal confirmation, and
//! text entry (a new source's link, then its name; a rename; a new link).
//! Focus is kept by identity (a source's feed id), never by position.
//! Pure: the view feeds it keys and taps and carries out its [`Effect`]s.

use crate::wizard::{Field, FieldError, validate_field};
use iptv_epg::FeedId as Feed;
use iptv_library::SourceId;
use iptv_providers::SecretUrl;

use iptv_library::SourceLine;
use iptv_library::guides::{Edit, MAX_SHIFT_MINUTES};

/// The time shift's step.
pub const SHIFT_STEP: i32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Source(Feed),
    Add,
    Update,
    Inspect,
}

/// What can be done to one source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Switch it on or off.
    Switch,
    Up,
    Down,
    /// Left/Right (or −/+) change it.
    Shift,
    Rename,
    /// A new link (a source the user added).
    Relink,
    /// Only a source the user added; the provider's are switched off.
    Remove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    List,
    Actions {
        feed: Feed,
        focus: Act,
    },
    /// Remove `feed`? Keeping it is focused first.
    Confirm {
        feed: Feed,
        remove: bool,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub enum EditKind {
    /// A new source's link.
    AddLink,
    /// Then its name (may stay empty).
    AddName {
        url: String,
    },
    Rename(Feed),
    Relink(Feed),
}

#[derive(Clone, PartialEq, Eq)]
pub struct Editing {
    pub kind: EditKind,
    pub text: String,
}

/// Why the last entry changed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    Link(FieldError),
    /// The playlist has that link already.
    Duplicate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
    Menu,
}

/// What the screen asks the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Edit(Edit),
    Inspect,
    /// Download the playlist's guide now.
    Update,
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    playlist: SourceId,
    lines: Vec<SourceLine>,
    focus: Item,
    layer: Layer,
    editing: Option<Editing>,
    /// A link that was refused, offered again at the next try.
    retry: Option<Editing>,

    notice: Option<Notice>,
    /// Adding and downloading guides are offered: false where the host
    /// cannot download them.
    downloads: bool,
}

impl Screen {
    pub fn new(playlist: SourceId, lines: Vec<SourceLine>) -> Self {
        let focus = lines
            .first()
            .map_or(Item::Add, |l| Item::Source(l.source.feed));
        Self {
            playlist,
            lines,
            focus,
            layer: Layer::List,
            editing: None,
            retry: None,
            notice: None,
            downloads: true,
        }
    }

    /// Without guide downloads: no Add and no Update row.
    pub fn without_downloads(mut self) -> Self {
        self.downloads = false;
        if !self.items().contains(&self.focus) {
            self.focus = self.items()[0];
        }
        self
    }

    /// The row the list falls back to when its focus is gone.
    fn first_action(&self) -> Item {
        if self.downloads {
            Item::Add
        } else {
            Item::Inspect
        }
    }

    pub fn playlist(&self) -> SourceId {
        self.playlist
    }

    pub fn line(&self, feed: Feed) -> Option<&SourceLine> {
        self.lines.iter().find(|l| l.source.feed == feed)
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

    /// The list's rows: the sources in order, then "Add", then "Update".
    pub fn items(&self) -> Vec<Item> {
        let mut items: Vec<Item> = self
            .lines
            .iter()
            .map(|l| Item::Source(l.source.feed))
            .collect();
        if self.downloads {
            items.extend([Item::Add, Item::Update]);
        }
        items.push(Item::Inspect);
        items
    }

    /// The actions offered for `feed`.
    pub fn acts(&self, feed: Feed) -> Vec<Act> {
        let Some(line) = self.line(feed) else {
            return Vec::new();
        };
        let mut acts = vec![Act::Switch, Act::Up, Act::Down, Act::Shift, Act::Rename];
        if !line.source.is_provider() {
            acts.extend([Act::Relink, Act::Remove]);
        }
        acts
    }

    /// New lines from the store: focus stays on the same source; when it
    /// is gone, on the one that took its place (else "Add").
    pub fn set_lines(&mut self, lines: Vec<SourceLine>) {
        let at = self.items().iter().position(|i| *i == self.focus);
        self.lines = lines;
        if !self.items().contains(&self.focus) {
            self.focus = match self.lines.len() {
                0 => self.first_action(),
                n => Item::Source(self.lines[at.unwrap_or(0).min(n - 1)].source.feed),
            };
        }
        match self.layer {
            Layer::Actions { feed, .. } | Layer::Confirm { feed, .. }
                if self.line(feed).is_none() =>
            {
                self.layer = Layer::List;
            }
            _ => {}
        }
    }

    /// Opens on entering a new source's link ("Add a guide source").
    pub fn start_add(&mut self) {
        if !self.downloads {
            return;
        }
        self.focus = Item::Add;
        self.layer = Layer::List;
        let fresh = Editing {
            kind: EditKind::AddLink,
            text: String::new(),
        };
        self.editing = Some(self.retried(EditKind::AddLink).unwrap_or(fresh));
    }

    /// The refused entry of `kind`, to try again.
    fn retried(&mut self, kind: EditKind) -> Option<Editing> {
        self.retry.take().filter(|e| e.kind == kind)
    }

    pub fn key(&mut self, key: Key) -> Option<Effect> {
        self.notice = None;
        if self.editing.is_some() {
            // Back keeps what was typed, as OK does.
            return matches!(key, Key::Ok | Key::Back)
                .then(|| self.commit())
                .flatten();
        }
        match self.layer {
            Layer::List => self.list_key(key),
            Layer::Actions { feed, focus } => self.actions_key(feed, focus, key),
            Layer::Confirm { feed, remove } => match key {
                Key::Up | Key::Down => {
                    self.layer = Layer::Confirm {
                        feed,
                        remove: !remove,
                    };
                    None
                }
                Key::Ok => self.confirm(feed, remove),
                Key::Back | Key::Menu => self.confirm(feed, false),
                Key::Left | Key::Right => None,
            },
        }
    }

    fn list_key(&mut self, key: Key) -> Option<Effect> {
        let items = self.items();
        let at = items.iter().position(|i| *i == self.focus).unwrap_or(0);
        match (key, self.focus) {
            (Key::Up, _) => self.focus = items[at.saturating_sub(1)],
            (Key::Down, _) => self.focus = items[(at + 1).min(items.len() - 1)],
            (Key::Ok, Item::Add) => self.start_add(),
            (Key::Ok, Item::Update) => return Some(Effect::Update),
            (Key::Ok, Item::Inspect) => return Some(Effect::Inspect),
            (Key::Ok | Key::Menu, Item::Source(feed)) => {
                self.layer = Layer::Actions {
                    feed,
                    focus: Act::Switch,
                };
            }
            (Key::Back, _) => return Some(Effect::Close),
            _ => {}
        }
        None
    }

    fn actions_key(&mut self, feed: Feed, focus: Act, key: Key) -> Option<Effect> {
        let acts = self.acts(feed);
        let at = acts.iter().position(|a| *a == focus).unwrap_or(0);
        let to = match key {
            Key::Up => acts[at.saturating_sub(1)],
            Key::Down => acts[(at + 1).min(acts.len() - 1)],
            Key::Left if focus == Act::Shift => return self.shift(feed, -1),
            Key::Right if focus == Act::Shift => return self.shift(feed, 1),
            Key::Left | Key::Right => return None,
            Key::Ok => return self.act(feed, focus),
            Key::Back | Key::Menu => {
                self.layer = Layer::List;
                self.focus = Item::Source(feed);
                return None;
            }
        };
        self.layer = Layer::Actions { feed, focus: to };
        None
    }

    /// The time shift of `feed` one step later (`1`) or earlier (`-1`).
    pub fn shift(&mut self, feed: Feed, dir: i32) -> Option<Effect> {
        let now = self.line(feed)?.source.shift_minutes;
        let to = (now + dir * SHIFT_STEP).clamp(-MAX_SHIFT_MINUTES, MAX_SHIFT_MINUTES);
        self.layer = Layer::Actions {
            feed,
            focus: Act::Shift,
        };
        (to != now).then_some(Effect::Edit(Edit::Shift(feed, to)))
    }

    /// OK (or a tap) on `act` of source `feed`.
    pub fn act(&mut self, feed: Feed, act: Act) -> Option<Effect> {
        let line = self.line(feed)?.clone();
        self.notice = None;
        self.layer = Layer::Actions { feed, focus: act };
        match act {
            Act::Switch => Some(Effect::Edit(Edit::SetEnabled(feed, !line.source.enabled))),
            Act::Up => Some(Effect::Edit(Edit::Move(feed, -1))),
            Act::Down => Some(Effect::Edit(Edit::Move(feed, 1))),
            // OK steps it later; Left/Right go either way.
            Act::Shift => self.shift(feed, 1),
            Act::Rename => {
                self.editing = Some(Editing {
                    kind: EditKind::Rename(feed),
                    text: line.source.name.unwrap_or_default(),
                });
                None
            }
            Act::Relink => {
                // A link with an account in it is never put in the field.
                let shown = Some(line.source.url.expose().to_owned())
                    .filter(|l| super::playlists::plain_link(l));
                let fresh = Editing {
                    kind: EditKind::Relink(feed),
                    text: shown.unwrap_or_default(),
                };
                self.editing = Some(self.retried(EditKind::Relink(feed)).unwrap_or(fresh));
                None
            }
            Act::Remove => {
                self.layer = Layer::Confirm {
                    feed,
                    remove: false,
                };
                None
            }
        }
    }

    /// The confirmation's answer: remove, or back to the actions.
    pub fn confirm(&mut self, feed: Feed, remove: bool) -> Option<Effect> {
        if remove {
            self.layer = Layer::List;
            return Some(Effect::Edit(Edit::Remove(feed)));
        }
        self.layer = Layer::Actions {
            feed,
            focus: Act::Remove,
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

    /// A tap beside an open sheet: as Back.
    pub fn dismiss(&mut self) -> Option<Effect> {
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

    /// A link typed for `except` (or a new source): valid and not one the
    /// playlist has already.
    fn check_link(&mut self, text: &str, except: Option<Feed>) -> Option<SecretUrl> {
        let problem = match validate_field(Field::EpgUrl, text) {
            Ok(()) if text.is_empty() => Some(Notice::Link(FieldError::Required)),
            Ok(()) => None,
            Err(e) => Some(Notice::Link(e)),
        };
        let duplicate = self
            .lines
            .iter()
            .any(|l| Some(l.source.feed) != except && l.source.url.expose() == text);
        self.notice = problem.or(duplicate.then_some(Notice::Duplicate));
        self.notice.is_none().then(|| SecretUrl::new(text))
    }

    /// Ends text entry and decides what the text changes.
    pub fn commit(&mut self) -> Option<Effect> {
        let editing = self.editing.take()?;
        let text = editing.text.trim().to_owned();
        match editing.kind.clone() {
            EditKind::AddLink => {
                if self.check_link(&text, None).is_none() {
                    // The notice says what is wrong; the text comes back at
                    // the next try.
                    self.retry = Some(editing);
                    return None;
                }
                self.editing = Some(Editing {
                    kind: EditKind::AddName { url: text },
                    text: String::new(),
                });
                None
            }
            EditKind::AddName { url } => {
                let name = (!text.is_empty()).then_some(text);
                Some(Effect::Edit(Edit::Add {
                    url: SecretUrl::new(url),
                    name,
                }))
            }
            EditKind::Rename(feed) => {
                let name = (!text.is_empty()).then_some(text);
                (self.line(feed)?.source.name != name)
                    .then_some(Effect::Edit(Edit::Rename(feed, name)))
            }
            // Left empty, the link stays.
            EditKind::Relink(_) if text.is_empty() => None,
            EditKind::Relink(feed) => {
                if self.line(feed)?.source.url.expose() == text {
                    return None;
                }
                let Some(url) = self.check_link(&text, Some(feed)) else {
                    self.retry = Some(editing);
                    return None;
                };
                Some(Effect::Edit(Edit::Relink(feed, url)))
            }
        }
    }
}

#[cfg(test)]
#[path = "epg_sources_tests.rs"]
mod tests;

impl std::fmt::Debug for EditKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddLink => f.write_str("AddLink"),
            Self::AddName { .. } => f
                .debug_struct("AddName")
                .field("url", &"[redacted]")
                .finish(),
            Self::Rename(feed) => f.debug_tuple("Rename").field(feed).finish(),
            Self::Relink(feed) => f.debug_tuple("Relink").field(feed).finish(),
        }
    }
}
impl std::fmt::Debug for Editing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editing")
            .field("kind", &self.kind)
            .field("text", &"[redacted]")
            .finish()
    }
}

#[cfg(test)]
mod editing_diagnostic_regression {
    use super::*;
    #[test]
    fn leaf_and_retained_retry_edits_redact_raw_input() {
        let kind = EditKind::AddName {
            url: "synthetic-private-marker".into(),
        };
        assert!(!format!("{kind:?}").contains("synthetic-private-marker"));
        let editing = Editing {
            kind,
            text: "synthetic-private-marker".into(),
        };
        assert!(!format!("{editing:?}").contains("synthetic-private-marker"));
        let mut screen = Screen::new(SourceId(1), Vec::new());
        screen.retry = Some(editing.clone());
        screen.editing = Some(editing);
        assert!(!format!("{screen:?}").contains("synthetic-private-marker"));
    }
}
