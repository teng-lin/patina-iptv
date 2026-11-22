//! A tile's menu and the connection warning: what they offer and what
//! choosing an item does.
//!
//! The menu's items, in order: Try again (when its stream failed), Add
//! screen and Search and add (while another tile fits), Change channel,
//! Pause or Play, Listen to this (two or more tiles, on one not heard),
//! Enlarge screen or Reduce screen (two or more tiles), Full screen,
//! Remove screen (two or more tiles).

use super::{Multiview, Outcome, Pick};

/// A tile's menu items, in the order they are offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileAction {
    /// Only when its stream failed.
    Retry,
    /// Only while another tile fits: from the channel list.
    Add,
    /// Only while another tile fits: from the search screen.
    SearchAdd,
    Replace,
    /// While the tile plays.
    Pause,
    /// While the tile is paused.
    Play,
    /// With two or more tiles, when another one is heard: the sound moves
    /// to this one.
    Listen,
    /// With two or more tiles, when this one is not the enlarged one.
    Enlarge,
    /// When this tile is the enlarged one.
    Reduce,
    FullScreen,
    /// With two or more tiles.
    Remove,
}

/// The connection warning's choices, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarningChoice {
    AddAnyway,
    Cancel,
}

pub const WARNING_CHOICES: [WarningChoice; 2] = [WarningChoice::AddAnyway, WarningChoice::Cancel];

/// A dialog over the tiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dialog {
    /// A tile's menu; `focus` indexes `items`.
    Options {
        tile: usize,
        items: Vec<TileAction>,
        focus: usize,
    },
    /// Adding one more stream exceeds what the account allows (`allowed`
    /// at once); `focus` indexes [`WARNING_CHOICES`]. `search`: the add
    /// came from Search and add, so insisting opens the search screen.
    Warning {
        allowed: u32,
        focus: usize,
        search: bool,
    },
}

impl<C: Clone + PartialEq> Multiview<C> {
    /// Opens tile `tile`'s menu, its first item focused.
    pub(super) fn open_options(&mut self, tile: usize, failed: &impl Fn(usize) -> bool) {
        let Some(t) = self.tiles.get(tile) else {
            return;
        };
        let several = self.tiles.len() > 1;
        let mut items = Vec::with_capacity(8);
        if failed(t.surface) {
            items.push(TileAction::Retry);
        }
        if self.can_add() {
            items.extend([TileAction::Add, TileAction::SearchAdd]);
        }
        items.push(TileAction::Replace);
        items.push(if t.paused {
            TileAction::Play
        } else {
            TileAction::Pause
        });
        if several && self.heard_index() != tile {
            items.push(TileAction::Listen);
        }
        if several {
            items.push(if self.enlarged == Some(tile) {
                TileAction::Reduce
            } else {
                TileAction::Enlarge
            });
        }
        items.push(TileAction::FullScreen);
        if several {
            items.push(TileAction::Remove);
        }
        self.dialog = Some(Dialog::Options {
            tile,
            items,
            focus: 0,
        });
    }

    /// Item `index` of the open dialog (OK on it, or a tap).
    pub fn choose(&mut self, index: usize) -> Outcome<C> {
        match self.dialog.take() {
            None => Outcome::Stay,
            Some(Dialog::Warning { .. }) if self.pending_pick.is_some() => {
                let (pick, channel) = self.pending_pick.take().unwrap();
                if WARNING_CHOICES.get(index) == Some(&WarningChoice::AddAnyway) {
                    self.pick = Some(pick);
                    self.picked(channel)
                } else {
                    Outcome::Stay
                }
            }
            Some(Dialog::Warning { search, .. }) => match WARNING_CHOICES.get(index) {
                Some(WarningChoice::AddAnyway) => {
                    self.insisted = true;
                    self.pick = Some(Pick::Add);
                    Self::picker(search)
                }
                _ => Outcome::Stay,
            },
            Some(Dialog::Options { tile, items, .. }) => {
                let Some(action) = items.get(index) else {
                    return Outcome::Stay;
                };
                match action {
                    TileAction::Retry => {
                        self.tiles[tile].paused = false;
                        Outcome::Retry(self.tiles[tile].surface)
                    }
                    TileAction::Add => self.add(),
                    TileAction::SearchAdd => self.search_add(),
                    TileAction::Replace => {
                        self.pick = Some(Pick::Replace(tile));
                        Outcome::PickChannel
                    }
                    TileAction::Pause | TileAction::Play => {
                        self.set_paused(tile, *action == TileAction::Pause);
                        Outcome::Stay
                    }
                    TileAction::Listen => {
                        self.listen(tile);
                        Outcome::Stay
                    }
                    TileAction::Enlarge | TileAction::Reduce => {
                        self.toggle_enlarge(tile);
                        Outcome::Stay
                    }
                    TileAction::FullScreen => self.full_screen(tile),
                    TileAction::Remove => {
                        self.remove(tile);
                        Outcome::Stay
                    }
                }
            }
        }
    }

    /// Adds a channel picked from the channel list (the menu's Add screen,
    /// the pointer's toolbar, opening): asks first past the account's
    /// limit. Nothing while no other tile fits.
    pub fn add(&mut self) -> Outcome<C> {
        self.start_add(false)
    }

    /// Adds a channel found on the search screen (the menu's Search and
    /// add), as [`Multiview::add`] does.
    pub fn search_add(&mut self) -> Outcome<C> {
        self.start_add(true)
    }

    fn start_add(&mut self, search: bool) -> Outcome<C> {
        self.dialog = None;
        if !self.can_add() {
            return Outcome::Stay;
        }
        if let Some(allowed) = self.over_connections().filter(|_| !self.insisted) {
            self.dialog = Some(Dialog::Warning {
                allowed,
                focus: 1,
                search,
            });
            return Outcome::Stay;
        }
        self.pick = Some(Pick::Add);
        Self::picker(search)
    }

    fn picker(search: bool) -> Outcome<C> {
        if search {
            Outcome::SearchChannel
        } else {
            Outcome::PickChannel
        }
    }
}
