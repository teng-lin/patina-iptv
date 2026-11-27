//! Open organization flows and their save decisions.
use super::{ACTIONS, Action as Edit, Editor, GROUP_SORTS, Mode};
use crate::lists::ListId;
use crate::reorder::{Key, LeaveChoice, Outcome};
use iptv_core::organization::{Blocks, Item, Sort};
use iptv_library::{OrganizationScope as Kind, SourceId};
use std::collections::HashMap;

pub struct Items {
    pub rows: Vec<Item>,
    pub sort: Sort,
    pub blocks: Blocks,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    #[default]
    Screen,
    Panel(ListId),
    Sort(ListId),
    RenameGroup {
        restore_name: bool,
    },
    Groups,
    Guide {
        list: ListId,
        return_to_groups: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    /// A source-scoped organization editor, or the whole playlist library.
    pub scope: Option<(SourceId, Kind)>,
    pub editor: Editor,
    pub saving: bool,
    pub blocks: Blocks,
    pub placement: Placement,
    pub custom_from: Option<Sort>,
    pub saved_playlist_positions: Option<Vec<SourceId>>,
    pub bulk_visibility: Option<(Edit, bool)>,
    pub dragging: bool,
}
pub enum OrderSave {
    Unchanged,
    Playlists {
        ids: Vec<SourceId>,
        already_saved: bool,
    },
    Pending {
        source: SourceId,
        kind: Kind,
        original: Vec<String>,
        order: Vec<String>,
        expected_sort: Sort,
        from_custom: bool,
    },
}

impl Open {
    pub fn new(source: SourceId, kind: Kind, mode: Mode, placement: Placement) -> Self {
        let mut editor = Editor::new(Vec::new(), None);
        editor.mode = mode;
        Self::with_editor(Some((source, kind)), editor, placement)
    }

    pub fn with_editor(
        scope: Option<(SourceId, Kind)>,
        editor: Editor,
        placement: Placement,
    ) -> Self {
        Self {
            scope,
            editor,
            saving: false,
            blocks: Blocks::default(),
            placement,
            custom_from: None,
            saved_playlist_positions: None,
            bulk_visibility: None,
            dragging: false,
        }
    }

    pub fn kind(&self) -> Option<&Kind> {
        self.scope.as_ref().map(|(_, kind)| kind)
    }

    pub fn load(
        &mut self,
        loaded: Items,
        focus: Option<String>,
        mode: Mode,
        initial: Option<Edit>,
        guide_keys: Option<&HashMap<String, usize>>,
    ) {
        let Items { rows, sort, blocks } = loaded;
        self.blocks = blocks;
        self.editor = Editor::new(rows, focus.clone());
        self.editor.sort = sort;
        self.custom_from = (initial == Some(Edit::Sort(Sort::Manual))).then_some(sort);
        self.editor.mode = mode;
        self.configure_actions();
        match initial {
            Some(Edit::SortOptions) => self.editor.sort_focus = Some(sort),
            Some(Edit::Rename) if self.editor.focus == focus => {
                self.editor.text = self.editor.focused().map(|item| item.name.clone());
            }
            Some(Edit::Visibility) if self.editor.focus == focus => {
                if let Some(id) = focus {
                    self.editor.open_actions(id);
                }
            }
            _ => {}
        }
        if mode == Mode::Reorder {
            let include_hidden = self.placement == Placement::Screen;
            self.editor.start_reorder(|item| match guide_keys {
                Some(keys) => keys.contains_key(&item.id),
                None => include_hidden || !item.hidden,
            });
        }
    }

    fn configure_actions(&mut self) {
        if self.kind() == Some(&Kind::Groups) {
            self.editor.set_sorts(GROUP_SORTS);
        }
        let mut actions = vec![Edit::Visibility, Edit::Blocking];
        if self.kind() == Some(&Kind::Groups) {
            actions.push(Edit::Channels);
        }
        if self.kind() != Some(&Kind::Favorites) {
            actions.push(Edit::SortOptions);
        }
        if self.kind() == Some(&Kind::Groups) {
            actions.extend([Edit::Reorder, Edit::SortChannels]);
        }
        actions.extend(ACTIONS.iter().copied().skip(1));
        self.editor.set_actions(actions);
    }

    fn restore_items(&mut self, loaded: Items) {
        self.blocks = loaded.blocks;
        self.editor.sort = loaded.sort;
        self.editor.reload(loaded.rows);
    }

    pub fn refresh(&mut self, loaded: Items) {
        if !self.saving && self.editor.reorder.is_none() {
            self.restore_items(loaded);
        }
    }

    pub fn begin_save(&mut self) {
        self.saving = true;
    }

    pub fn finish_save(&mut self) {
        self.saving = false;
    }

    pub fn begin_block(&mut self) {
        self.editor.action = None;
        self.begin_save();
    }

    pub fn finish_block(&mut self, blocks: Blocks) {
        self.blocks = blocks;
        self.finish_save();
    }

    pub fn finish_edit(&mut self, expected: &Self, loaded: Option<Items>) {
        self.finish_save();
        if let Some(loaded) = loaded {
            self.restore_items(loaded);
        }
        if self.editor.action == expected.editor.action {
            self.editor.action = None;
        }
        if self.editor.text == expected.editor.text {
            self.editor.text = None;
        }
    }

    pub fn request_bulk_visibility(&mut self, edit: Edit) {
        if !self.saving {
            self.bulk_visibility = Some((edit, false));
        }
    }

    pub fn focus_bulk_visibility(&mut self, confirm: bool) {
        if let Some((_, focus)) = &mut self.bulk_visibility {
            *focus = confirm;
        }
    }

    pub fn dismiss_bulk_visibility(&mut self) -> Option<Edit> {
        self.bulk_visibility.take().map(|(edit, _)| edit)
    }

    pub fn activate_row(&mut self, id: String) -> Option<Edit> {
        if self.saving {
            return None;
        }
        let edit = match self.editor.mode {
            Mode::Options => {
                self.editor.open_actions(id);
                return None;
            }
            Mode::Reorder => {
                if self.scope.is_none()
                    && let Some(order) = self.editor.reorder.as_mut()
                    && let Some(item) = order.items().iter().find(|row| row.id == id).cloned()
                {
                    if order.selected().is_some() {
                        order.toggle_selection();
                    } else {
                        order.focus_on(&item);
                        order.toggle_selection();
                    }
                }
                return None;
            }
            Mode::Visibility => Edit::Visibility,
            Mode::Blocking => Edit::Blocking,
        };
        self.editor.focus_item(id);
        Some(edit)
    }

    pub fn reorder_key(&mut self, key: Key, drag_editing: bool) -> Outcome<Item> {
        if self.saving {
            return Outcome::Ignored;
        }
        let Some(order) = &mut self.editor.reorder else {
            return Outcome::Ignored;
        };
        let outcome = if key == Key::Back {
            order.leave_with_changes(
                !drag_editing
                    && (self.custom_from.is_some_and(|sort| sort != Sort::Manual)
                        || self.saved_playlist_positions.is_some()),
            )
        } else {
            order.key(key)
        };
        if matches!(outcome, Outcome::ConfirmLeave) {
            self.editor.leave_choice = LeaveChoice::Save;
        }
        outcome
    }

    pub fn resolve_order(&mut self, choice: LeaveChoice) -> Outcome<Item> {
        if self.saving {
            return Outcome::Ignored;
        }
        self.editor
            .reorder
            .as_mut()
            .map_or(Outcome::Ignored, |order| order.resolve_leave(choice))
    }

    pub fn begin_order_save(&mut self) -> Option<OrderSave> {
        if self.saving || self.dragging {
            return None;
        }
        let (original, order) = self.editor.order_ids()?;
        if original == order
            && self.saved_playlist_positions.is_none()
            && self.custom_from.is_none_or(|sort| sort == Sort::Manual)
        {
            return Some(OrderSave::Unchanged);
        }
        let Some((source, kind)) = self.scope.clone() else {
            let ids = order
                .iter()
                .map(|id| id.parse().ok().map(SourceId))
                .collect::<Option<_>>()?;
            let already_saved = self.saved_playlist_positions.as_ref() == Some(&ids);
            self.begin_save();
            return Some(OrderSave::Playlists { ids, already_saved });
        };
        self.begin_save();
        Some(OrderSave::Pending {
            source,
            kind,
            original,
            order,
            expected_sort: self.editor.sort,
            from_custom: self.custom_from.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(ids: &[&str], sort: Sort) -> Items {
        Items {
            rows: ids
                .iter()
                .map(|id| Item {
                    id: (*id).into(),
                    name: format!("Generated {id}"),
                    hidden: false,
                })
                .collect(),
            sort,
            blocks: Blocks::default(),
        }
    }

    #[test]
    fn preference_conversion_can_save_an_unchanged_custom_order() {
        let mut screen = Open::new(SourceId(1), Kind::Groups, Mode::Reorder, Placement::Screen);
        screen.load(
            loaded(&["amber", "birch"], Sort::Name),
            Some("birch".into()),
            Mode::Reorder,
            Some(Edit::Sort(Sort::Manual)),
            None,
        );
        let Some(OrderSave::Pending {
            source,
            kind,
            original,
            order,
            expected_sort,
            from_custom,
        }) = screen.begin_order_save()
        else {
            panic!("the custom order has not yet been saved");
        };
        assert_eq!(source, SourceId(1));
        assert_eq!(kind, Kind::Groups);
        assert!(from_custom);
        assert_eq!(expected_sort, Sort::Name);
        assert_eq!(original, order);
        screen.refresh(loaded(&["oak"], Sort::Provider));
        assert_eq!(screen.editor.focus.as_deref(), Some("birch"));
    }

    #[test]
    fn edit_completion_preserves_new_input_and_restores_row_identity() {
        let mut screen = Open::new(SourceId(1), Kind::Groups, Mode::Options, Placement::Screen);
        screen.load(
            loaded(&["amber", "birch"], Sort::Name),
            Some("birch".into()),
            Mode::Options,
            Some(Edit::Rename),
            None,
        );
        let expected = screen.clone();
        screen.begin_save();
        screen.editor.text = Some("New input".into());
        screen.finish_edit(&expected, Some(loaded(&["birch", "amber"], Sort::Provider)));
        assert!(!screen.saving);
        assert_eq!(screen.editor.focus.as_deref(), Some("birch"));
        assert_eq!(screen.editor.text.as_deref(), Some("New input"));
        assert_eq!(screen.editor.sort, Sort::Provider);

        screen.request_bulk_visibility(Edit::HideAll);
        screen.focus_bulk_visibility(true);
        assert_eq!(screen.bulk_visibility, Some((Edit::HideAll, true)));
        assert_eq!(screen.dismiss_bulk_visibility(), Some(Edit::HideAll));
        assert!(screen.bulk_visibility.is_none());
    }
}
