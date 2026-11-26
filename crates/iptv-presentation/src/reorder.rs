//! Reorder mode for a list of channel or group ids.
//!
//! Two sub-modes, as observed in the reference UX's bulk operations, channel
//! and group editing, and manual sorting (acceptance criterion AC06):
//!
//! | Key | Browse (nothing selected) | Move (an item selected) |
//! | --- | --- | --- |
//! | OK | select the focused item | deselect it |
//! | Up/Down | move focus | move the item one place |
//! | Left/Right | focus the first/last item | move the item to the top/bottom |
//! | Back | leave | leave |
//!
//! Selection is not focus: the selected item is always the focused one,
//! but a focused item need not be selected. Focus is held by identity, so
//! it follows a moved item rather than staying on an index.
//!
//! Changes stay local until [`Reorder::commit`]. [`Reorder::cancel`] puts
//! the original order back. Leaving with unsaved changes asks first
//! ([`Outcome::ConfirmLeave`], answered with [`Reorder::resolve_leave`]).
//! The hint panel is a pure function of the sub-mode ([`Reorder::hints`]),
//! so it stays on screen for as long as the mode does; the shell supplies
//! the wording and the operation scope (which group, which list).
//!
//! Items must be unique: identity is `==`.

/// A remote key, as reorder mode sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubMode {
    /// Nothing selected: directions move focus.
    Browse,
    /// The focused item is selected: directions move it.
    Move,
}

/// What a key did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    /// Nothing changed (an edge, an empty list, a dialog open, mode over).
    Ignored,
    FocusMoved,
    Selected,
    Deselected,
    /// The selected item changed place.
    Moved,
    /// Leaving would lose changes: show the confirmation and answer with
    /// [`Reorder::resolve_leave`]. Keys are ignored until then.
    ConfirmLeave,
    /// The mode is over.
    Done(Exit<T>),
}

/// How the mode ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exit<T> {
    /// Persist this order.
    Saved(Vec<T>),
    /// Nothing to persist: the order equals the original.
    Unchanged,
    /// Changes were thrown away; the original order stands.
    Discarded,
}

/// The answer to [`Outcome::ConfirmLeave`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaveChoice {
    Save,
    Discard,
    /// Back to editing, nothing lost.
    Stay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintKey {
    Ok,
    UpDown,
    LeftRight,
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintAction {
    Select,
    Deselect,
    MoveFocus,
    MoveItem,
    FocusEnds,
    MoveItemToEnds,
    Leave,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    pub key: HintKey,
    pub action: HintAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Editing,
    Confirming,
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reorder<T> {
    original: Vec<T>,
    items: Vec<T>,
    focus: Option<T>,
    selected: bool,
    dirty: bool,
    phase: Phase,
}

impl<T: Clone + Eq> Reorder<T> {
    /// Opens on `focus` when it is in the list, else on the first item,
    /// with nothing selected.
    pub fn new(items: Vec<T>, focus: Option<&T>) -> Self {
        let focus = focus
            .filter(|f| items.contains(f))
            .or_else(|| items.first())
            .cloned();
        Reorder {
            original: items.clone(),
            items,
            focus,
            selected: false,
            dirty: false,
            phase: Phase::Editing,
        }
    }

    /// Opens with the focused item already selected (the reference player
    /// enters the mode this way).
    pub fn selecting_focused(mut self) -> Self {
        self.selected = self.focus.is_some();
        self
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn original(&self) -> &[T] {
        &self.original
    }

    pub fn focused(&self) -> Option<&T> {
        self.focus.as_ref()
    }

    /// The focused item's current row.
    pub fn focus_index(&self) -> Option<usize> {
        let focus = self.focus.as_ref()?;
        self.items.iter().position(|x| x == focus)
    }

    pub fn selected(&self) -> Option<&T> {
        self.focus.as_ref().filter(|_| self.selected)
    }

    pub fn sub_mode(&self) -> SubMode {
        if self.selected {
            SubMode::Move
        } else {
            SubMode::Browse
        }
    }

    /// The order differs from the original.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn is_confirming(&self) -> bool {
        self.phase == Phase::Confirming
    }

    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// The hint panel for the current sub-mode, in display order.
    pub fn hints(&self) -> [Hint; 4] {
        let (ok, up_down, left_right) = match self.sub_mode() {
            SubMode::Browse => (
                HintAction::Select,
                HintAction::MoveFocus,
                HintAction::FocusEnds,
            ),
            SubMode::Move => (
                HintAction::Deselect,
                HintAction::MoveItem,
                HintAction::MoveItemToEnds,
            ),
        };
        [
            Hint {
                key: HintKey::Ok,
                action: ok,
            },
            Hint {
                key: HintKey::UpDown,
                action: up_down,
            },
            Hint {
                key: HintKey::LeftRight,
                action: left_right,
            },
            Hint {
                key: HintKey::Back,
                action: HintAction::Leave,
            },
        ]
    }

    pub fn key(&mut self, key: Key) -> Outcome<T> {
        if self.phase != Phase::Editing {
            return Outcome::Ignored;
        }
        if key == Key::Back {
            return self.leave();
        }
        let Some(at) = self.focus_index() else {
            return Outcome::Ignored;
        };
        let last = self.items.len() - 1;
        match (key, self.selected) {
            (Key::Ok, _) => self.toggle_selection(),
            (Key::Up, false) => self.focus_row(at.saturating_sub(1)),
            (Key::Down, false) => self.focus_row((at + 1).min(last)),
            (Key::Left, false) => self.focus_row(0),
            (Key::Right, false) => self.focus_row(last),
            (Key::Up, true) => self.move_selected_to(at.saturating_sub(1)),
            (Key::Down, true) => self.move_selected_to((at + 1).min(last)),
            (Key::Left, true) => self.move_selected_to(0),
            (Key::Right, true) => self.move_selected_to(last),
            (Key::Back, _) => unreachable!("handled above"),
        }
    }

    /// Touch: focus `item`. Ignored while an item is selected: the
    /// selection pins focus until it is dropped.
    pub fn focus_on(&mut self, item: &T) -> Outcome<T> {
        if self.phase != Phase::Editing || self.selected || self.focus.as_ref() == Some(item) {
            return Outcome::Ignored;
        }
        match self.items.iter().position(|x| x == item) {
            Some(row) => self.focus_row(row),
            None => Outcome::Ignored,
        }
    }

    /// OK on the focused item.
    pub fn toggle_selection(&mut self) -> Outcome<T> {
        if self.phase != Phase::Editing || self.focus.is_none() {
            return Outcome::Ignored;
        }
        self.selected = !self.selected;
        if self.selected {
            Outcome::Selected
        } else {
            Outcome::Deselected
        }
    }

    /// Moves the selected item to `row` (clamped); touch drag-and-drop uses
    /// this directly. Focus stays on the item.
    pub fn move_selected_to(&mut self, row: usize) -> Outcome<T> {
        if self.phase != Phase::Editing || !self.selected {
            return Outcome::Ignored;
        }
        let Some(from) = self.focus_index() else {
            return Outcome::Ignored;
        };
        let to = row.min(self.items.len() - 1);
        if to == from {
            return Outcome::Ignored;
        }
        let item = self.items.remove(from);
        self.items.insert(to, item);
        self.dirty = self.items != self.original;
        Outcome::Moved
    }

    fn focus_row(&mut self, row: usize) -> Outcome<T> {
        let item = &self.items[row];
        if self.focus.as_ref() == Some(item) {
            return Outcome::Ignored;
        }
        self.focus = Some(item.clone());
        Outcome::FocusMoved
    }

    /// Back: leave now when there is nothing to lose, else ask.
    pub fn leave(&mut self) -> Outcome<T> {
        self.leave_with_changes(false)
    }

    /// Also confirm when the caller has pending changes outside the row order.
    pub fn leave_with_changes(&mut self, has_other_changes: bool) -> Outcome<T> {
        match self.phase {
            Phase::Editing if self.dirty || has_other_changes => {
                self.phase = Phase::Confirming;
                Outcome::ConfirmLeave
            }
            Phase::Editing => self.finish(Exit::Unchanged),
            _ => Outcome::Ignored,
        }
    }

    pub fn resolve_leave(&mut self, choice: LeaveChoice) -> Outcome<T> {
        if self.phase != Phase::Confirming {
            return Outcome::Ignored;
        }
        match choice {
            LeaveChoice::Save => self.commit(),
            LeaveChoice::Discard => self.cancel(),
            LeaveChoice::Stay => {
                self.phase = Phase::Editing;
                Outcome::Ignored
            }
        }
    }

    /// Records the current row order as saved while related settings still need saving.
    pub fn mark_saved(&mut self) {
        self.original.clone_from(&self.items);
        self.dirty = false;
    }

    /// Save: ends the mode with the new order (or `Unchanged`).
    pub fn commit(&mut self) -> Outcome<T> {
        if self.phase == Phase::Done {
            return Outcome::Ignored;
        }
        if self.dirty {
            let order = self.items.clone();
            self.original = order.clone();
            self.finish(Exit::Saved(order))
        } else {
            self.finish(Exit::Unchanged)
        }
    }

    /// Cancel: restores the original order and ends the mode. Focus stays
    /// on the same item, now at its original row.
    pub fn cancel(&mut self) -> Outcome<T> {
        if self.phase == Phase::Done {
            return Outcome::Ignored;
        }
        let changed = self.dirty;
        self.items = self.original.clone();
        self.dirty = false;
        self.finish(if changed {
            Exit::Discarded
        } else {
            Exit::Unchanged
        })
    }

    fn finish(&mut self, exit: Exit<T>) -> Outcome<T> {
        self.phase = Phase::Done;
        self.selected = false;
        Outcome::Done(exit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u32) -> Vec<u32> {
        (1..=n).collect()
    }

    fn mode(n: u32) -> Reorder<u32> {
        Reorder::new(ids(n), None)
    }

    #[test]
    fn opens_on_the_requested_item_or_the_first() {
        let r = Reorder::new(ids(5), Some(&3));
        assert_eq!((r.focused(), r.focus_index()), (Some(&3), Some(2)));
        assert_eq!(r.sub_mode(), SubMode::Browse);
        assert_eq!(r.selected(), None);
        let r = Reorder::new(ids(5), Some(&42));
        assert_eq!(r.focused(), Some(&1));
        assert!(!r.is_dirty() && !r.is_confirming() && !r.is_done());
    }

    #[test]
    fn an_empty_list_ignores_everything_and_leaves_unchanged() {
        let mut r: Reorder<u32> = Reorder::new(vec![], None).selecting_focused();
        assert_eq!(r.focused(), None);
        assert_eq!(r.sub_mode(), SubMode::Browse);
        for key in [Key::Up, Key::Down, Key::Left, Key::Right, Key::Ok] {
            assert_eq!(r.key(key), Outcome::Ignored);
        }
        assert_eq!(r.move_selected_to(0), Outcome::Ignored);
        assert_eq!(r.key(Key::Back), Outcome::Done(Exit::Unchanged));
    }

    #[test]
    fn up_down_move_focus_without_reordering_and_clamp() {
        let mut r = mode(3);
        assert_eq!(r.key(Key::Up), Outcome::Ignored);
        assert_eq!(r.key(Key::Down), Outcome::FocusMoved);
        assert_eq!(r.key(Key::Down), Outcome::FocusMoved);
        assert_eq!(r.key(Key::Down), Outcome::Ignored);
        assert_eq!(r.focused(), Some(&3));
        assert_eq!(r.key(Key::Up), Outcome::FocusMoved);
        assert_eq!(r.focused(), Some(&2));
        assert_eq!(r.items(), ids(3));
        assert!(!r.is_dirty());
    }

    #[test]
    fn left_right_without_a_selection_jump_focus_to_the_ends() {
        let mut r = Reorder::new(ids(5), Some(&3));
        assert_eq!(r.key(Key::Right), Outcome::FocusMoved);
        assert_eq!(r.focused(), Some(&5));
        assert_eq!(r.key(Key::Right), Outcome::Ignored);
        assert_eq!(r.key(Key::Left), Outcome::FocusMoved);
        assert_eq!(r.focused(), Some(&1));
        assert_eq!(r.items(), ids(5));
    }

    #[test]
    fn ok_selects_and_ok_again_deselects() {
        let mut r = Reorder::new(ids(3), Some(&2));
        assert_eq!(r.key(Key::Ok), Outcome::Selected);
        assert_eq!(r.selected(), Some(&2));
        assert_eq!(r.sub_mode(), SubMode::Move);
        assert_eq!(r.key(Key::Ok), Outcome::Deselected);
        assert_eq!(r.selected(), None);
        assert_eq!(r.focused(), Some(&2), "focus stays");
    }

    #[test]
    fn the_reference_entry_starts_selected_and_ok_then_down_only_moves_focus() {
        // docs/09: enter on the first row (selected), OK deselects, Down
        // moves focus and the order stays.
        let mut r = mode(5).selecting_focused();
        assert_eq!(r.selected(), Some(&1));
        r.key(Key::Ok);
        r.key(Key::Down);
        assert_eq!(r.focused(), Some(&2));
        assert_eq!(r.items(), ids(5));
        r.key(Key::Ok);
        assert_eq!(r.selected(), Some(&2));
    }

    #[test]
    fn up_down_move_the_selected_item_and_focus_follows_it() {
        let mut r = Reorder::new(ids(5), Some(&2));
        r.key(Key::Ok);
        assert_eq!(r.key(Key::Down), Outcome::Moved);
        assert_eq!(r.items(), [1, 3, 2, 4, 5]);
        assert_eq!((r.focused(), r.focus_index()), (Some(&2), Some(2)));
        r.key(Key::Down);
        assert_eq!(r.items(), [1, 3, 4, 2, 5]);
        assert_eq!(r.focus_index(), Some(3));
        r.key(Key::Up);
        r.key(Key::Up);
        r.key(Key::Up);
        assert_eq!(r.items(), [2, 1, 3, 4, 5]);
        assert_eq!(r.key(Key::Up), Outcome::Ignored, "already on top");
        assert_eq!(r.focused(), Some(&2));
        assert_eq!(r.selected(), Some(&2));
    }

    #[test]
    fn left_right_move_the_selected_item_to_the_top_and_bottom() {
        let mut r = Reorder::new(ids(5), Some(&3));
        r.key(Key::Ok);
        assert_eq!(r.key(Key::Left), Outcome::Moved);
        assert_eq!(r.items(), [3, 1, 2, 4, 5]);
        assert_eq!(r.key(Key::Left), Outcome::Ignored);
        assert_eq!(r.key(Key::Right), Outcome::Moved);
        assert_eq!(r.items(), [1, 2, 4, 5, 3]);
        assert_eq!(r.key(Key::Right), Outcome::Ignored);
        assert_eq!((r.focused(), r.focus_index()), (Some(&3), Some(4)));
    }

    #[test]
    fn after_deselecting_directions_move_focus_from_the_items_new_place() {
        let mut r = Reorder::new(ids(5), Some(&1));
        r.key(Key::Ok);
        r.key(Key::Right);
        r.key(Key::Ok);
        assert_eq!(r.items(), [2, 3, 4, 5, 1]);
        assert_eq!(r.key(Key::Up), Outcome::FocusMoved);
        assert_eq!(r.focused(), Some(&5));
        assert_eq!(r.items(), [2, 3, 4, 5, 1]);
    }

    #[test]
    fn focus_is_by_identity_for_any_item_type() {
        let mut r = Reorder::new(vec!["news", "film", "kids"], Some(&"kids"));
        r.key(Key::Ok);
        r.key(Key::Left);
        assert_eq!(r.items(), ["kids", "news", "film"]);
        assert_eq!(r.focused(), Some(&"kids"));
        r.key(Key::Ok);
        r.key(Key::Down);
        assert_eq!(r.focused(), Some(&"news"));
    }

    #[test]
    fn touch_focus_and_drag_to_a_row() {
        let mut r = mode(5);
        assert_eq!(r.focus_on(&4), Outcome::FocusMoved);
        assert_eq!(r.focus_on(&4), Outcome::Ignored);
        assert_eq!(r.focus_on(&9), Outcome::Ignored);
        assert_eq!(r.move_selected_to(0), Outcome::Ignored, "not selected");
        r.toggle_selection();
        assert_eq!(r.focus_on(&1), Outcome::Ignored, "a selection pins focus");
        assert_eq!(r.move_selected_to(1), Outcome::Moved);
        assert_eq!(r.items(), [1, 4, 2, 3, 5]);
        assert_eq!(r.move_selected_to(99), Outcome::Moved, "clamped");
        assert_eq!(r.items(), [1, 2, 3, 5, 4]);
        assert_eq!(r.move_selected_to(4), Outcome::Ignored);
        assert_eq!(r.focused(), Some(&4));
    }

    #[test]
    fn dirty_tracks_the_order_not_the_number_of_moves() {
        let mut r = mode(3);
        r.key(Key::Ok);
        r.key(Key::Down);
        assert!(r.is_dirty());
        r.key(Key::Up);
        assert!(!r.is_dirty(), "back to the original order");
        r.key(Key::Right);
        assert!(r.is_dirty());
    }

    #[test]
    fn hints_follow_the_sub_mode() {
        let mut r = mode(3);
        let actions = |r: &Reorder<u32>| r.hints().map(|h| h.action);
        let keys = r.hints().map(|h| h.key);
        assert_eq!(
            keys,
            [
                HintKey::Ok,
                HintKey::UpDown,
                HintKey::LeftRight,
                HintKey::Back
            ]
        );
        assert_eq!(
            actions(&r),
            [
                HintAction::Select,
                HintAction::MoveFocus,
                HintAction::FocusEnds,
                HintAction::Leave
            ]
        );
        r.key(Key::Ok);
        assert_eq!(
            actions(&r),
            [
                HintAction::Deselect,
                HintAction::MoveItem,
                HintAction::MoveItemToEnds,
                HintAction::Leave
            ]
        );
        r.key(Key::Down);
        assert_eq!(
            actions(&r)[1],
            HintAction::MoveItem,
            "persists while moving"
        );
        r.key(Key::Ok);
        assert_eq!(actions(&r)[0], HintAction::Select);
    }

    #[test]
    fn commit_returns_the_new_order_and_ends_the_mode() {
        let mut r = mode(3);
        r.key(Key::Ok);
        r.key(Key::Right);
        assert_eq!(r.commit(), Outcome::Done(Exit::Saved(vec![2, 3, 1])));
        assert!(r.is_done());
        assert_eq!(r.original(), [2, 3, 1]);
        assert_eq!(r.selected(), None);
        for key in [Key::Ok, Key::Up, Key::Back] {
            assert_eq!(r.key(key), Outcome::Ignored);
        }
        assert_eq!(r.commit(), Outcome::Ignored);
        assert_eq!(r.cancel(), Outcome::Ignored);
    }

    #[test]
    fn commit_without_changes_is_unchanged() {
        let mut r = mode(3);
        r.key(Key::Down);
        assert_eq!(r.commit(), Outcome::Done(Exit::Unchanged));
    }

    #[test]
    fn cancel_restores_the_original_order_and_keeps_focus_on_the_item() {
        let mut r = Reorder::new(ids(4), Some(&1));
        r.key(Key::Ok);
        r.key(Key::Right);
        assert_eq!(r.items(), [2, 3, 4, 1]);
        assert_eq!(r.cancel(), Outcome::Done(Exit::Discarded));
        assert_eq!(r.items(), ids(4));
        assert!(!r.is_dirty());
        assert_eq!((r.focused(), r.focus_index()), (Some(&1), Some(0)));
        let mut clean = mode(2);
        assert_eq!(clean.cancel(), Outcome::Done(Exit::Unchanged));
    }

    #[test]
    fn back_without_changes_leaves_at_once() {
        let mut r = mode(3);
        r.key(Key::Ok);
        assert_eq!(r.key(Key::Back), Outcome::Done(Exit::Unchanged));
    }

    #[test]
    fn back_with_changes_asks_and_locks_the_list_until_answered() {
        let mut r = mode(3);
        r.key(Key::Ok);
        r.key(Key::Down);
        assert_eq!(r.key(Key::Back), Outcome::ConfirmLeave);
        assert!(r.is_confirming());
        for key in [Key::Up, Key::Down, Key::Ok, Key::Back] {
            assert_eq!(r.key(key), Outcome::Ignored);
        }
        assert_eq!(r.move_selected_to(0), Outcome::Ignored);
        assert_eq!(r.toggle_selection(), Outcome::Ignored);
        assert_eq!(r.focus_on(&3), Outcome::Ignored);
        assert_eq!(r.items(), [2, 1, 3]);
    }

    #[test]
    fn stay_returns_to_editing_with_changes_and_selection_kept() {
        let mut r = mode(3);
        r.key(Key::Ok);
        r.key(Key::Down);
        r.key(Key::Back);
        assert_eq!(r.resolve_leave(LeaveChoice::Stay), Outcome::Ignored);
        assert!(!r.is_confirming());
        assert!(r.is_dirty());
        assert_eq!(r.selected(), Some(&1));
        assert_eq!(r.key(Key::Down), Outcome::Moved);
        assert_eq!(r.items(), [2, 3, 1]);
    }

    #[test]
    fn save_and_discard_answers_end_the_mode() {
        let mut r = mode(3);
        r.key(Key::Ok);
        r.key(Key::Down);
        r.key(Key::Back);
        assert_eq!(
            r.resolve_leave(LeaveChoice::Save),
            Outcome::Done(Exit::Saved(vec![2, 1, 3]))
        );
        let mut r = mode(3);
        r.key(Key::Ok);
        r.key(Key::Down);
        r.key(Key::Back);
        assert_eq!(
            r.resolve_leave(LeaveChoice::Discard),
            Outcome::Done(Exit::Discarded)
        );
        assert_eq!(r.items(), ids(3));
    }

    #[test]
    fn resolve_leave_without_a_question_does_nothing() {
        let mut r = mode(3);
        for choice in [LeaveChoice::Save, LeaveChoice::Discard, LeaveChoice::Stay] {
            assert_eq!(r.resolve_leave(choice), Outcome::Ignored);
        }
        assert!(!r.is_done());
    }

    #[test]
    fn a_single_item_never_moves() {
        let mut r = mode(1).selecting_focused();
        for key in [Key::Up, Key::Down, Key::Left, Key::Right] {
            assert_eq!(r.key(key), Outcome::Ignored);
        }
        assert!(!r.is_dirty());
    }
}
