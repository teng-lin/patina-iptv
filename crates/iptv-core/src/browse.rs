//! The guide screen's columns, left to right: the navigation rail, the
//! groups column and the guide grid ([`Focus`] owns the remote's place in
//! all three).
//!
//! - Left from the guide's floor programme (the oldest replay on a
//!   catch-up channel, now's programme on one without:
//!   [`crate::guide::floor`]) shows the groups column and, beside it, the
//!   rail (collapsed to icons); the guide shifts right. Hold Left (Tab on a
//!   keyboard) shows them from anywhere in the grid ([`Focus::open_groups`]).
//! - Moving over the groups previews each one: the guide shows that list
//!   at once, from its first row ([`Outcome::Preview`]). OK chooses it: the
//!   columns close and the list's first channel has the focus, with a hint
//!   card. Right goes to the guide without moving the row. A tap selects a
//!   group as moving onto it does.
//! - Left from the groups focuses the rail (it expands with labels) on
//!   Search: Left, Left, OK searches. Items without a screen in this
//!   build are dimmed and skipped.
//! - Back: hint card → guide (a past or future programme returns to now
//!   first, unless [`Columns::back_to_now`] is off) → groups → rail →
//!   leave the app.
//!
//! An empty list (no favourites yet) cannot be entered: the focus stays in
//! the groups column while the guide explains how to add some.

use crate::guide::{Focus, Move, Pane, Schedule};

/// An item of the navigation rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RailItem {
    Search,
    /// The guide (this screen).
    Tv,
    Movies,
    Shows,
    Recordings,
    MyList,
    /// iptv-rs's own entry: the playlists screen.
    Playlists,
    Settings,
}

impl RailItem {
    /// Whether it opens something in this build; the rest are dimmed and
    /// the focus skips them.
    pub fn enabled(self) -> bool {
        matches!(
            self,
            RailItem::Search
                | RailItem::Tv
                | RailItem::Movies
                | RailItem::Shows
                | RailItem::Recordings
                | RailItem::Playlists
                | RailItem::Settings
        )
    }

    /// Kept at the bottom of the rail.
    pub fn at_bottom(self) -> bool {
        matches!(self, RailItem::Playlists | RailItem::Settings)
    }
}

/// The rail's items, top to bottom: Movies and Shows only for a playlist
/// with video on demand.
pub fn rail_items(has_vod: bool) -> Vec<RailItem> {
    let mut items = vec![RailItem::Search, RailItem::Tv];
    if has_vod {
        items.extend([RailItem::Movies, RailItem::Shows]);
    }
    items.extend([
        RailItem::Recordings,
        RailItem::MyList,
        RailItem::Playlists,
        RailItem::Settings,
    ]);
    items
}

/// The rail's current item (TV): where Back from the groups focuses it.
pub fn rail_home(items: &[RailItem]) -> usize {
    items.iter().position(|&i| i == RailItem::Tv).unwrap_or(0)
}

/// The rail's Search: where Left from the groups focuses it.
pub fn rail_search(items: &[RailItem]) -> usize {
    items
        .iter()
        .position(|&i| i == RailItem::Search)
        .unwrap_or_else(|| rail_home(items))
}

/// What the columns hold.
#[derive(Clone, Copy, Debug)]
pub struct Columns<'a> {
    /// Entries in the groups column ([`crate::lists::entry_count`]).
    pub entries: usize,
    pub rail: &'a [RailItem],
    /// Back on a past or future programme returns to now before it goes
    /// to the groups (`guide.back_returns_to_now`).
    pub back_to_now: bool,
}

/// What a key did, for the shell to follow up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing changed.
    None,
    /// Only the focus moved (or a column opened or closed).
    Moved,
    /// The selected group changed: show its list (the focus is on its
    /// first row, at now).
    Preview,
    /// A list was chosen: the columns closed, the guide has the focus.
    Committed,
    /// OK on a programme: watch it.
    Watch,
    /// Return the existing preview to full screen without tuning a channel.
    ReturnToPlayer,
    /// OK on a rail item that opens a screen.
    Open(RailItem),
    /// Back with nothing left to close: the platform leaves the app.
    Leave,
}

impl Focus {
    /// Whether the rail and the groups column are shown.
    pub fn columns_shown(&self) -> bool {
        matches!(self.pane, Pane::Rail | Pane::Groups)
    }

    /// TV hosts may hand Up at the first guide row to their mini-player.
    /// The row and time anchor remain intact for Down/Back and list refresh.
    pub fn focus_preview(&mut self, playing: bool) -> bool {
        if playing && self.pane == Pane::Guide && self.channel == 0 {
            self.pane = Pane::Preview;
            self.hint = false;
            true
        } else {
            false
        }
    }

    /// A stopped player or a layout without a preview returns focus to
    /// the anchored guide row; the content identity/time stay unchanged.
    pub fn retain_preview(&mut self, available: bool) {
        if self.pane == Pane::Preview && !available {
            self.pane = Pane::Guide;
        }
    }

    /// A direction on the guide screen. `list` is the list the guide shows.
    pub fn navigate(&mut self, m: Move, cols: &Columns, list: &dyn Schedule) -> Outcome {
        self.hint = false;
        match (self.pane, m) {
            (Pane::Preview, Move::Down | Move::Right) => {
                self.pane = if list.channel_count() > 0 {
                    Pane::Guide
                } else {
                    Pane::Groups
                };
                Outcome::Moved
            }
            (Pane::Preview, Move::Left) => {
                self.pane = Pane::Groups;
                Outcome::Moved
            }
            (Pane::Preview, Move::Up) => Outcome::None,
            (Pane::Guide, _) => {
                self.apply(m, list);
                Outcome::Moved
            }
            (Pane::Groups, Move::Up | Move::Down) => {
                let step = if m == Move::Up { -1 } else { 1 };
                let last = cols.entries.saturating_sub(1) as isize;
                let to = (self.group as isize + step).clamp(0, last.max(0)) as usize;
                self.select(to)
            }
            (Pane::Groups, Move::Left) => {
                self.pane = Pane::Rail;
                self.rail = rail_search(cols.rail);
                Outcome::Moved
            }
            (Pane::Groups, Move::Right) if list.channel_count() > 0 => {
                self.pane = Pane::Guide;
                Outcome::Committed
            }
            (Pane::Groups, Move::Right) => Outcome::None,
            (Pane::Rail, Move::Up | Move::Down) => {
                let step = if m == Move::Up { -1 } else { 1 };
                let to = rail_step(cols.rail, self.rail, step);
                if to == self.rail {
                    return Outcome::None;
                }
                self.rail = to;
                Outcome::Moved
            }
            (Pane::Rail, Move::Right) => {
                self.pane = Pane::Groups;
                Outcome::Moved
            }
            (Pane::Rail, Move::Left) => Outcome::None,
        }
    }

    /// OK on the guide screen.
    pub fn ok(&mut self, cols: &Columns, list: &dyn Schedule) -> Outcome {
        self.hint = false;
        match self.pane {
            Pane::Preview => Outcome::ReturnToPlayer,
            Pane::Guide => Outcome::Watch,
            Pane::Groups if list.channel_count() == 0 => Outcome::None,
            Pane::Groups => {
                self.pane = Pane::Guide;
                self.go_to_top();
                self.hint = true;
                Outcome::Committed
            }
            Pane::Rail => match cols.rail.get(self.rail) {
                Some(RailItem::Tv) => {
                    self.pane = Pane::Groups;
                    Outcome::Moved
                }
                Some(&item) if item.enabled() => Outcome::Open(item),
                _ => Outcome::None,
            },
        }
    }

    /// Back on the guide screen: one step along the chain.
    pub fn back(&mut self, cols: &Columns, list: &dyn Schedule) -> Outcome {
        if self.hint {
            self.hint = false;
            return Outcome::Moved;
        }
        match self.pane {
            Pane::Preview => {
                self.pane = if list.channel_count() > 0 {
                    Pane::Guide
                } else {
                    Pane::Groups
                };
                Outcome::Moved
            }
            Pane::Guide => {
                if !(cols.back_to_now && self.return_to_now(list)) {
                    self.pane = Pane::Groups;
                }
                Outcome::Moved
            }
            Pane::Groups => {
                self.focus_rail(cols);
                Outcome::Moved
            }
            Pane::Rail => Outcome::Leave,
        }
    }

    /// Shows the groups column (and the rail beside it) with the focus on
    /// the selected group: Left from the floor programme does it in
    /// [`Focus::navigate`]; this is Hold Left, the Groups key or a tap on
    /// the list's name.
    pub fn open_groups(&mut self) {
        self.hint = false;
        self.pane = Pane::Groups;
    }

    /// A touch's way to the rail (the groups column's ‹ on an iPad): the
    /// rail open on its home item, as Back from the groups.
    pub fn open_rail(&mut self, cols: &Columns) {
        self.hint = false;
        self.focus_rail(cols);
    }

    /// A keyboard's Tab: the groups column from the grid, the grid again
    /// from the columns (when the list has rows). Whether it changed.
    pub fn toggle_groups(&mut self, list: &dyn Schedule) -> bool {
        self.hint = false;
        match self.pane {
            Pane::Guide => self.pane = Pane::Groups,
            _ if list.channel_count() == 0 => return false,
            _ => self.pane = Pane::Guide,
        }
        true
    }

    /// A tap on a groups column entry: selects it as moving onto it does.
    pub fn tap_entry(&mut self, entry: usize, cols: &Columns) -> Outcome {
        if entry >= cols.entries {
            return Outcome::None;
        }
        self.hint = false;
        self.pane = Pane::Groups;
        match self.select(entry) {
            Outcome::None => Outcome::Moved,
            o => o,
        }
    }

    /// Selects groups column `entry` from elsewhere (the player's channel
    /// list): the guide shows its list from the first row, at now, with
    /// the columns as they were. Whether the selection changed.
    pub fn preview_list(&mut self, entry: usize, cols: &Columns) -> bool {
        entry < cols.entries && self.select(entry) == Outcome::Preview
    }

    /// A tap on a rail item.
    pub fn tap_rail(&mut self, index: usize, cols: &Columns, list: &dyn Schedule) -> Outcome {
        match cols.rail.get(index) {
            Some(item) if item.enabled() => {
                self.pane = Pane::Rail;
                self.rail = index;
                self.ok(cols, list)
            }
            _ => Outcome::None,
        }
    }

    /// After the list changed under the focus (favourites edited, a new
    /// catalog): back on `row` (where the focused channel is now), else on
    /// the nearest row. An emptied list hands the focus to the groups.
    pub fn restore(&mut self, row: Option<usize>, list: &dyn Schedule) {
        if let Some(row) = row {
            self.channel = row;
        }
        self.clamp_to(list);
        if list.channel_count() == 0 && self.pane == Pane::Guide {
            self.pane = Pane::Groups;
        }
    }

    fn select(&mut self, entry: usize) -> Outcome {
        if entry == self.group {
            return Outcome::None;
        }
        self.group = entry;
        self.go_to_top();
        Outcome::Preview
    }

    fn focus_rail(&mut self, cols: &Columns) {
        self.pane = Pane::Rail;
        self.rail = rail_home(cols.rail);
    }
}

/// The next enabled rail item from `from` in direction `step`, or `from`.
fn rail_step(items: &[RailItem], from: usize, step: isize) -> usize {
    let mut i = from as isize + step;
    while i >= 0 && (i as usize) < items.len() {
        if items[i as usize].enabled() {
            return i as usize;
        }
        i += step;
    }
    from
}

#[cfg(test)]
mod tests;
