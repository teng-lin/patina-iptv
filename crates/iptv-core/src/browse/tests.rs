use std::collections::BTreeSet;
use std::rc::Rc;

use super::*;
use crate::favorites::{Favorites, KeyIndex};
use crate::guide::{Airing, Focus, Move, Pane, home_window};
use crate::lists::tests::Catalog;
use crate::lists::{ALL, ChannelList, Entry, FAVORITES, FIRST_GROUP, ListView, entry_count};

/// The guide screen as the shell runs it: the focus, the list it shows and
/// the favourites, with the list rebuilt as the shell rebuilds it.
struct Screen {
    catalog: Rc<dyn Schedule>,
    index: KeyIndex,
    favorites: Favorites,
    rail: Vec<RailItem>,
    view: ListView,
    focus: Focus,
    /// `guide.back_returns_to_now`.
    back_to_now: bool,
}

impl Screen {
    fn new() -> Self {
        let catalog: Rc<dyn Schedule> = Rc::new(Catalog::small());
        let index = KeyIndex::build(&*catalog);
        let focus = Focus::at(0);
        let view = ListView::new(
            catalog.clone(),
            Rc::new(ChannelList::build(focus.group, &*catalog, &[], &[])),
        );
        Screen {
            catalog,
            index,
            favorites: Favorites::default(),
            rail: rail_items(VodAvailability::default()),
            view,
            focus,
            back_to_now: true,
        }
    }

    /// Rebuilds the list for the selected entry, keeping the focused
    /// channel by key when `keep` (the shell's restore).
    fn rebuild(&mut self, keep: bool) {
        let key = self.view.channel_key(self.focus.channel);
        let favorites = self.favorites.channels(&self.index);
        let list = ChannelList::build(self.focus.group, &*self.catalog, &favorites, &[]);
        self.view = ListView::new(self.catalog.clone(), Rc::new(list));
        if keep {
            let row = key.and_then(|k| self.view.find_channel(&k));
            self.focus.restore(row, &self.view);
        }
    }

    fn follow(&mut self, outcome: Outcome) -> Outcome {
        if outcome == Outcome::Preview {
            self.rebuild(false);
        }
        outcome
    }

    fn mv(&mut self, m: Move) -> Outcome {
        let cols = Columns {
            entries: entry_count(&*self.catalog),
            rail: &self.rail.clone(),
            back_to_now: true,
        };
        let o = self.focus.navigate(m, &cols, &self.view);
        self.follow(o)
    }

    fn ok(&mut self) -> Outcome {
        let rail = self.rail.clone();
        let cols = Columns {
            entries: entry_count(&*self.catalog),
            rail: &rail,
            back_to_now: true,
        };
        let o = self.focus.ok(&cols, &self.view);
        self.follow(o)
    }

    fn back(&mut self) -> Outcome {
        let rail = self.rail.clone();
        let cols = Columns {
            entries: entry_count(&*self.catalog),
            rail: &rail,
            back_to_now: self.back_to_now,
        };
        self.focus.back(&cols, &self.view)
    }

    /// The catalog channel the focus is on.
    fn channel(&self) -> Option<usize> {
        self.view.channel(self.focus.channel)
    }

    fn shown(&self) -> Vec<usize> {
        self.view.list().channels().to_vec()
    }

    fn toggle_favourite(&mut self, channel: usize) {
        let key = self.catalog.channel_key(channel).unwrap();
        let on = !self.favorites.contains(&key);
        self.favorites.set(&key, on);
        if self.focus.group == FAVORITES {
            self.rebuild(true);
        }
    }
}

#[test]
fn the_guide_opens_on_all_channels() {
    let s = Screen::new();
    assert_eq!((s.focus.pane, s.focus.group), (Pane::Guide, ALL));
    assert_eq!(s.shown(), [0, 3, 5, 1, 4, 6, 2, 7, 8]);
    assert!(!s.focus.columns_shown());
}

#[test]
fn left_from_the_live_programme_opens_the_groups_and_the_rail() {
    let mut s = Screen::new();
    // Row 4 is channel 4: even channels have no catch-up here.
    for _ in 0..4 {
        s.mv(Move::Down);
    }
    assert_eq!(s.channel(), Some(4));
    assert_eq!(s.mv(Move::Left), Outcome::Moved);
    assert_eq!(s.focus.pane, Pane::Groups);
    assert!(s.focus.columns_shown());
    assert_eq!(s.focus.group, ALL, "the current list is selected");
    assert_eq!(s.focus.channel, 4, "the guide keeps its row meanwhile");
    // Left again: the rail, on Search; OK searches.
    assert_eq!(s.mv(Move::Left), Outcome::Moved);
    assert_eq!(s.focus.pane, Pane::Rail);
    assert_eq!(s.rail[s.focus.rail], RailItem::Search);
    assert_eq!(s.ok(), Outcome::Open(RailItem::Search));
}

#[test]
fn left_on_a_catch_up_row_walks_its_archive_before_the_groups() {
    let mut s = Screen::new();
    s.mv(Move::Down);
    assert_eq!(s.channel(), Some(3), "a day of catch-up");
    let floor = s.focus.floor(&s.view);
    assert!((-24 * 60..0).contains(&floor), "{floor}");
    let mut steps = 0;
    while s.mv(Move::Left) == Outcome::Moved && s.focus.pane == Pane::Guide {
        steps += 1;
    }
    assert!(steps > 0);
    assert_eq!(s.focus.pane, Pane::Groups);
    let oldest = s.focus.program(&s.view);
    assert!(oldest.start >= floor, "from the oldest replay");
    let before = s.view.program_at(s.focus.channel, oldest.start - 1);
    assert!(before.start < floor);
}

#[test]
fn moving_over_the_groups_filters_the_guide_live() {
    let mut s = Screen::new();
    s.focus.open_groups();
    s.focus.now = 40;
    assert_eq!(s.mv(Move::Down), Outcome::Preview);
    assert_eq!(s.focus.group, FIRST_GROUP);
    assert_eq!(s.shown(), [0, 1, 2], "News only");
    assert_eq!((s.focus.channel, s.focus.cursor), (0, 40));
    assert_eq!(s.focus.window, home_window(40));
    assert_eq!(s.mv(Move::Down), Outcome::Preview);
    assert_eq!(s.shown(), [3, 4], "Sports only");
    s.mv(Move::Down);
    assert_eq!(s.mv(Move::Down), Outcome::None, "the last group clamps");
    assert_eq!(s.shown(), [5, 6, 7, 8]);
    assert_eq!(
        s.focus.pane,
        Pane::Groups,
        "previewing keeps the groups open"
    );
    for _ in 0..10 {
        s.mv(Move::Up);
    }
    assert_eq!(s.focus.group, FAVORITES, "Favourites is first");
    assert!(s.shown().is_empty());
}

#[test]
fn ok_chooses_the_group_and_focuses_its_first_channel() {
    let mut s = Screen::new();
    s.focus.open_groups();
    s.mv(Move::Down);
    s.mv(Move::Down); // Sports
    assert_eq!(s.ok(), Outcome::Committed);
    assert_eq!(s.focus.pane, Pane::Guide);
    assert!(!s.focus.columns_shown(), "the columns close");
    assert_eq!(s.channel(), Some(3), "the group's first channel");
    assert_eq!(s.view.channel_number(s.focus.channel), 1);
    assert!(s.focus.hint, "the hint card shows");
    assert_eq!(s.focus.program(&s.view).airing(s.focus.now), Airing::Live);
    // OK again watches the programme.
    assert_eq!(s.ok(), Outcome::Watch);
    assert!(!s.focus.hint, "any key dismisses the hint");
}

#[test]
fn right_from_the_groups_returns_to_the_same_row() {
    let mut s = Screen::new();
    for _ in 0..4 {
        s.mv(Move::Down);
    }
    s.mv(Move::Left);
    assert_eq!(s.mv(Move::Right), Outcome::Committed);
    assert_eq!((s.focus.pane, s.focus.channel), (Pane::Guide, 4));
    assert!(!s.focus.hint);
}

#[test]
fn back_walks_hint_guide_groups_rail_then_leaves() {
    let mut s = Screen::new();
    s.focus.open_groups();
    s.mv(Move::Down);
    s.ok();
    assert!(s.focus.hint);
    assert_eq!(s.back(), Outcome::Moved);
    assert_eq!((s.focus.pane, s.focus.hint), (Pane::Guide, false));
    assert_eq!(s.back(), Outcome::Moved);
    assert_eq!(s.focus.pane, Pane::Groups);
    assert_eq!(s.back(), Outcome::Moved);
    assert_eq!(s.focus.pane, Pane::Rail);
    assert_eq!(s.rail[s.focus.rail], RailItem::Tv, "the rail opens on TV");
    assert_eq!(s.back(), Outcome::Leave);
}

#[test]
fn back_on_a_future_programme_returns_to_now_first() {
    let mut s = Screen::new();
    s.mv(Move::Right);
    s.mv(Move::Right);
    assert_eq!(s.back(), Outcome::Moved);
    assert_eq!(s.focus.pane, Pane::Guide);
    assert_eq!(s.focus.cursor, s.focus.now);
    s.back();
    assert_eq!(s.focus.pane, Pane::Groups);
}

#[test]
fn without_back_to_now_back_on_a_past_or_future_programme_opens_the_groups() {
    let mut s = Screen::new();
    s.back_to_now = false;
    s.mv(Move::Right);
    s.mv(Move::Right);
    let cursor = s.focus.cursor;
    assert_ne!(cursor, s.focus.now);
    assert_eq!(s.back(), Outcome::Moved);
    assert_eq!(s.focus.pane, Pane::Groups, "straight to the groups");
    assert_eq!(s.focus.cursor, cursor, "the guide stays where it was");
    // The rest of the chain is the same.
    assert_eq!(s.back(), Outcome::Moved);
    assert_eq!(s.focus.pane, Pane::Rail);
    assert_eq!(s.back(), Outcome::Leave);
}

#[test]
fn the_player_list_selects_a_list_without_touching_the_columns() {
    let mut s = Screen::new();
    s.mv(Move::Down);
    s.focus.now = 40;
    let rail = s.rail.clone();
    let cols = Columns {
        entries: entry_count(&*s.catalog),
        rail: &rail,
        back_to_now: true,
    };
    assert!(s.focus.preview_list(FIRST_GROUP + 1, &cols));
    s.rebuild(false);
    assert_eq!(s.shown(), [3, 4], "Sports");
    assert_eq!((s.focus.pane, s.focus.channel), (Pane::Guide, 0));
    assert_eq!(s.focus.cursor, 40, "at now");
    assert!(
        !s.focus.preview_list(FIRST_GROUP + 1, &cols),
        "already shown"
    );
    assert!(!s.focus.preview_list(99, &cols), "no such entry");
}

#[test]
fn the_rail_skips_items_without_a_screen() {
    let mut s = Screen::new();
    s.focus.open_groups();
    assert_eq!(s.mv(Move::Left), Outcome::Moved);
    assert_eq!(s.focus.pane, Pane::Rail);
    assert_eq!(s.rail[s.focus.rail], RailItem::Search);
    assert_eq!(s.mv(Move::Down), Outcome::Moved);
    assert_eq!(s.rail[s.focus.rail], RailItem::Tv);
    assert_eq!(s.mv(Move::Down), Outcome::Moved);
    assert_eq!(
        s.rail[s.focus.rail],
        RailItem::Recordings,
        "Recordings has its own screen"
    );
    assert_eq!(s.ok(), Outcome::Open(RailItem::Recordings));
    s.mv(Move::Down);
    assert_eq!(s.ok(), Outcome::Open(RailItem::Playlists));
    s.mv(Move::Down);
    assert_eq!(s.rail[s.focus.rail], RailItem::Settings);
    assert_eq!(s.mv(Move::Down), Outcome::None, "Settings is last");
    s.mv(Move::Up);
    s.mv(Move::Up);
    s.mv(Move::Up);
    assert_eq!(s.rail[s.focus.rail], RailItem::Tv);
    s.mv(Move::Up);
    assert_eq!(s.ok(), Outcome::Open(RailItem::Search));
    assert_eq!(s.mv(Move::Left), Outcome::None);
    assert_eq!(s.mv(Move::Right), Outcome::Moved);
    assert_eq!(s.focus.pane, Pane::Groups);
    // OK on TV goes back to the groups.
    s.mv(Move::Left);
    s.mv(Move::Down);
    assert_eq!(s.ok(), Outcome::Moved);
    assert_eq!(s.focus.pane, Pane::Groups);
}

#[test]
fn rail_items_follow_the_playlist() {
    use RailItem::*;
    let none = VodAvailability::default();
    let both = VodAvailability {
        shown: true,
        any: true,
    };
    assert_eq!(
        rail_items(none),
        [Search, Tv, Recordings, MyList, Playlists, Settings]
    );
    assert_eq!(
        rail_items(both),
        [
            Search, Tv, Continue, Recent, Movies, Shows, Recordings, MyList, Playlists, Settings
        ]
    );
    // A live-only playlist on screen while another enabled playlist has
    // films: the cross-source shelves stay reachable, the per-playlist
    // Movies and Shows do not.
    assert_eq!(
        rail_items(VodAvailability {
            shown: false,
            any: true,
        }),
        [
            Search, Tv, Continue, Recent, Recordings, MyList, Playlists, Settings
        ]
    );
    assert!(Settings.at_bottom() && Playlists.at_bottom() && !Search.at_bottom());
    assert!(Continue.enabled() && Recent.enabled() && !Continue.at_bottom());
    let mut f = Focus::at(0);
    let items = rail_items(both);
    assert_eq!(
        rail_home(&items),
        1,
        "TV stays home: discovery entries do not replace the guide"
    );
    let cols = Columns {
        entries: 3,
        rail: &items,
        back_to_now: true,
    };
    assert_eq!(
        f.tap_rail(2, &cols, &SyntheticList),
        Outcome::Open(Continue),
        "Continue watching, one below TV"
    );
    assert_eq!(
        f.tap_rail(3, &cols, &SyntheticList),
        Outcome::Open(Recent),
        "Recently added"
    );
    assert_eq!(
        f.tap_rail(4, &cols, &SyntheticList),
        Outcome::Open(Movies),
        "Movies"
    );
    assert_eq!(f.tap_rail(0, &cols, &SyntheticList), Outcome::Open(Search));
    assert_eq!(f.pane, Pane::Rail);
}

/// An empty list for tests that do not need rows.
struct SyntheticList;

impl Schedule for SyntheticList {
    fn channel_count(&self) -> usize {
        0
    }
    fn group_count(&self) -> usize {
        1
    }
    fn group_start(&self, _: usize) -> usize {
        0
    }
    fn program_at(&self, _: usize, minute: i32) -> crate::guide::Program {
        crate::guide::Program {
            start: minute,
            end: minute + 1,
        }
    }
    fn programs_between(&self, _: usize, _: i32, _: i32) -> Vec<crate::guide::Program> {
        Vec::new()
    }
    fn catchup_days(&self, _: usize) -> u8 {
        0
    }
    fn channel_name(&self, _: usize) -> String {
        String::new()
    }
    fn group_name(&self, _: usize) -> String {
        String::new()
    }
    fn title(&self, _: usize, _: crate::guide::Program) -> String {
        String::new()
    }
}

#[test]
fn a_tap_on_a_group_selects_and_filters_it() {
    let mut s = Screen::new();
    let cols_entries = entry_count(&*s.catalog);
    let rail = s.rail.clone();
    let cols = Columns {
        entries: cols_entries,
        rail: &rail,
        back_to_now: true,
    };
    let o = s.focus.tap_entry(Entry::Group(2).index(), &cols);
    s.follow(o);
    assert_eq!(o, Outcome::Preview);
    assert_eq!((s.focus.pane, s.shown()), (Pane::Groups, vec![5, 6, 7, 8]));
    assert_eq!(
        s.focus.tap_entry(Entry::Group(2).index(), &cols),
        Outcome::Moved
    );
    assert_eq!(s.focus.tap_entry(99, &cols), Outcome::None);
    // A tap on a programme then leaves the columns (the grid's own tap).
    s.focus.tap(1, s.focus.now, &s.view);
    assert_eq!((s.focus.pane, s.channel()), (Pane::Guide, Some(6)));
}

#[test]
fn an_empty_favourites_list_cannot_be_entered() {
    let mut s = Screen::new();
    s.focus.open_groups();
    s.focus.group = FAVORITES + 1;
    s.mv(Move::Up);
    assert!(s.shown().is_empty());
    assert_eq!(s.ok(), Outcome::None);
    assert_eq!(s.mv(Move::Right), Outcome::None);
    assert_eq!(s.focus.pane, Pane::Groups);
}

#[test]
fn favourites_list_membership_follows_the_favourites() {
    let mut s = Screen::new();
    s.toggle_favourite(7);
    s.toggle_favourite(3);
    s.toggle_favourite(5);
    s.focus.open_groups();
    s.focus.group = FAVORITES + 1;
    s.mv(Move::Up);
    assert_eq!(s.shown(), [7, 3, 5], "in the order they were added");
    s.ok();
    s.mv(Move::Down); // on 3
    assert_eq!(s.channel(), Some(3));
    // Removing the channel above the focus keeps the focus on its channel.
    s.toggle_favourite(7);
    assert_eq!(s.shown(), [3, 5]);
    assert_eq!((s.channel(), s.focus.channel), (Some(3), 0));
    // Removing the focused channel moves to the nearest row.
    s.toggle_favourite(3);
    assert_eq!(s.channel(), Some(5));
    // Removing the last one hands the focus back to the groups.
    s.toggle_favourite(5);
    assert!(s.shown().is_empty());
    assert_eq!(s.focus.pane, Pane::Groups);
    let index = KeyIndex::build(&*s.catalog);
    assert_eq!(s.favorites.set_of(&index), BTreeSet::new());
}

#[test]
fn a_new_catalog_keeps_the_list_and_the_channel_by_identity() {
    let mut s = Screen::new();
    s.focus.open_groups();
    for _ in 0..2 {
        s.mv(Move::Down);
    }
    s.ok(); // Sports: channels 3, 4
    s.mv(Move::Down);
    assert_eq!(s.channel(), Some(4));
    let id = crate::lists::ListId::of(s.focus.group, &*s.catalog);
    let key = s.view.channel_key(s.focus.channel);
    // A refresh: News gone, Sports now first and holding one more channel.
    let mut new = Catalog::new(vec![("Sports", 3), ("Kids", 4)], (0..7).collect());
    new.keys = ["k3", "k9", "k4", "k5", "k6", "k7", "k8"]
        .map(str::to_owned)
        .to_vec();
    s.catalog = Rc::new(new);
    s.index = KeyIndex::build(&*s.catalog);
    s.focus.group = id.find(&*s.catalog);
    assert_eq!(s.focus.group, Entry::Group(0).index());
    s.rebuild(false);
    let row = key.and_then(|k| s.view.find_channel(&k));
    s.focus.restore(row, &s.view);
    assert_eq!(s.shown(), [0, 1, 2]);
    assert_eq!(s.focus.channel, 2, "found by key, not by row");
    assert_eq!(s.view.channel_key(s.focus.channel).as_deref(), Some("k4"));
}

#[test]
fn the_catalog_list_view_zaps_within_the_current_list() {
    let mut s = Screen::new();
    s.focus.open_groups();
    s.mv(Move::Down); // News: 0, 1, 2
    s.ok();
    let list = s.view.list().clone();
    let last = list.len() - 1;
    assert_eq!(list.zap(last, 1).and_then(|r| list.channel(r)), Some(0));
    assert_eq!(list.zap(0, -1).and_then(|r| list.channel(r)), Some(2));
}

#[test]
fn tab_toggles_between_the_grid_and_the_groups() {
    let mut s = Screen::new();
    s.mv(Move::Down);
    s.mv(Move::Right);
    let (row, cursor) = (s.focus.channel, s.focus.cursor);
    assert!(s.focus.toggle_groups(&s.view));
    assert_eq!(s.focus.pane, Pane::Groups, "also off the live programme");
    assert!(s.focus.toggle_groups(&s.view));
    assert_eq!(
        (s.focus.pane, s.focus.channel, s.focus.cursor),
        (Pane::Guide, row, cursor),
        "back where it was"
    );
    s.focus.pane = Pane::Rail;
    assert!(s.focus.toggle_groups(&s.view));
    assert_eq!(s.focus.pane, Pane::Guide);
    // An empty list keeps the columns.
    s.focus.pane = Pane::Groups;
    assert!(!s.focus.toggle_groups(&SyntheticList));
    assert_eq!(s.focus.pane, Pane::Groups);
}

#[test]
fn the_groups_back_button_opens_the_rail_as_back_does() {
    // An iPad's ‹ in the groups column: where Back from the groups goes.
    let mut s = Screen::new();
    s.focus.open_groups();
    let mut by_back = s.focus;
    let rail = s.rail.clone();
    let cols = Columns {
        entries: entry_count(&*s.catalog),
        rail: &rail,
        back_to_now: true,
    };
    by_back.back(&cols, &s.view);
    s.focus.hint = true;
    s.focus.open_rail(&cols);
    assert_eq!(s.focus.pane, Pane::Rail);
    assert_eq!(s.focus.rail, by_back.rail);
    assert!(!s.focus.hint, "the hint goes");
}
