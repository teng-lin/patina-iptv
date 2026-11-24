//! The favorites editor for one list: its channels with a star each.
//! OK toggles the focused channel and Right sets the whole list.
//! Bulk removal requires an explicit action and confirmation. Channels are
//! catalog indices (the list's rows when it opened); the shell keeps favorites by channel key.

use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FavoritesEditor {
    /// The groups column entry the list belongs to ([`iptv_core::lists`]).
    pub group: usize,
    /// The list's catalog channels, in order, as it was when opened.
    pub channels: Vec<usize>,
    /// Focused row.
    pub focus: usize,
    /// Toolbar button, or None while a channel row has focus.
    pub control: Option<usize>,
    /// Pending bulk removal; false focuses Cancel, true focuses Remove all.
    pub clear_confirmation: Option<bool>,
}

impl FavoritesEditor {
    pub fn new(group: usize, channels: Vec<usize>) -> Self {
        FavoritesEditor {
            group,
            channels,
            focus: 0,
            control: None,
            clear_confirmation: None,
        }
    }

    pub fn len(&self) -> usize {
        self.channels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// The catalog channel on `row`.
    pub fn channel(&self, row: usize) -> Option<usize> {
        self.channels.get(row).copied()
    }

    /// The focused channel, if the list has any.
    pub fn focused(&self) -> Option<usize> {
        self.channel(self.focus)
    }

    pub fn move_focus(&mut self, step: isize) {
        let last = self.len().saturating_sub(1) as isize;
        self.focus = (self.focus as isize + step).clamp(0, last) as usize;
    }

    /// OK: star or unstar the focused channel.
    pub fn toggle(&self, favorites: &mut BTreeSet<usize>) {
        if let Some(ch) = self.focused()
            && !favorites.remove(&ch)
        {
            favorites.insert(ch);
        }
    }

    pub fn request_clear(&mut self) {
        self.clear_confirmation = Some(false);
    }

    /// Resolve a pending request. Only explicit confirmation removes favorites.
    pub fn resolve_clear(&mut self, favorites: &mut BTreeSet<usize>, confirmed: bool) {
        if self.clear_confirmation.take().is_none() || !confirmed {
            return;
        }
        for ch in &self.channels {
            favorites.remove(ch);
        }
    }

    /// Right: every channel of this list becomes a favorite.
    pub fn set_group(&self, favorites: &mut BTreeSet<usize>) {
        favorites.extend(self.channels.iter().copied());
    }

    /// How many of the list's channels are favorites.
    pub fn starred(&self, favorites: &BTreeSet<usize>) -> usize {
        self.channels
            .iter()
            .filter(|c| favorites.contains(c))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_toggles_the_focused_channel() {
        let mut e = FavoritesEditor::new(1, (10..15).collect());
        let mut fav = BTreeSet::new();
        e.move_focus(2);
        e.toggle(&mut fav);
        assert_eq!(fav, BTreeSet::from([12]));
        e.toggle(&mut fav);
        assert!(fav.is_empty());
    }

    #[test]
    fn confirmed_removal_and_add_all_affect_only_this_group() {
        let mut e = FavoritesEditor::new(1, (10..15).collect());
        let mut fav = BTreeSet::from([3, 11, 40]);
        e.set_group(&mut fav);
        assert_eq!(e.starred(&fav), 5);
        assert!(
            fav.contains(&3) && fav.contains(&40),
            "other groups untouched"
        );
        e.request_clear();
        e.resolve_clear(&mut fav, true);
        assert_eq!(fav, BTreeSet::from([3, 40]));
        e.request_clear();
        e.resolve_clear(&mut fav, true);
        assert_eq!(
            fav,
            BTreeSet::from([3, 40]),
            "clearing an empty group is harmless"
        );
    }

    #[test]
    fn cancelling_or_missing_confirmation_preserves_favorites() {
        let mut e = FavoritesEditor::new(1, vec![10, 11]);
        let mut fav = BTreeSet::from([3, 10, 11]);
        e.resolve_clear(&mut fav, true);
        e.request_clear();
        assert_eq!(e.clear_confirmation, Some(false), "Cancel is focused first");
        e.resolve_clear(&mut fav, false);
        assert_eq!(fav, BTreeSet::from([3, 10, 11]));
        assert_eq!(e.clear_confirmation, None);
    }

    #[test]
    fn a_list_need_not_be_contiguous() {
        // Favorites or recent channels: any catalog channels, in list order.
        let mut e = FavoritesEditor::new(0, vec![42, 3, 17]);
        let mut fav = BTreeSet::from([3]);
        e.move_focus(2);
        e.toggle(&mut fav);
        assert_eq!(fav, BTreeSet::from([3, 17]));
        assert_eq!(e.starred(&fav), 2);
        e.request_clear();
        e.resolve_clear(&mut fav, true);
        assert!(fav.is_empty());
    }

    #[test]
    fn focus_clamps_and_empty_groups_do_nothing() {
        let mut e = FavoritesEditor::new(0, vec![0, 1, 2]);
        e.move_focus(-4);
        assert_eq!(e.focused(), Some(0));
        e.move_focus(10);
        assert_eq!(e.focused(), Some(2));
        let empty = FavoritesEditor::new(0, Vec::new());
        let mut fav = BTreeSet::new();
        empty.toggle(&mut fav);
        empty.set_group(&mut fav);
        assert!(fav.is_empty());
        assert_eq!(empty.focused(), None);
    }
}
