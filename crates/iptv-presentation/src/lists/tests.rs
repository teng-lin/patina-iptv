use std::rc::Rc;

use super::*;
use iptv_core::guide::{Airing, SyntheticSchedule};

/// A small catalog: channels stored grouped (News 0..3, Sports 3..5, Kids
/// 5..9) while the playlist interleaves them.
pub(crate) struct Catalog {
    pub groups: Vec<(&'static str, usize)>,
    pub order: Vec<usize>,
    /// Channel keys by index (default `k<index>`).
    pub keys: Vec<String>,
}

impl Catalog {
    pub fn small() -> Self {
        Catalog::new(
            vec![("News", 3), ("Sports", 2), ("Kids", 4)],
            vec![0, 3, 5, 1, 4, 6, 2, 7, 8],
        )
    }

    pub fn new(groups: Vec<(&'static str, usize)>, order: Vec<usize>) -> Self {
        let n = groups.iter().map(|g| g.1).sum();
        Catalog {
            groups,
            order,
            keys: (0..n).map(|c| format!("k{c}")).collect(),
        }
    }
}

impl Schedule for Catalog {
    fn channel_count(&self) -> usize {
        self.groups.iter().map(|g| g.1).sum()
    }
    fn group_count(&self) -> usize {
        self.groups.len()
    }
    fn group_start(&self, group: usize) -> usize {
        self.groups[..group].iter().map(|g| g.1).sum()
    }
    fn program_at(&self, channel: usize, minute: i32) -> Program {
        SyntheticSchedule.program_at(channel, minute)
    }
    fn programs_between(&self, channel: usize, from: i32, minutes: i32) -> Vec<Program> {
        SyntheticSchedule.programs_between(channel, from, minutes)
    }
    fn catchup_days(&self, channel: usize) -> u8 {
        (channel % 2) as u8
    }
    fn channel_name(&self, channel: usize) -> String {
        format!("C{channel}")
    }
    fn channel_playlist(&self, channel: usize) -> Option<&str> {
        if channel >= self.channel_count() {
            None
        } else if channel < 5 {
            Some("Alpha")
        } else {
            Some("Beta")
        }
    }
    fn channel_number(&self, channel: usize) -> u32 {
        100 + channel as u32
    }
    fn channel_logo(&self, channel: usize) -> Option<String> {
        (channel != 4).then(|| format!("logo{channel}"))
    }
    fn group_label(&self, group: usize) -> GroupName {
        GroupName::Named(self.groups[group].0.to_owned())
    }
    fn listing(&self, channel: usize, _program: Program) -> Option<Listing> {
        Some(Listing::Title(format!("T{channel}")))
    }
    fn channel_key(&self, channel: usize) -> Option<String> {
        self.keys.get(channel).cloned()
    }
    fn playlist_order(&self) -> Vec<usize> {
        self.order.clone()
    }
}

fn view(catalog: &Rc<dyn Schedule>, list: ChannelList) -> ListView {
    ListView::new(catalog.clone(), Rc::new(list))
}

#[test]
fn the_groups_column_starts_with_favorites_recent_and_all() {
    let c = Catalog::small();
    assert_eq!(entry_count(&c), 6);
    let names: Vec<_> = (0..entry_count(&c)).map(|e| entry_name(e, &c)).collect();
    assert_eq!(
        names,
        [
            GroupName::Favorites,
            GroupName::Recent,
            GroupName::AllChannels,
            GroupName::Named("News".into()),
            GroupName::Named("Sports".into()),
            GroupName::Named("Kids".into()),
        ]
    );
    for e in 0..entry_count(&c) {
        assert_eq!(Entry::at(e).index(), e);
    }
    assert_eq!(Entry::at(ALL), Entry::All);
    assert_eq!(Entry::at(FIRST_GROUP + 1), Entry::Group(1));
}

#[test]
fn a_group_lists_only_its_channels_and_all_follows_the_playlist() {
    let c = Catalog::small();
    let sports = ChannelList::build(Entry::Group(1).index(), &c, &[], &[]);
    assert_eq!(sports.channels(), &[3, 4]);
    let kids = ChannelList::build(Entry::Group(2).index(), &c, &[], &[]);
    assert_eq!(
        kids.channels(),
        &[5, 6, 7, 8],
        "the last group runs to the end"
    );
    let all = ChannelList::build(ALL, &c, &[], &[]);
    assert_eq!(all.channels(), &[0, 3, 5, 1, 4, 6, 2, 7, 8]);
    let gone = ChannelList::build(Entry::Group(9).index(), &c, &[], &[]);
    assert!(gone.is_empty());
}

#[test]
fn favorites_and_recent_lists_keep_their_order() {
    let c = Catalog::small();
    let fav = ChannelList::build(FAVORITES, &c, &[7, 1, 99], &[]);
    assert_eq!(fav.channels(), &[7, 1], "out-of-range channels are dropped");
    let recent = ChannelList::build(RECENT, &c, &[], &[4, 0]);
    assert_eq!(recent.channels(), &[4, 0]);
    assert!(ChannelList::build(FAVORITES, &c, &[], &[]).is_empty());
}

#[test]
fn rows_are_numbered_by_their_place_in_the_list() {
    let catalog: Rc<dyn Schedule> = Rc::new(Catalog::small());
    let kids = view(&catalog, ChannelList::build(5, &*catalog, &[], &[]));
    let numbers: Vec<u32> = (0..kids.channel_count())
        .map(|r| kids.channel_number(r))
        .collect();
    assert_eq!(numbers, [1, 2, 3, 4], "each list starts at 1");
    let all = view(&catalog, ChannelList::build(ALL, &*catalog, &[], &[]));
    assert_eq!(all.channel_number(8), 9);
    assert_eq!(catalog.channel_number(8), 108, "the catalog keeps its own");
}

#[test]
fn a_channel_is_numbered_by_the_list_on_screen_else_by_the_catalog() {
    let c = Catalog::small();
    let kids = ChannelList::build(5, &c, &[], &[]);
    assert_eq!(kids.number_of(7, &c), 3, "its row + 1");
    assert_eq!(kids.number_of(0, &c), 100, "not in the list: the catalog's");
    let fav = ChannelList::build(FAVORITES, &c, &[8, 0], &[]);
    assert_eq!(fav.number_of(0, &c), 2);
}

#[test]
fn a_list_view_reads_the_catalog_channel_behind_each_row() {
    let catalog: Rc<dyn Schedule> = Rc::new(Catalog::small());
    let fav = view(
        &catalog,
        ChannelList::build(FAVORITES, &*catalog, &[7, 4], &[]),
    );
    assert_eq!(fav.channel_count(), 2);
    assert_eq!(fav.channel(0), Some(7));
    assert_eq!(fav.channel_name(0), "C7");
    assert_eq!(fav.channel_playlist(0), Some("Beta"));
    assert_eq!(fav.channel_playlist(1), Some("Alpha"));
    assert_eq!(
        fav.channel_group_label(0),
        Some(GroupName::Named("Kids".into()))
    );
    assert_eq!(
        fav.channel_group_label(1),
        Some(GroupName::Named("Sports".into()))
    );
    assert_eq!(fav.channel_key(1).as_deref(), Some("k4"));
    assert_eq!(fav.channel_logo(0).as_deref(), Some("logo7"));
    assert_eq!(fav.channel_logo(1), None, "a channel without a logo");
    assert_eq!(fav.catchup_days(0), 1);
    assert_eq!(fav.program_at(0, 0), catalog.program_at(7, 0));
    assert_eq!(
        fav.programs_between(1, 0, 90),
        catalog.programs_between(4, 0, 90)
    );
    assert_eq!(
        fav.listing(0, fav.program_at(0, 0)),
        Some(Listing::Title("T7".into()))
    );
    assert_eq!(fav.find_channel("k4"), Some(1));
    assert_eq!(fav.find_channel("k0"), None, "not in this list");
    assert_eq!(fav.group_label(0), GroupName::Favorites);
    // Rows the list does not have still answer (an empty list's cursor).
    assert_eq!(fav.channel_name(5), "");
    assert_eq!(fav.channel_playlist(5), None);
    assert_eq!(fav.channel_group_label(5), None);
    assert_eq!(fav.listing(5, fav.program_at(5, 75)), None);
    assert_eq!(fav.program_at(5, 75).airing(75), Airing::Live);
}

#[test]
fn lists_are_found_again_by_content_in_a_new_catalog() {
    let old = Catalog::small();
    let sports = ListId::of(Entry::Group(1).index(), &old);
    assert_eq!(sports, ListId::Group("Sports".into()));
    // A refresh reordered the groups and dropped News.
    let new = Catalog::new(vec![("Kids", 4), ("Sports", 2)], (0..6).collect());
    assert_eq!(sports.find(&new), Entry::Group(1).index());
    let news = ListId::of(Entry::Group(0).index(), &old);
    assert_eq!(
        news.find(&new),
        ALL,
        "a gone group falls back to all channels"
    );
    assert_eq!(ListId::of(FAVORITES, &old).find(&new), FAVORITES);
    assert_eq!(ListId::of(RECENT, &old).find(&new), RECENT);
}

#[test]
fn zapping_wraps_within_the_list() {
    let c = Catalog::small();
    let fav = ChannelList::build(FAVORITES, &c, &[7, 1, 4], &[]);
    assert_eq!(fav.zap(2, 1), Some(0));
    assert_eq!(fav.zap(0, -1), Some(2));
    assert_eq!(fav.zap(1, 1), Some(2));
    assert_eq!(ChannelList::default().zap(0, 1), None);
}

#[test]
fn a_resumed_favorite_is_shown_in_favorites() {
    let catalog = Catalog::small();
    let favorites = [5, 1];
    assert_eq!(resume_entry(1, &catalog, &favorites, true), Some(FAVORITES));
    let list = ChannelList::build(FAVORITES, &catalog, &favorites, &[]);
    assert_eq!(list.row_of(1), Some(1));
}

#[test]
fn a_resumed_channel_keeps_its_usual_list_otherwise() {
    let catalog = Catalog::small();
    // Not a favorite of this playlist (another playlist's favorites are
    // not among this catalog's channels).
    assert_eq!(resume_entry(3, &catalog, &[5, 1], true), None);
    assert_eq!(resume_entry(3, &catalog, &[], true), None);
    // The Favorites category is not offered.
    assert_eq!(resume_entry(1, &catalog, &[5, 1], false), None);
    // A channel the catalog does not have.
    assert_eq!(resume_entry(42, &catalog, &[42], true), None);
}

#[test]
fn a_resumed_hidden_channel_keeps_all_channels() {
    struct Hiding(Catalog);
    impl Schedule for Hiding {
        fn channel_count(&self) -> usize {
            self.0.channel_count()
        }
        fn group_count(&self) -> usize {
            self.0.group_count()
        }
        fn group_start(&self, group: usize) -> usize {
            self.0.group_start(group)
        }
        fn program_at(&self, channel: usize, minute: i32) -> Program {
            self.0.program_at(channel, minute)
        }
        fn programs_between(&self, channel: usize, from: i32, minutes: i32) -> Vec<Program> {
            self.0.programs_between(channel, from, minutes)
        }
        fn catchup_days(&self, channel: usize) -> u8 {
            self.0.catchup_days(channel)
        }
        fn channel_name(&self, channel: usize) -> String {
            self.0.channel_name(channel)
        }
        fn channel_number(&self, channel: usize) -> u32 {
            self.0.channel_number(channel)
        }
        fn channel_logo(&self, channel: usize) -> Option<String> {
            self.0.channel_logo(channel)
        }
        fn group_label(&self, group: usize) -> GroupName {
            self.0.group_label(group)
        }
        fn listing(&self, channel: usize, program: Program) -> Option<Listing> {
            self.0.listing(channel, program)
        }
        fn channel_hidden(&self, channel: usize) -> bool {
            channel == 1
        }
    }
    let catalog = Hiding(Catalog::small());
    // Hidden: no list has its row, favorite or not.
    assert_eq!(resume_entry(1, &catalog, &[1], true), Some(ALL));
    assert_eq!(resume_entry(1, &catalog, &[], true), Some(ALL));
    assert_eq!(ChannelList::build(ALL, &catalog, &[1], &[]).row_of(1), None);
    assert_eq!(resume_entry(5, &catalog, &[5], true), Some(FAVORITES));
}

#[test]
fn visible_positions_keep_the_original_category_indices() {
    for bits in 0..8 {
        let enabled = std::array::from_fn(|index| bits & (1 << index) != 0);
        let entries = VisibleEntries::new(FIRST_GROUP + 2, enabled);
        let expected: Vec<_> = (0..FIRST_GROUP + 2)
            .filter(|&entry| entry >= FIRST_GROUP || enabled[entry])
            .collect();
        assert_eq!(entries.len(), expected.len());
        for (position, &entry) in expected.iter().enumerate() {
            assert_eq!(entries.position(entry), Some(position));
            assert_eq!(entries.entry(position), Some(entry));
        }
        assert_eq!(entries.entry(entries.len()), None);
        assert_eq!(entries.retain(FIRST_GROUP + 1), FIRST_GROUP + 1);
        if !enabled[FAVORITES] {
            assert_eq!(entries.position(FAVORITES), None);
            assert_eq!(
                entries.retain(FAVORITES),
                if enabled[ALL] { ALL } else { FIRST_GROUP }
            );
        }
    }

    let empty = VisibleEntries::new(FIRST_GROUP, [false; FIRST_GROUP]);
    assert!(empty.is_empty());
    assert_eq!(empty.retain(ALL), ALL);
}
