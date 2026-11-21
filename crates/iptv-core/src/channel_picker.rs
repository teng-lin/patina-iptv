//! A channel picker with independent group selection and scoped search.
//! Channels and groups retain focus by content identity as filters or the
//! catalogue change. It never changes the guide's selected list.

use crate::guide::Schedule;
use crate::lists::{ChannelList, ListId, entry_count};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Search,
    Clear,
    Close,
    Groups,
    Channels,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    pub index: usize,
    pub key: String,
    pub name: String,
    /// Number in the unfiltered group, so typing does not renumber results.
    pub number: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelPicker {
    pub pane: Pane,
    pub keyboard: bool,
    query: String,
    group: ListId,
    channel: Option<String>,
    channels: Rc<Vec<Channel>>,
}

impl ChannelPicker {
    pub fn open(group: ListId, channel: Option<String>) -> Self {
        Self {
            pane: Pane::Channels,
            keyboard: false,
            query: String::new(),
            group,
            channel,
            channels: Rc::default(),
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }

    pub fn entry(&self, catalog: &dyn Schedule) -> usize {
        self.group.find(catalog)
    }

    pub fn row(&self) -> Option<usize> {
        self.channels
            .iter()
            .position(|c| Some(&c.key) == self.channel.as_ref())
    }

    pub fn selected(&self) -> Option<usize> {
        self.channels.get(self.row()?).map(|c| c.index)
    }

    pub fn refresh(&mut self, catalog: &dyn Schedule, favorites: &[usize], recent: &[usize]) {
        let entry = self.entry(catalog);
        self.group = ListId::of(entry, catalog);
        let rows = ChannelList::build(entry, catalog, favorites, recent);
        let query = self.query.trim().to_lowercase();
        self.channels = Rc::new(
            rows.channels()
                .iter()
                .enumerate()
                .filter_map(|(row, &index)| {
                    let name = catalog.channel_name(index);
                    let key = catalog.channel_key(index)?;
                    let number = ChannelList::number(row);
                    (query.is_empty()
                        || name.to_lowercase().contains(&query)
                        || number.to_string() == query)
                        .then_some(Channel {
                            index,
                            key,
                            name,
                            number,
                        })
                })
                .collect(),
        );
        if self.row().is_none()
            && let Some(first) = self.channels.first()
        {
            self.channel = Some(first.key.clone());
        }
    }

    /// The caller refreshes after changing the query or group.
    pub fn type_query(&mut self, query: String) {
        self.query = query;
    }

    pub fn group(&mut self, entry: usize, catalog: &dyn Schedule) {
        self.group = ListId::of(entry, catalog);
        self.pane = Pane::Groups;
        self.keyboard = false;
    }

    pub fn focus_row(&mut self, row: usize) {
        if let Some(channel) = self.channels.get(row) {
            self.channel = Some(channel.key.clone());
            self.pane = Pane::Channels;
            self.keyboard = false;
        }
    }

    pub fn edit(&mut self) {
        self.pane = Pane::Search;
        self.keyboard = true;
    }

    pub fn keyboard_hidden(&mut self) {
        self.keyboard = false;
    }

    pub fn results(&mut self) {
        self.keyboard = false;
        self.pane = if self.channels.is_empty() {
            Pane::Groups
        } else {
            Pane::Channels
        };
    }

    pub fn vertical(&mut self, by: isize, catalog: &dyn Schedule) {
        match self.pane {
            Pane::Groups => {
                let entry = (self.entry(catalog) as isize + by)
                    .clamp(0, entry_count(catalog).saturating_sub(1) as isize)
                    as usize;
                if by < 0 && entry == self.entry(catalog) {
                    self.pane = Pane::Search;
                } else {
                    self.group(entry, catalog);
                }
            }
            Pane::Channels => {
                let row = self.row().unwrap_or(0);
                if by < 0 && row == 0 {
                    self.pane = Pane::Search;
                } else if !self.channels.is_empty() {
                    let to =
                        (row as isize + by).clamp(0, self.channels.len() as isize - 1) as usize;
                    self.focus_row(to);
                }
            }
            _ if by > 0 => self.results(),
            _ => {}
        }
    }

    pub fn horizontal(&mut self, right: bool) {
        self.keyboard = false;
        self.pane = match (self.pane, right) {
            (Pane::Channels, false) => Pane::Groups,
            (Pane::Groups, true) if !self.channels.is_empty() => Pane::Channels,
            (Pane::Search, true) => Pane::Clear,
            (Pane::Clear, true) => Pane::Close,
            (Pane::Close, false) => Pane::Clear,
            (Pane::Clear, false) => Pane::Search,
            (pane, _) => pane,
        };
    }

    /// Back first finishes editing, then clears a filter, then closes.
    /// `true` asks the shell to close the picker.
    pub fn back(&mut self) -> bool {
        if self.keyboard {
            self.results();
            false
        } else if !self.query.is_empty() {
            self.query.clear();
            false
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lists::{ALL, FAVORITES, FIRST_GROUP, RECENT, tests::Catalog};

    fn picker(catalog: &Catalog) -> ChannelPicker {
        let mut picker = ChannelPicker::open(ListId::All, Some("k4".into()));
        picker.refresh(catalog, &[7, 1], &[4, 0]);
        picker
    }

    #[test]
    fn filtering_keeps_channel_identity_and_unfiltered_numbers() {
        let catalog = Catalog::small();
        let mut picker = picker(&catalog);
        assert_eq!(picker.row(), Some(4));
        picker.type_query(" c4 ".into());
        picker.refresh(&catalog, &[], &[]);
        assert_eq!(picker.selected(), Some(4));
        assert_eq!(picker.row(), Some(0));
        assert_eq!(picker.channels()[0].number, 5);
        picker.type_query("".into());
        picker.refresh(&catalog, &[], &[]);
        assert_eq!(picker.row(), Some(4));
        picker.type_query("5".into());
        picker.refresh(&catalog, &[], &[]);
        assert!(
            picker
                .channels()
                .iter()
                .any(|c| c.index == 4 && c.number == 5)
        );
    }

    #[test]
    fn search_is_scoped_to_the_group_and_survives_group_changes() {
        let catalog = Catalog::small();
        let mut picker = picker(&catalog);
        picker.type_query("c4".into());
        picker.group(FIRST_GROUP, &catalog);
        picker.refresh(&catalog, &[], &[]);
        assert!(picker.channels().is_empty());
        assert_eq!(picker.selected(), None);
        picker.horizontal(true);
        assert_eq!(picker.pane, Pane::Groups, "empty results cannot be entered");
        picker.group(FIRST_GROUP + 1, &catalog);
        picker.refresh(&catalog, &[], &[]);
        assert_eq!(picker.query(), "c4");
        assert_eq!(picker.selected(), Some(4));
        assert_eq!(picker.channels()[0].number, 2);
        picker.type_query("".into());
        picker.group(FAVORITES, &catalog);
        picker.refresh(&catalog, &[7, 1], &[4, 0]);
        assert_eq!(
            picker
                .channels()
                .iter()
                .map(|c| c.index)
                .collect::<Vec<_>>(),
            [7, 1]
        );
        picker.group(RECENT, &catalog);
        picker.refresh(&catalog, &[7, 1], &[4, 0]);
        assert_eq!(picker.selected(), Some(4));
    }

    #[test]
    fn catalog_reordering_restores_groups_and_channels_by_key() {
        let catalog = Catalog::small();
        let mut picker = picker(&catalog);
        picker.group(FIRST_GROUP + 1, &catalog);
        picker.refresh(&catalog, &[], &[]);
        let mut changed = Catalog::new(
            vec![("Sports", 2), ("News", 3), ("Kids", 4)],
            (0..9).collect(),
        );
        changed.keys = ["k3", "k4", "k0", "k1", "k2", "k5", "k6", "k7", "k8"]
            .map(String::from)
            .to_vec();
        picker.refresh(&changed, &[], &[]);
        assert_eq!(picker.entry(&changed), FIRST_GROUP);
        assert_eq!(
            picker.selected(),
            Some(1),
            "k4 moved from catalogue index 4 to 1"
        );
        assert_eq!(picker.row(), Some(1));
        let gone = Catalog::new(vec![("News", 3)], vec![0, 1, 2]);
        picker.refresh(&gone, &[], &[]);
        assert_eq!(picker.entry(&gone), ALL);
        assert!(picker.selected().is_some());
    }

    #[test]
    fn remote_reaches_search_groups_results_clear_and_close() {
        let catalog = Catalog::small();
        let mut picker = picker(&catalog);
        picker.horizontal(false);
        assert_eq!(picker.pane, Pane::Groups);
        picker.vertical(1, &catalog);
        picker.refresh(&catalog, &[], &[]);
        assert_eq!(picker.entry(&catalog), FIRST_GROUP);
        picker.horizontal(true);
        assert_eq!(picker.pane, Pane::Channels);
        picker.vertical(-1, &catalog);
        assert_eq!(picker.pane, Pane::Search);
        picker.horizontal(true);
        assert_eq!(picker.pane, Pane::Clear);
        picker.horizontal(true);
        assert_eq!(picker.pane, Pane::Close);
        picker.horizontal(false);
        picker.horizontal(false);
        picker.edit();
        picker.type_query("missing".into());
        picker.refresh(&catalog, &[], &[]);
        assert!(!picker.back(), "finish editing first");
        assert!(!picker.keyboard);
        assert!(!picker.back(), "then clear the search");
        picker.refresh(&catalog, &[], &[]);
        assert_eq!(picker.query(), "");
        assert!(!picker.channels().is_empty());
        assert!(picker.back(), "finally close");
    }
}
