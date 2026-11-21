//! The user's channel lists that outlive a catalog: favourites (in the
//! order they were added) and recently watched channels (newest first).
//! Both hold channel keys (content identity, stable across refreshes);
//! [`KeyIndex`] turns them into the catalog channels on screen. The shell
//! persists them per source.

use std::collections::{BTreeSet, HashMap};

use crate::guide::Schedule;

/// How many recently watched channels are kept.
pub const RECENT_LIMIT: usize = 30;

/// Catalog channels by key, built once per catalog (a schedule's own
/// lookup may be linear).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyIndex {
    rows: HashMap<String, usize>,
}

impl KeyIndex {
    pub fn build(catalog: &dyn Schedule) -> Self {
        let rows = (0..catalog.channel_count())
            .filter_map(|c| Some((catalog.channel_key(c)?, c)))
            .collect();
        KeyIndex { rows }
    }

    pub fn get(&self, key: &str) -> Option<usize> {
        self.rows.get(key).copied()
    }

    /// The channels of `keys` present in the catalog, in `keys`' order.
    pub fn channels(&self, keys: &[String]) -> Vec<usize> {
        keys.iter().filter_map(|k| self.get(k)).collect()
    }
}

/// Favourite channels by key, in the order they were added.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Favorites {
    keys: Vec<String>,
}

impl Favorites {
    /// From stored keys (duplicates dropped, first kept).
    pub fn from_keys(keys: impl IntoIterator<Item = String>) -> Self {
        let mut f = Favorites::default();
        for k in keys {
            f.set(&k, true);
        }
        f
    }

    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    pub fn contains(&self, key: &str) -> bool {
        self.keys.iter().any(|k| k == key)
    }

    /// Adds (at the end) or removes `key`; whether anything changed.
    pub fn set(&mut self, key: &str, on: bool) -> bool {
        match (self.contains(key), on) {
            (false, true) => {
                self.keys.push(key.to_owned());
                true
            }
            (true, false) => {
                self.keys.retain(|k| k != key);
                true
            }
            _ => false,
        }
    }

    /// Move an existing key without changing membership.
    pub fn move_key(&mut self, key: &str, index: usize) -> bool {
        let Some(at) = self.keys.iter().position(|k| k == key) else {
            return false;
        };
        let key = self.keys.remove(at);
        self.keys.insert(index.min(self.keys.len()), key);
        true
    }

    /// Moves a favourite to its displayed position, retaining absent keys.
    pub fn move_key_among(
        &mut self,
        key: &str,
        index: usize,
        present: impl Fn(&str) -> bool,
    ) -> bool {
        crate::reorder::move_to_present(&mut self.keys, key, index, |item| present(item))
    }

    /// Replaces the order of these members. Retains any other keys in
    /// place, including channels a provider has temporarily removed.
    pub fn apply_order(&mut self, order: &[String]) {
        use std::collections::HashSet;

        let present: HashSet<_> = self.keys.iter().collect();
        let mut reordered = order.iter().filter(|key| present.contains(key));
        let included: HashSet<_> = order.iter().collect();
        self.keys = self
            .keys
            .iter()
            .map(|key| {
                if included.contains(key) {
                    reordered.next().unwrap_or(key).clone()
                } else {
                    key.clone()
                }
            })
            .collect();
    }

    /// The favourites as catalog channels: the Favourites list, in order.
    pub fn channels(&self, index: &KeyIndex) -> Vec<usize> {
        index.channels(&self.keys)
    }

    /// The favourite catalog channels as a set (stars, search ranking).
    pub fn set_of(&self, index: &KeyIndex) -> BTreeSet<usize> {
        self.channels(index).into_iter().collect()
    }
}

/// Recently watched channels by key, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recent {
    keys: Vec<String>,
}

impl Recent {
    /// From stored keys, newest first.
    pub fn from_keys(keys: impl IntoIterator<Item = String>) -> Self {
        let mut r = Recent::default();
        let mut keys: Vec<String> = keys.into_iter().collect();
        keys.reverse();
        for k in keys {
            r.played(&k);
        }
        r
    }

    /// `key` started playing: first in the list, once.
    pub fn played(&mut self, key: &str) {
        self.keys.retain(|k| k != key);
        self.keys.insert(0, key.to_owned());
        self.keys.truncate(RECENT_LIMIT);
    }

    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    pub fn channels(&self, index: &KeyIndex) -> Vec<usize> {
        index.channels(&self.keys)
    }

    /// The player's History: the same channels as the Recently watched
    /// list, and the row of `playing` (catalog) among them.
    pub fn history(&self, index: &KeyIndex, playing: Option<usize>) -> History {
        let channels = self.channels(index);
        let playing = playing.and_then(|p| channels.iter().position(|&c| c == p));
        History { channels, playing }
    }
}

/// The recently watched channels as the player's History lists them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    /// Catalog channels, newest first.
    pub channels: Vec<usize>,
    /// The row of the playing channel, marked (and focused when it opens).
    pub playing: Option<usize>,
}

/// What turned `before` into `after`: (added, removed) channels.
pub fn diff(before: &BTreeSet<usize>, after: &BTreeSet<usize>) -> (Vec<usize>, Vec<usize>) {
    (
        after.difference(before).copied().collect(),
        before.difference(after).copied().collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::SyntheticSchedule;

    fn key(c: usize) -> String {
        format!("synthetic:{c}")
    }

    #[test]
    fn favorites_keep_their_order_and_survive_a_reordered_catalog() {
        let mut f = Favorites::default();
        assert!(f.set(&key(40), true));
        assert!(f.set(&key(7), true));
        assert!(!f.set(&key(7), true), "already a favorite");
        let index = KeyIndex::build(&SyntheticSchedule);
        assert_eq!(f.channels(&index), vec![40, 7], "the order they were added");
        assert_eq!(f.set_of(&index), BTreeSet::from([7, 40]));
        // A catalog where the keys sit at other rows (a refresh).
        let moved = KeyIndex {
            rows: HashMap::from([(key(7), 0), (key(40), 1)]),
        };
        assert_eq!(f.channels(&moved), vec![1, 0]);
        // A key the catalog no longer has stays saved but is not listed.
        f.set("gone", true);
        assert_eq!(f.channels(&moved), vec![1, 0]);
        assert!(f.contains("gone"));
        assert!(f.set(&key(40), false));
        assert_eq!(f.keys(), &[key(7), "gone".to_owned()]);
    }

    #[test]
    fn stored_favorites_drop_duplicates() {
        let f = Favorites::from_keys([key(1), key(2), key(1)]);
        assert_eq!(f.keys(), &[key(1), key(2)]);
    }

    #[test]
    fn recent_is_newest_first_once_and_bounded() {
        let mut r = Recent::from_keys([key(3), key(2), key(1)]);
        assert_eq!(r.keys(), &[key(3), key(2), key(1)]);
        r.played(&key(1));
        assert_eq!(r.keys(), &[key(1), key(3), key(2)]);
        for c in 100..100 + RECENT_LIMIT {
            r.played(&key(c));
        }
        assert_eq!(r.keys().len(), RECENT_LIMIT);
        assert_eq!(r.keys()[0], key(100 + RECENT_LIMIT - 1));
        let index = KeyIndex::build(&SyntheticSchedule);
        assert_eq!(r.channels(&index)[0], 100 + RECENT_LIMIT - 1);
    }

    #[test]
    fn history_is_the_recent_list_newest_first_with_the_playing_one_marked() {
        // As stored (newest first), then two channels played this session.
        let mut r = Recent::from_keys([key(9), key(4), "gone".to_owned(), key(2)]);
        r.played(&key(2));
        r.played(&key(30));
        let index = KeyIndex::build(&SyntheticSchedule);
        let h = r.history(&index, Some(30));
        assert_eq!(
            h.channels,
            [30, 2, 9, 4],
            "newest first, once, known keys only"
        );
        assert_eq!(h.playing, Some(0));
        // Replaying an older one (catch-up does not record) keeps the order.
        assert_eq!(r.history(&index, Some(9)).playing, Some(2));
        assert_eq!(r.history(&index, Some(500)).playing, None);
        assert_eq!(r.history(&index, None).channels, r.channels(&index));
    }

    #[test]
    fn diff_names_added_and_removed() {
        let before = BTreeSet::from([1, 2, 3]);
        let after = BTreeSet::from([2, 3, 4, 5]);
        assert_eq!(diff(&before, &after), (vec![4, 5], vec![1]));
    }
}
