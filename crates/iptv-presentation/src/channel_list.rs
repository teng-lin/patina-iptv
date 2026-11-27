//! The channel list over full-screen video (the player's Left; Long Left
//! shows it beside the video shrunk to a preview), as the reference's
//! player defaults have it: the list the guide shows (a group, All
//! channels, Favorites, Recently watched: [`crate::lists::ChannelList`])
//! with the same numbers and what is on now. Up/Down move, OK plays (OK
//! on what plays closes the list), Back closes.
//!
//! Left shows the guide's groups column beside it, with the same entries
//! ([`crate::lists::Entry`]): moving over an entry previews its list
//! ([`Outcome::Preview`]; the shell changes the guide's list as its groups
//! column does), OK or Right goes into it, Back returns to the channels.
//! The focus is a row of the current list and an entry index; the list
//! itself is the caller's.

use crate::lists::{ChannelList as Rows, VisibleEntries};

/// How the list sits over the video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListMode {
    /// Over the full-screen video.
    Overlay,
    /// Beside the video, which shrinks to a preview.
    Preview,
}

/// Which column has the focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListPane {
    Channels,
    Groups,
}

/// What a key did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Only the focus moved (or nothing happened).
    Stay,
    /// Play this catalog channel; the list stays.
    Play(usize),
    /// Show the list of this groups column entry: the shell selects it
    /// (as the guide's groups column does) and calls [`ChannelList::shown`].
    Preview(usize),
    /// Close the list.
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelList {
    pub mode: ListMode,
    pub pane: ListPane,
    /// The focused row of the current list.
    pub row: usize,
    /// The focused entry of the groups column.
    pub entry: usize,
}

impl ChannelList {
    /// Opens on the row of `playing` (catalog), else the list's first.
    pub fn open(mode: ListMode, playing: Option<usize>, list: &Rows) -> ChannelList {
        ChannelList {
            mode,
            pane: ListPane::Channels,
            row: playing.and_then(|c| list.row_of(c)).unwrap_or(0),
            entry: list.entry(),
        }
    }

    /// Up/Down (and pages): moves within the focused column, clamped. In
    /// the groups a new entry is previewed. `entries` is the groups
    /// column's visible entries.
    pub fn move_focus(&mut self, step: isize, list: &Rows, entries: VisibleEntries) -> Outcome {
        match self.pane {
            ListPane::Channels => {
                if !list.is_empty() {
                    let last = list.len() as isize - 1;
                    self.row = (self.row as isize + step).clamp(0, last) as usize;
                }
                Outcome::Stay
            }
            ListPane::Groups => {
                let position = entries.position(self.entry);
                let next = match position {
                    Some(position) => crate::guide::page_row(position, entries.len(), step),
                    None if step < 0 => entries.len().saturating_sub(1),
                    None => 0,
                };
                entries
                    .entry(next)
                    .map_or(Outcome::Stay, |entry| self.preview(entry, list))
            }
        }
    }

    /// The list of `entry` is shown now (after a [`Outcome::Preview`]):
    /// the focus starts at its first row.
    pub fn shown(&mut self, list: &Rows) {
        self.entry = list.entry();
        self.row = 0;
    }

    /// Left: from the channels to the groups, on the current list's entry.
    pub fn left(&mut self, list: &Rows) {
        if self.pane == ListPane::Channels {
            self.pane = ListPane::Groups;
            self.entry = list.entry();
        }
    }

    /// Right: from the groups into the list shown (an empty one cannot be
    /// entered).
    pub fn right(&mut self, list: &Rows) {
        if self.pane == ListPane::Groups && !list.is_empty() {
            self.pane = ListPane::Channels;
        }
    }

    /// OK: a channel plays (on the one playing: the list closes); on an
    /// entry, its list (previewed already) is entered.
    pub fn select(&mut self, playing: Option<usize>, list: &Rows) -> Outcome {
        match self.pane {
            ListPane::Groups if self.entry != list.entry() => self.preview(self.entry, list),
            ListPane::Groups => {
                self.right(list);
                Outcome::Stay
            }
            ListPane::Channels => match list.channel(self.row) {
                Some(c) if playing == Some(c) => Outcome::Close,
                Some(c) => Outcome::Play(c),
                None => Outcome::Stay,
            },
        }
    }

    /// A tap on an entry of the groups column: previews it, or enters it
    /// when it is the list shown.
    pub fn tap_entry(&mut self, entry: usize, list: &Rows) -> Outcome {
        self.pane = ListPane::Groups;
        self.entry = entry;
        self.select(None, list)
    }

    /// A tap on a row: focuses it and picks it.
    pub fn tap_row(&mut self, row: usize, playing: Option<usize>, list: &Rows) -> Outcome {
        self.focus_row(row);
        self.select(playing, list)
    }

    /// Marks a channel targeted by a secondary gesture without picking it.
    pub fn focus_row(&mut self, row: usize) {
        self.pane = ListPane::Channels;
        self.row = row;
    }

    /// Back: from the groups to the channels, else closes.
    pub fn back(&mut self) -> Outcome {
        match self.pane {
            ListPane::Groups => {
                self.pane = ListPane::Channels;
                Outcome::Stay
            }
            ListPane::Channels => Outcome::Close,
        }
    }

    /// The playing channel changed (a zap while the list shows): follow it
    /// when the list has it.
    pub fn follow(&mut self, playing: usize, list: &Rows) {
        if let Some(row) = list.row_of(playing) {
            self.row = row;
        }
    }

    fn preview(&mut self, entry: usize, list: &Rows) -> Outcome {
        self.entry = entry;
        if entry == list.entry() {
            Outcome::Stay
        } else {
            Outcome::Preview(entry)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lists::tests::Catalog;
    use crate::lists::{ALL, FAVORITES, FIRST_GROUP, RECENT, entry_count};

    /// The player's list as the shell runs it: the guide's list, rebuilt
    /// on a preview.
    struct Player {
        catalog: Catalog,
        favorites: Vec<usize>,
        recent: Vec<usize>,
        list: Rows,
        state: ChannelList,
    }

    impl Player {
        fn on(entry: usize, playing: Option<usize>) -> Player {
            let catalog = Catalog::small();
            let (favorites, recent) = (vec![7, 3], vec![4, 0, 8]);
            let list = Rows::build(entry, &catalog, &favorites, &recent);
            let state = ChannelList::open(ListMode::Overlay, playing, &list);
            Player {
                catalog,
                favorites,
                recent,
                list,
                state,
            }
        }

        fn follow(&mut self, o: Outcome) -> Outcome {
            if let Outcome::Preview(entry) = o {
                self.list = Rows::build(entry, &self.catalog, &self.favorites, &self.recent);
                self.state.shown(&self.list);
            }
            o
        }

        fn step(&mut self, by: isize) -> Outcome {
            let o = self.state.move_focus(
                by,
                &self.list,
                VisibleEntries::all(entry_count(&self.catalog)),
            );
            self.follow(o)
        }

        fn ok(&mut self, playing: Option<usize>) -> Outcome {
            let o = self.state.select(playing, &self.list);
            self.follow(o)
        }
    }

    #[test]
    fn opens_on_the_current_list_at_the_playing_row() {
        // All channels in playlist order: 0, 3, 5, 1, 4, ...
        let p = Player::on(ALL, Some(1));
        assert_eq!((p.state.row, p.state.entry), (3, ALL));
        assert_eq!(Rows::number(p.state.row), 4, "numbered as in the guide");
        // A list without the playing channel opens on its first row.
        let p = Player::on(FAVORITES, Some(1));
        assert_eq!((p.state.row, p.state.pane), (0, ListPane::Channels));
        assert_eq!(p.list.channels(), [7, 3]);
    }

    #[test]
    fn moves_stay_inside_the_list_and_ok_plays_its_channels() {
        let mut p = Player::on(RECENT, Some(4));
        assert_eq!(p.state.row, 0);
        assert_eq!(p.ok(Some(4)), Outcome::Close, "OK on what plays closes");
        p.step(-1);
        assert_eq!(p.state.row, 0, "clamped at the first row");
        p.step(100);
        assert_eq!(p.state.row, 2, "and at the last");
        assert_eq!(p.ok(Some(4)), Outcome::Play(8), "a catalog channel");
        p.state.follow(0, &p.list);
        assert_eq!(p.state.row, 1, "a zap moves the focus to its row");
        p.state.follow(5, &p.list);
        assert_eq!(p.state.row, 1, "a channel the list lacks leaves it");
    }

    #[test]
    fn left_opens_the_guides_groups_and_moving_previews_their_lists() {
        let mut p = Player::on(ALL, Some(0));
        p.state.left(&p.list);
        assert_eq!((p.state.pane, p.state.entry), (ListPane::Groups, ALL));
        assert_eq!(p.step(-1), Outcome::Preview(RECENT));
        assert_eq!(
            p.list.channels(),
            [4, 0, 8],
            "Recently watched, newest first"
        );
        assert_eq!(p.step(-1), Outcome::Preview(FAVORITES));
        assert_eq!(p.step(-1), Outcome::Stay, "the first entry clamps");
        assert_eq!(p.step(4), Outcome::Preview(FIRST_GROUP + 1));
        assert_eq!(p.list.channels(), [3, 4], "Sports");
        assert_eq!(
            p.step(10),
            Outcome::Preview(FIRST_GROUP + 2),
            "the last clamps"
        );
        // OK goes into the previewed list at its first row.
        assert_eq!(p.ok(Some(0)), Outcome::Stay);
        assert_eq!((p.state.pane, p.state.row), (ListPane::Channels, 0));
        assert_eq!(p.ok(Some(0)), Outcome::Play(5));
    }

    #[test]
    fn an_empty_list_is_shown_but_not_entered() {
        let mut p = Player::on(ALL, None);
        p.favorites.clear();
        p.state.left(&p.list);
        p.step(-2);
        assert!(p.list.is_empty());
        assert_eq!(p.ok(None), Outcome::Stay);
        p.state.right(&p.list);
        assert_eq!(p.state.pane, ListPane::Groups, "nothing to enter");
        assert_eq!(p.ok(None), Outcome::Stay);
        // Back returns to the (empty) channels, then closes.
        assert_eq!(p.state.back(), Outcome::Stay);
        assert_eq!(p.state.pane, ListPane::Channels);
        assert_eq!(p.ok(None), Outcome::Stay, "no row to play");
        assert_eq!(p.state.back(), Outcome::Close);
    }

    #[test]
    fn taps_preview_then_enter_an_entry_and_play_a_row() {
        let mut p = Player::on(ALL, Some(0));
        let o = p.state.tap_entry(FIRST_GROUP, &p.list);
        assert_eq!(p.follow(o), Outcome::Preview(FIRST_GROUP));
        assert_eq!(p.list.channels(), [0, 1, 2]);
        assert_eq!(p.state.pane, ListPane::Groups);
        assert_eq!(p.state.tap_entry(FIRST_GROUP, &p.list), Outcome::Stay);
        assert_eq!(p.state.pane, ListPane::Channels);
        assert_eq!(p.state.tap_row(2, Some(0), &p.list), Outcome::Play(2));
        assert_eq!(p.state.tap_row(0, Some(0), &p.list), Outcome::Close);
    }
}
