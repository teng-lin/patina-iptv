//! Identity-based organization editor. Imported items are never deleted.

mod open;
pub use open::{Items, Open, OrderSave, Placement};

use crate::reorder::{LeaveChoice, Reorder};
use iptv_core::organization::{Edit, Item, Sort};
use std::collections::{BTreeSet, HashSet};

/// Which operation owns the editor's row selection and direction keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Options,
    Visibility,
    Blocking,
    Reorder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Visibility,
    Rename,
    ResetName,
    Earlier,
    Later,
    First,
    Last,
    ResetOrder,
    Select,
    HideSelected,
    ShowSelected,
    HideAll,
    ShowAll,
    Blocking,
    Channels,
    SortChannels,
    Reorder,
    SortOptions,
    Sort(Sort),
}
impl Action {
    /// The stored edit requested by an action handled by the library.
    pub fn edit(self) -> Option<Edit> {
        Some(match self {
            Self::Visibility => Edit::Visibility,
            Self::ResetName => Edit::ResetName,
            Self::Earlier => Edit::Earlier,
            Self::Later => Edit::Later,
            Self::First => Edit::First,
            Self::Last => Edit::Last,
            Self::ResetOrder => Edit::ResetOrder,
            Self::HideSelected => Edit::HideSelected,
            Self::ShowSelected => Edit::ShowSelected,
            Self::HideAll => Edit::HideAll,
            Self::ShowAll => Edit::ShowAll,
            Self::Rename
            | Self::Select
            | Self::Blocking
            | Self::Channels
            | Self::SortChannels
            | Self::Reorder
            | Self::SortOptions
            | Self::Sort(_) => return None,
        })
    }
}

pub const ACTIONS: &[Action] = &[
    Action::Visibility,
    Action::Rename,
    Action::ResetName,
    Action::Earlier,
    Action::Later,
    Action::First,
    Action::Last,
    Action::ResetOrder,
    Action::Select,
    Action::HideSelected,
    Action::ShowSelected,
    Action::HideAll,
    Action::ShowAll,
];
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    pub items: Vec<Item>,
    pub focus: Option<String>,
    pub action: Option<Action>,
    pub text: Option<String>,
    pub selected: BTreeSet<String>,
    pub mode: Mode,
    pub sort: Sort,
    pub sort_focus: Option<Sort>,
    pub reorder: Option<Reorder<Item>>,
    pub leave_choice: LeaveChoice,
    actions: Vec<Action>,
    sorts: &'static [Sort],
}
impl Editor {
    pub fn new(items: Vec<Item>, preferred: Option<String>) -> Self {
        let focus = preferred
            .filter(|id| items.iter().any(|i| &i.id == id))
            .or_else(|| items.first().map(|i| i.id.clone()));
        Self {
            items,
            focus,
            action: None,
            text: None,
            selected: BTreeSet::new(),
            mode: Mode::Options,
            sort: Sort::Manual,
            sort_focus: None,
            reorder: None,
            leave_choice: LeaveChoice::Save,
            actions: ACTIONS.to_vec(),
            sorts: SORTS,
        }
    }
    /// Sets the actions available for rows in this list.
    pub fn set_actions(&mut self, actions: Vec<Action>) {
        self.actions = actions;
    }

    pub fn set_sorts(&mut self, sorts: &'static [Sort]) {
        self.sorts = sorts;
    }

    pub fn sorts(&self) -> &'static [Sort] {
        self.sorts
    }

    /// Actions offered for the focused row.
    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    /// Starts an order draft from the currently displayed order.
    pub fn start_reorder(&mut self, include: impl Fn(&Item) -> bool) {
        self.mode = Mode::Reorder;
        self.action = None;
        self.sort_focus = None;
        let focused = self.focused();
        let items = self
            .items
            .iter()
            .filter(|item| include(item))
            .cloned()
            .collect();
        self.reorder = Some(Reorder::new(items, focused));
        self.leave_choice = LeaveChoice::Save;
    }

    /// Merges the reordered rows back into the complete list. Any rows
    /// omitted from the draft keep their slots in the saved order.
    pub fn order_ids(&self) -> Option<(Vec<String>, Vec<String>)> {
        let draft = self.reorder.as_ref()?;
        let original: Vec<_> = self.items.iter().map(|item| item.id.clone()).collect();
        let included: HashSet<_> = draft.original().iter().map(|item| &item.id).collect();
        let mut reordered = draft.items().iter();
        let order = self
            .items
            .iter()
            .map(|item| {
                if included.contains(&item.id) {
                    reordered.next().unwrap_or(item).id.clone()
                } else {
                    item.id.clone()
                }
            })
            .collect();
        Some((original, order))
    }

    /// Items in the order currently shown, including an unsaved order.
    pub fn rows(&self) -> &[Item] {
        self.reorder.as_ref().map_or(&self.items, Reorder::items)
    }

    /// The focused identity's current position in the displayed list.
    pub fn focus_index(&self) -> Option<usize> {
        match &self.reorder {
            Some(order) => order.focus_index(),
            None => self
                .items
                .iter()
                .position(|i| Some(&i.id) == self.focus.as_ref()),
        }
    }

    /// Focuses a row by identity without opening its actions.
    pub fn focus_item(&mut self, id: String) {
        if self.items.iter().any(|i| i.id == id) {
            self.focus = Some(id);
        }
    }

    pub fn focused(&self) -> Option<&Item> {
        self.items
            .iter()
            .find(|i| Some(&i.id) == self.focus.as_ref())
    }
    pub fn reload(&mut self, items: Vec<Item>) {
        // An open draft keeps its original identity set for validation at save.
        if self.reorder.is_some() {
            return;
        }
        let present: HashSet<_> = items.iter().map(|item| item.id.as_str()).collect();
        if self
            .focus
            .as_ref()
            .is_none_or(|id| !present.contains(id.as_str()))
        {
            let fallback = self.focus.as_ref().and_then(|id| {
                let at = self.items.iter().position(|item| &item.id == id)?;
                self.items[at + 1..]
                    .iter()
                    .chain(self.items[..at].iter().rev())
                    .find(|item| present.contains(item.id.as_str()))
            });
            self.focus = fallback
                .or_else(|| items.first())
                .map(|item| item.id.clone());
            self.action = None;
            self.text = None;
        }
        self.selected.retain(|id| present.contains(id.as_str()));
        self.items = items;
    }

    pub fn step(&mut self, by: isize) {
        if self.text.is_some() {
            return;
        }
        if let Some(sort) = self.sort_focus {
            self.sort_focus = Some(step_choice(self.sorts, sort, by));
        } else if let Some(a) = self.action {
            self.action = Some(step_choice(&self.actions, a, by));
        } else if !self.items.is_empty() {
            let at = self
                .items
                .iter()
                .position(|i| Some(&i.id) == self.focus.as_ref())
                .unwrap_or(0);
            self.focus = Some(
                self.items[(at as isize + by).clamp(0, self.items.len() as isize - 1) as usize]
                    .id
                    .clone(),
            );
        }
    }
    pub fn open_actions(&mut self, id: String) {
        if self.items.iter().any(|i| i.id == id) {
            self.focus = Some(id);
            self.action = self.actions.first().copied();
        }
    }
    /// True when a sheet was dismissed; false when the caller should close the editor.
    pub fn back(&mut self) -> bool {
        if self.text.take().is_some() {
            return true;
        }
        self.sort_focus.take().is_some() || self.action.take().is_some()
    }
    pub fn toggle_selection(&mut self) {
        if let Some(id) = self.focus.clone()
            && !self.selected.remove(&id)
        {
            self.selected.insert(id);
        }
        self.action = None;
    }
}
/// Choices in the guide's sort picker.
const SORTS: &[Sort] = &[
    Sort::Provider,
    Sort::Name,
    Sort::NameDescending,
    Sort::LastWatched,
    Sort::OldestWatched,
    Sort::MostWatched,
    Sort::LeastWatched,
    Sort::Manual,
];
pub const GROUP_SORTS: &[Sort] = &[
    Sort::Provider,
    Sort::Name,
    Sort::NameDescending,
    Sort::Manual,
];

/// Choices offered when leaving a modified order.
pub const LEAVE_CHOICES: [LeaveChoice; 3] =
    [LeaveChoice::Save, LeaveChoice::Discard, LeaveChoice::Stay];

/// Steps among the available choices, clamping at either end.
pub fn step_choice<T: Copy + Eq>(choices: &[T], current: T, by: isize) -> T {
    let at = choices
        .iter()
        .position(|choice| *choice == current)
        .unwrap_or(0);
    choices[(at as isize + by).clamp(0, choices.len() as isize - 1) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(id: &str) -> Item {
        Item {
            id: id.into(),
            name: "Duplicate name".into(),
            hidden: false,
        }
    }
    #[test]
    fn identity_survives_reorder_and_hidden_items_are_recoverable() {
        let mut e = Editor::new(vec![item("a"), item("b")], Some("b".into()));
        e.open_actions("b".into());
        e.toggle_selection();
        let mut b = item("b");
        b.hidden = true;
        e.reload(vec![b, item("a")]);
        assert_eq!(e.focus.as_deref(), Some("b"));
        assert!(e.focused().unwrap().hidden);
        assert!(e.selected.contains("b"));
        e.reload(vec![item("a")]);
        assert_eq!(e.focus.as_deref(), Some("a"));
        assert!(e.selected.is_empty());
        e.reload(vec![]);
        e.step(1);
        assert!(e.focus.is_none());
    }
    #[test]
    fn back_cancels_rename_then_actions_then_leaves() {
        let mut e = Editor::new(vec![item("a")], None);
        e.open_actions("a".into());
        e.text = Some("new".into());
        assert!(e.back());
        assert!(e.action.is_some());
        assert!(e.back());
        assert!(!e.back());
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    fn item(id: &str) -> Item {
        Item {
            id: id.into(),
            name: format!("Name {id}"),
            hidden: false,
        }
    }
    #[test]
    fn remote_navigation_clamps_and_a_name_draft_keeps_its_identity() {
        let mut editor = Editor::new(
            vec![item("a"), item("b"), item("c")],
            Some("missing".into()),
        );
        assert_eq!(editor.focus.as_deref(), Some("a"));
        editor.step(1);
        assert_eq!(editor.focus.as_deref(), Some("b"));
        editor.step(100);
        assert_eq!(editor.focus.as_deref(), Some("c"));
        editor.step(-100);
        assert_eq!(editor.focus.as_deref(), Some("a"));
        editor.open_actions("missing".into());
        assert_eq!(editor.action, None);
        editor.open_actions("b".into());
        editor.step(1);
        assert_eq!(editor.action, Some(Action::Rename));
        editor.text = Some("Draft".into());
        editor.step(100);
        assert_eq!(editor.focus.as_deref(), Some("b"));
        assert_eq!(editor.action, Some(Action::Rename));
        assert!(editor.back());
        editor.step(100);
        assert_eq!(editor.action, Some(Action::ShowAll));
        editor.step(-100);
        assert_eq!(editor.action, Some(Action::Visibility));
        editor.toggle_selection();
        assert!(editor.selected.contains("b"));
        assert_eq!(editor.action, None);
        editor.toggle_selection();
        assert!(editor.selected.is_empty());
        editor.reload(Vec::new());
        editor.toggle_selection();
        assert!(editor.selected.is_empty());
    }
    #[test]
    fn removing_a_channel_during_rename_cannot_rename_its_neighbor() {
        let mut editor = Editor::new(vec![item("a"), item("b"), item("c")], Some("c".into()));
        editor.open_actions("c".into());
        editor.text = Some("Only for C".into());
        editor.selected.insert("c".into());
        editor.reload(vec![item("a"), item("b")]);
        assert_eq!(editor.focus.as_deref(), Some("b"));
        assert_eq!(editor.action, None);
        assert_eq!(
            editor.text, None,
            "cancel a draft whose content identity disappeared"
        );
        assert!(editor.selected.is_empty());
        editor.reload(vec![]);
        assert!(editor.focused().is_none());
        assert!(!editor.back());
    }
}
