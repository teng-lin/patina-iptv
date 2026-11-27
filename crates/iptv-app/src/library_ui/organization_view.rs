//! Channel and provider-group organization, including hidden-item recovery.
use crate::chrome::i18n::{tr, trf};
use crate::chrome::input::dismiss_keyboard;
use crate::chrome::scale::Scale;
use crate::chrome::ui_kit::entry::{self, TextSheet};
use crate::guide::list_rows::{self, Mark, ROW};
use crate::runtime::{Error, model::SourceId};
use crate::{Ui, chrome::ui_kit, wiring::data};
use iptv_core::organization::{Block, BlockKind, Item, Sort};
use iptv_nav::Action;
use iptv_presentation::guide::Pane;
use iptv_presentation::lists::ListId;
use iptv_presentation::organization::{
    Action as Edit, Editor, Items, LEAVE_CHOICES, Mode, OrderSave, Placement, step_choice,
};
use iptv_presentation::reorder::{Key, LeaveChoice, Outcome};
use nsh_core::{Checked, prelude::*};
use std::collections::{HashMap, HashSet};

pub use iptv_library::OrganizationScope as Kind;
pub use iptv_presentation::organization::Open;

async fn items(source: SourceId, kind: &Kind, order: Option<Sort>) -> Result<Items, Error> {
    crate::runtime::commands::organization_editor(
        &crate::wiring::runtime_owner::runtime(),
        source,
        kind.clone(),
        crate::guide::favorites::ordered_keys(),
        order,
    )
    .await
    .map(|loaded| Items {
        rows: loaded.rows,
        sort: loaded.sort,
        blocks: loaded.blocks,
    })
}

fn still_open(ui: Ui, generation: u64) -> bool {
    ui.screens.organization.is_current(generation)
}

pub fn open(ui: Ui, source: SourceId, kind: Kind, focus: Option<String>) {
    open_with(
        ui,
        source,
        kind,
        focus,
        Mode::Options,
        None,
        Placement::Screen,
    );
}

pub fn open_mode(ui: Ui, source: SourceId, kind: Kind, focus: Option<String>, mode: Mode) {
    let initial = (mode == Mode::Options).then_some(Edit::Visibility);
    open_with(ui, source, kind, focus, mode, initial, Placement::Screen);
}

pub fn rename_group(ui: Ui, source: SourceId, id: String) {
    let restore_name = matches!(Kind::group_channels(&id), Kind::GroupChannels(_));
    open_with(
        ui,
        source,
        Kind::Groups,
        Some(id),
        Mode::Options,
        Some(Edit::Rename),
        Placement::RenameGroup { restore_name },
    );
}

/// The group column displays just the playlist headings during a move.
/// Its normal catalog keeps the viewer's expansion and visibility choices.
pub fn open_playlist_reorder(ui: Ui, preferred: Option<SourceId>) {
    let Ok(playlists) = crate::runtime().playlists() else {
        return;
    };
    if playlists.is_empty()
        || (preferred.is_none()
            && (playlists.len() < 2 || !crate::wiring::settings::playlists_by_hand()))
    {
        return;
    }
    let rows = playlists
        .into_iter()
        .map(|p| Item {
            id: p.id.0.to_string(),
            name: p.name,
            hidden: !p.policy.enabled,
        })
        .collect();
    let mut editor = Editor::new(rows, preferred.map(|id| id.0.to_string()));
    editor.start_reorder(|_| true);
    batch(|| {
        if crate::screens::settings_view::is_open(ui) {
            crate::screens::settings_view::close(ui);
        }
        crate::screens::channels_view::close(ui);
        ui.playback.panels.update(|panels| panels.hide());
        ui.chrome
            .mode
            .apply(crate::chrome::mode::Action::Restore(crate::ViewMode::Guide));
        crate::guide::lists::open_groups(ui);
        crate::playback::browse_playback::hold(ui);
        ui.screens
            .organization
            .open(Open::with_editor(None, editor, Placement::Groups));
    });
}

pub fn open_group_reorder(ui: Ui, source: SourceId, focus: Option<String>, list: ListId) {
    batch(|| {
        leave_fullscreen_for_reorder(ui);
        if let Some(entry) = list.resolve(&*crate::wiring::runtime_owner::schedule()) {
            crate::guide::lists::preview(ui, entry);
        }
        crate::guide::lists::open_groups(ui);
        crate::playback::browse_playback::hold(ui);
        open_with(
            ui,
            source,
            Kind::Groups,
            focus,
            Mode::Reorder,
            None,
            Placement::Groups,
        );
    });
}

pub fn open_context_reorder(
    ui: Ui,
    source: SourceId,
    kind: Kind,
    focus: Option<String>,
    list: ListId,
) {
    batch(|| {
        let from_player = leave_fullscreen_for_reorder(ui);
        let placement = guide_reorder_placement(ui, list.clone());
        if from_player
            && let Some(id) = &focus
            && let Some(&channel) = guide_order_keys(ui, source, &list).get(id)
        {
            let now = ui.guide.focus.with_untracked(|f| f.now);
            crate::guide::lists::tap(ui, channel, now);
        }
        crate::playback::browse_playback::hold(ui);
        open_with(ui, source, kind, focus, Mode::Reorder, None, placement);
    });
}

fn leave_fullscreen_for_reorder(ui: Ui) -> bool {
    let from_player = ui.chrome.mode.get_untracked() == crate::ViewMode::Fullscreen;
    if from_player {
        crate::screens::channels_view::close(ui);
        ui.playback.panels.update(|panels| panels.hide());
        ui.chrome
            .mode
            .apply(crate::chrome::mode::Action::ReturnToBrowse);
    }
    from_player
}

fn reorder_placement(ui: Ui, kind: &Kind, list: ListId) -> Placement {
    if idiom() != nsh_core::platform::Idiom::Tv
        || ui.chrome.mode.get_untracked() == crate::ViewMode::Fullscreen
    {
        Placement::Panel(list)
    } else if *kind == Kind::Groups {
        crate::guide::lists::open_groups(ui);
        Placement::Groups
    } else {
        guide_reorder_placement(ui, list)
    }
}

fn guide_reorder_placement(ui: Ui, list: ListId) -> Placement {
    let return_to_groups = ui.guide.focus.with_untracked(|f| f.pane == Pane::Groups);
    let entry = list.find(&*crate::wiring::runtime_owner::schedule());
    crate::guide::lists::preview(ui, entry);
    ui.guide.focus.update(|f| {
        f.close_columns();
        f.hint = false;
    });
    Placement::Guide {
        list,
        return_to_groups,
    }
}

pub fn open_sort(ui: Ui, source: SourceId, kind: Kind, list: Option<ListId>) {
    open_with(
        ui,
        source,
        kind,
        None,
        Mode::Options,
        Some(Edit::SortOptions),
        list.map_or(Placement::Screen, Placement::Sort),
    );
}

fn open_with(
    ui: Ui,
    source: SourceId,
    kind: Kind,
    focus: Option<String>,
    mode: Mode,
    initial: Option<Edit>,
    placement: Placement,
) {
    let generation = ui
        .screens
        .organization
        .open(Open::new(source, kind.clone(), mode, placement));
    crate::wiring::tasks::spawn_in_app(async move {
        let custom = initial == Some(Edit::Sort(Sort::Manual));
        let loaded = items(source, &kind, custom.then_some(Sort::Manual)).await;
        if !still_open(ui, generation) || !is_open(ui) {
            return;
        }
        match loaded {
            Ok(loaded) => {
                if initial == Some(Edit::Rename)
                    && !loaded
                        .rows
                        .iter()
                        .any(|item| Some(&item.id) == focus.as_ref())
                {
                    close(ui);
                    return;
                }
                update(ui, |state| {
                    let guide_keys = match &state.placement {
                        Placement::Guide { list, .. } if mode == Mode::Reorder => {
                            Some(guide_order_keys(ui, source, list))
                        }
                        _ => None,
                    };
                    state.load(loaded, focus, mode, initial, guide_keys.as_ref());
                });
            }
            Err(error) => {
                log::warn!("iptv organization: {error}");
                close(ui);
            }
        }
    });
}
fn block_kind(kind: &Kind) -> BlockKind {
    if *kind == Kind::Groups {
        BlockKind::Group
    } else {
        BlockKind::Channel
    }
}

fn block_state(state: &Open, id: &str) -> Block {
    let Some(kind) = state.kind() else {
        return Block::None;
    };
    match block_kind(kind) {
        BlockKind::Group if state.blocks.groups.contains(id) => Block::Group,
        BlockKind::Group => Block::None,
        BlockKind::Channel => state.blocks.channel(id),
    }
}

pub fn set_block(ui: Ui, source: SourceId, kind: BlockKind, id: String, blocked: bool) {
    let generation = ui.screens.organization.epoch();
    let owner = crate::wiring::runtime_owner::runtime();
    let save = crate::runtime::commands::set_guide_block(&owner, source, kind, id, blocked);
    crate::wiring::tasks::spawn_in_app(async move {
        if save.await.is_err() {
            crate::chrome::toast::notify(tr(
                "The change could not be saved. Reopen the list and try again.",
            ));
            if still_open(ui, generation) {
                update(ui, Open::finish_save);
            }
            return;
        }
        crate::chrome::toast::notify(if blocked {
            tr("Content blocked")
        } else {
            tr("Content unblocked")
        });
        data::organization_changed(&owner, source);
        if !still_open(ui, generation) || !is_open(ui) {
            return;
        }
        let blocks = crate::runtime::commands::guide_blocks(&owner, source).await;
        if !still_open(ui, generation) {
            return;
        }
        match blocks {
            Ok(blocks) => update(ui, |state| state.finish_block(blocks)),
            Err(_) => {
                close(ui);
                crate::chrome::toast::notify(tr(
                    "The list could not be reloaded. Reopen it and try again.",
                ));
            }
        }
    });
}

pub fn is_open(ui: Ui) -> bool {
    ui.screens.organization.is_open()
}
fn state<R>(ui: Ui, f: impl FnOnce(&Open) -> R) -> Option<R> {
    ui.screens.organization.read(f)
}
fn update(ui: Ui, f: impl FnOnce(&mut Open)) {
    ui.screens.organization.update(f)
}
pub fn back(ui: Ui) -> bool {
    if state(ui, |s| s.saving).unwrap_or(false) {
        return true;
    }
    if state(ui, |s| s.bulk_visibility.is_some()) == Some(true) {
        update(ui, |s| {
            s.dismiss_bulk_visibility();
        });
        return true;
    }
    if matches!(
        placement(ui),
        Placement::Sort(_) | Placement::RenameGroup { .. }
    ) {
        close(ui);
        return true;
    }
    if reordering(ui) {
        reorder_key(ui, Key::Back);
        return true;
    }
    let mut close = false;
    update(ui, |s| close = !s.editor.back());
    dismiss_keyboard();
    if close {
        self::close(ui);
    }
    true
}
fn close(ui: Ui) {
    if state(ui, |s| {
        matches!(
            s.placement,
            Placement::Guide {
                return_to_groups: true,
                ..
            }
        )
    }) == Some(true)
    {
        crate::guide::lists::open_groups(ui);
    }
    ui.screens.organization.close();
    dismiss_keyboard();
}
pub fn key(ui: Ui, action: Action) {
    if state(ui, |s| s.saving).unwrap_or(false) {
        return;
    }
    if let Some((_, focus_confirm)) = state(ui, |s| s.bulk_visibility).flatten() {
        match action {
            Action::MoveUp | Action::MoveDown => {
                update(ui, |s| s.focus_bulk_visibility(action == Action::MoveDown));
            }
            Action::Select => resolve_bulk_visibility(ui, focus_confirm),
            Action::Back => resolve_bulk_visibility(ui, false),
            _ => {}
        }
        return;
    }
    if reordering(ui) {
        if confirming_order(ui) {
            match action {
                Action::MoveUp | Action::MoveDown => update(ui, |s| {
                    let by = if action == Action::MoveUp { -1 } else { 1 };
                    s.editor.leave_choice = step_choice(&LEAVE_CHOICES, s.editor.leave_choice, by);
                }),
                Action::Select => {
                    if let Some(choice) = state(ui, |s| s.editor.leave_choice) {
                        resolve_order(ui, choice);
                    }
                }
                Action::Back => resolve_order(ui, LeaveChoice::Stay),
                _ => {}
            }
        } else if let Some(key) = match action {
            Action::MoveUp => Some(Key::Up),
            Action::MoveDown => Some(Key::Down),
            Action::MoveLeft => Some(Key::Left),
            Action::MoveRight => Some(Key::Right),
            Action::Select => Some(Key::Ok),
            Action::Back => Some(Key::Back),
            _ => None,
        } {
            reorder_key(ui, key);
        }
        return;
    }
    match action {
        Action::MoveUp => update(ui, |s| s.editor.step(-1)),
        Action::MoveDown => update(ui, |s| s.editor.step(1)),
        Action::Back => {
            back(ui);
        }
        Action::Select => {
            if let Some(sort) = state(ui, |s| s.editor.sort_focus).flatten() {
                perform(ui, Edit::Sort(sort));
                return;
            }
            match state(ui, |s| s.editor.mode) {
                Some(Mode::Visibility) => return perform(ui, Edit::Visibility),
                Some(Mode::Blocking) => return perform(ui, Edit::Blocking),
                _ => {}
            }
            if state(ui, |s| s.editor.text.is_some()).unwrap_or(false) {
                commit(ui);
                return;
            }
            if let Some(edit) = state(ui, |s| s.editor.action).flatten() {
                perform(ui, edit)
            } else {
                update(ui, |s| {
                    if let Some(id) = s.editor.focus.clone() {
                        s.editor.open_actions(id)
                    }
                })
            }
        }
        _ => {}
    }
}
fn reordering(ui: Ui) -> bool {
    state(ui, |s| s.editor.reorder.is_some()).unwrap_or(false)
}

fn confirming_order(ui: Ui) -> bool {
    state(ui, |s| {
        s.editor
            .reorder
            .as_ref()
            .is_some_and(|order| order.is_confirming())
    })
    .unwrap_or(false)
}

fn reorder_key(ui: Ui, key: Key) {
    if state(ui, |s| s.dragging) == Some(true) {
        return;
    }
    if confirming_order(ui) && key == Key::Back {
        resolve_order(ui, LeaveChoice::Stay);
        return;
    }
    let mut outcome = Outcome::Ignored;
    update(ui, |s| {
        outcome = s.reorder_key(key, key == Key::Back && ui.drag_editing())
    });
    if matches!(outcome, Outcome::Done(_)) {
        close(ui);
    }
}

fn resolve_order(ui: Ui, choice: LeaveChoice) {
    if choice == LeaveChoice::Save {
        save_order(ui);
        return;
    }
    let mut outcome = Outcome::Ignored;
    update(ui, |s| outcome = s.resolve_order(choice));
    if matches!(outcome, Outcome::Done(_)) {
        close(ui);
    }
}

fn save_order(ui: Ui) {
    if state(ui, |s| s.scope.is_none()) == Some(true) {
        let generation = ui.screens.organization.epoch();
        crate::wiring::navigation::settings_guarded(ui, move || {
            if !still_open(ui, generation) {
                return;
            }
            if !crate::wiring::settings::playlists_by_hand()
                && let Err(error) = crate::runtime()
                    .check_setting_write(iptv_settings::keys::PLAYLIST_SORT.as_str())
            {
                crate::screens::settings_view::notify_setting_error(error);
                return;
            }
            persist_order(ui);
        });
    } else {
        persist_order(ui);
    }
}

fn check_playlist_order_catalog(ui: Ui) -> bool {
    let valid = crate::runtime().playlists().is_ok_and(|rows| {
        untracked(|| {
            state(ui, |s| {
                if let Some(saved) = &s.saved_playlist_positions {
                    // The saved positions can change the tie order of equal names.
                    // The draft can still be retried while its playlists all exist.
                    let saved: HashSet<_> = saved.iter().copied().collect();
                    rows.len() == saved.len() && rows.iter().all(|p| saved.contains(&p.id))
                } else {
                    rows.iter().map(|p| p.id.0.to_string()).eq(s
                        .editor
                        .items
                        .iter()
                        .map(|item| item.id.clone()))
                }
            })
        }) == Some(true)
    });
    if !valid {
        untracked(|| close(ui));
        crate::chrome::toast::notify(tr(
            "The list could not be reloaded. Reopen it and try again.",
        ));
    }
    valid
}

fn persist_order(ui: Ui) {
    let mut draft = None;
    update(ui, |s| draft = s.begin_order_save());
    let Some(draft) = draft else { return };
    if matches!(draft, OrderSave::Unchanged) {
        return close(ui);
    }
    let playlists = matches!(&draft, OrderSave::Playlists { .. });
    if playlists && !check_playlist_order_catalog(ui) {
        return;
    }
    let generation = ui.screens.organization.epoch();
    let owner = crate::wiring::runtime_owner::runtime();
    crate::wiring::tasks::spawn_in_app(async move {
        let mut saved_positions = None;
        let result = match draft {
            OrderSave::Playlists {
                already_saved: true,
                ..
            } => Ok(()),
            OrderSave::Playlists {
                ids,
                already_saved: false,
            } => {
                let result = data::reorder(&owner, ids.clone()).await;
                if result.is_ok() {
                    saved_positions = Some(ids);
                }
                result
            }
            OrderSave::Pending {
                source,
                kind: Kind::Favorites,
                original,
                order,
                ..
            } => owner.save_favorite_order(source, original, order).await,
            OrderSave::Pending {
                source,
                kind,
                original,
                order,
                expected_sort,
                from_custom,
            } => {
                let result = crate::runtime::commands::save_organization_order(
                    &owner,
                    source,
                    kind,
                    original,
                    order,
                    expected_sort,
                    from_custom,
                )
                .await;
                if result.is_ok() {
                    data::organization_changed(&owner, source);
                }
                result
            }
            OrderSave::Unchanged => return,
        };
        if result.is_ok()
            && playlists
            && !crate::wiring::settings::playlists_by_hand()
            && let Err(error) = owner
                .set_setting_and_wait(
                    iptv_settings::keys::PLAYLIST_SORT.as_str(),
                    iptv_settings::Value::choice("manual"),
                )
                .await
        {
            if !still_open(ui, generation) {
                return;
            }
            match error {
                error @ crate::wiring::settings::SaveError::Durability(_) => {
                    crate::screens::settings_view::notify_setting_error(error);
                }
                error => {
                    update(ui, |s| {
                        if let Some(positions) = saved_positions {
                            s.saved_playlist_positions = Some(positions);
                            if let Some(order) = &mut s.editor.reorder {
                                order.mark_saved();
                            }
                        }
                        s.finish_save();
                    });
                    if !check_playlist_order_catalog(ui) {
                        return;
                    }
                    if matches!(error, crate::wiring::settings::SaveError::Storage(_)) {
                        crate::chrome::toast::notify(tr(
                            "Playlist positions saved. Could not switch to Custom order. Try saving again.",
                        ));
                    } else {
                        crate::screens::settings_view::notify_setting_error(error);
                    }
                    return;
                }
            }
        }
        if !still_open(ui, generation) {
            return;
        }
        match result {
            Ok(()) => close(ui),
            Err(error) => {
                log::warn!("iptv organization: {error}");
                update(ui, Open::finish_save);
                if playlists && error.settings_locked() {
                    crate::screens::settings_view::notify_setting_error(
                        crate::wiring::settings::SaveError::ParentalLocked,
                    );
                } else {
                    crate::chrome::toast::notify(tr(
                        "The change could not be saved. Reopen the list and try again.",
                    ));
                }
            }
        }
    });
}

fn perform(ui: Ui, edit: Edit) {
    if matches!(edit, Edit::HideAll | Edit::ShowAll) {
        update(ui, |s| s.request_bulk_visibility(edit));
        return;
    }
    apply_edit(ui, edit);
}

fn apply_edit(ui: Ui, edit: Edit) {
    let generation = ui.screens.organization.epoch();
    let Some(s) = state(ui, |s| (!s.saving).then(|| s.clone())).flatten() else {
        return;
    };
    let Some((source, scope_kind)) = s.scope.clone() else {
        return;
    };
    if let Edit::Sort(sort) = edit {
        if sort == Sort::Manual {
            let placement = match s.placement {
                Placement::Sort(list) => reorder_placement(ui, &scope_kind, list),
                _ => Placement::Screen,
            };
            open_with(
                ui,
                source,
                scope_kind,
                s.editor.focus,
                Mode::Reorder,
                Some(Edit::Sort(Sort::Manual)),
                placement,
            );
        } else {
            store_sort(ui, &s, generation, sort);
        }
        return;
    }
    let Some(item) = s.editor.focused().cloned() else {
        return;
    };
    match edit {
        Edit::Blocking => {
            let kind = block_kind(&scope_kind);
            let blocked = block_state(&s, &item.id);
            if kind == BlockKind::Channel && blocked == Block::Group {
                crate::chrome::toast::notify(tr("This channel is blocked by a group."));
                return;
            }
            update(ui, Open::begin_block);
            set_block(ui, source, kind, item.id, blocked == Block::None);
            return;
        }
        Edit::Channels => {
            let kind = Kind::group_channels(&item.id);
            open(ui, source, kind, None);
            return;
        }
        Edit::SortChannels => {
            open_sort(ui, source, Kind::group_channels(&item.id), None);
            return;
        }
        Edit::Reorder => {
            update(ui, |s| s.editor.start_reorder(|_| true));
            return;
        }
        Edit::SortOptions => {
            update(ui, |s| s.editor.sort_focus = Some(s.editor.sort));
            return;
        }
        _ => {}
    }
    if edit == Edit::Rename {
        update(ui, |s| s.editor.text = Some(item.name));
        return;
    }
    if edit == Edit::Select {
        update(ui, |s| s.editor.toggle_selection());
        return;
    }
    if scope_kind == Kind::Favorites
        && matches!(edit, Edit::Earlier | Edit::Later | Edit::First | Edit::Last)
    {
        let keys = crate::guide::favorites::ordered_keys();
        let Some(at) = keys.iter().position(|key| key == &item.id) else {
            return;
        };
        let to = match edit {
            Edit::Earlier => at.saturating_sub(1),
            Edit::Later => (at + 1).min(keys.len().saturating_sub(1)),
            Edit::First => 0,
            _ => keys.len().saturating_sub(1),
        };
        crate::guide::favorites::reorder(ui, &item.id, to);
        finish(ui, &s, generation, Ok(()));
        return;
    }
    if scope_kind == Kind::Favorites && edit == Edit::ResetOrder {
        let keys = crate::guide::favorites::ordered_keys();
        let ordered = crate::runtime::commands::favorite_provider_order(
            &crate::wiring::runtime_owner::runtime(),
            source,
            keys,
        );
        crate::wiring::tasks::spawn_in_app(async move {
            let ordered = ordered.await;
            if !still_open(ui, generation) {
                return;
            }
            let result = ordered.map(|keys| {
                for (at, key) in keys.iter().enumerate() {
                    crate::guide::favorites::reorder(ui, key, at);
                }
            });
            finish(ui, &s, generation, result);
        });
        return;
    }
    if (s.editor.sort != Sort::Manual || scope_kind == Kind::Groups)
        && matches!(edit, Edit::Earlier | Edit::Later | Edit::First | Edit::Last)
    {
        let original: Vec<_> = s.editor.items.iter().map(|item| item.id.clone()).collect();
        let mut order = original.clone();
        let Some(at) = s.editor.focus_index() else {
            return;
        };
        let to = match edit {
            Edit::Earlier => at.saturating_sub(1),
            Edit::Later => (at + 1).min(order.len() - 1),
            Edit::First => 0,
            _ => order.len() - 1,
        };
        if at == to {
            return;
        }
        let id = order.remove(at);
        order.insert(to, id);
        let result = crate::runtime::commands::save_organization_order(
            &crate::wiring::runtime_owner::runtime(),
            source,
            scope_kind.clone(),
            original,
            order,
            s.editor.sort,
            false,
        );
        update(ui, Open::begin_save);
        crate::wiring::tasks::spawn_in_app(async move {
            finish(ui, &s, generation, result.await);
        });
        return;
    }
    if s.editor.sort != Sort::Manual && edit == Edit::ResetOrder {
        store_sort(ui, &s, generation, Sort::Provider);
        return;
    }
    let Some(edit) = edit.edit() else {
        return;
    };
    update(ui, Open::begin_save);
    let finished = s.clone();
    let result = crate::runtime::commands::edit_organization(
        &crate::wiring::runtime_owner::runtime(),
        source,
        scope_kind,
        s.editor.items,
        item,
        edit,
        s.editor.selected,
    );
    crate::wiring::tasks::spawn_in_app(async move {
        finish(ui, &finished, generation, result.await);
    });
}
fn store_sort(ui: Ui, s: &Open, generation: u64, sort: Sort) {
    let Some((source, kind)) = s.scope.clone() else {
        return;
    };
    let result = crate::runtime::commands::set_organization_sort(
        &crate::wiring::runtime_owner::runtime(),
        source,
        kind,
        sort,
    );
    let expected = s.clone();
    update(ui, Open::begin_save);
    crate::wiring::tasks::spawn_in_app(async move {
        let result = result.await;
        if still_open(ui, generation) && result.is_ok() {
            if matches!(expected.placement, Placement::Sort(_)) {
                close(ui);
                data::organization_changed(&crate::wiring::runtime_owner::runtime(), source);
                return;
            }
            update(ui, |s| s.editor.sort_focus = None);
        }
        finish(ui, &expected, generation, result);
    });
}

fn finish(ui: Ui, s: &Open, generation: u64, result: Result<(), Error>) {
    if let Err(e) = result {
        if still_open(ui, generation) {
            update(ui, Open::finish_save);
        }
        log::warn!("iptv organization: change refused: {e}");
        crate::chrome::toast::notify(tr(
            "The change could not be saved. Reopen the list and try again.",
        ));
        return;
    }
    let Some((source, kind)) = s.scope.clone() else {
        return;
    };
    if matches!(s.placement, Placement::RenameGroup { .. }) {
        if still_open(ui, generation) {
            close(ui);
        }
        data::organization_changed(&crate::wiring::runtime_owner::runtime(), source);
        return;
    }
    let expected = s.clone();
    crate::wiring::tasks::spawn_in_app(async move {
        let loaded = items(source, &kind, None).await;
        if still_open(ui, generation) {
            update(ui, |s| s.finish_edit(&expected, loaded.ok()));
        }
        data::organization_changed(&crate::wiring::runtime_owner::runtime(), source);
    });
}

fn commit(ui: Ui) {
    let generation = ui.screens.organization.epoch();
    let Some(s) = state(ui, |s| (!s.saving).then(|| s.clone())).flatten() else {
        return;
    };
    let Some(item) = s.editor.focused().cloned() else {
        return;
    };
    let Some(text) = s.editor.text.clone() else {
        return;
    };
    if matches!(
        s.placement,
        Placement::RenameGroup {
            restore_name: false
        }
    ) && text.trim().is_empty()
    {
        crate::chrome::toast::notify(tr("Enter a group name."));
        return;
    }
    let Some((source, kind)) = s.scope.clone() else {
        return;
    };
    let result = crate::runtime::commands::rename_organization_item(
        &crate::wiring::runtime_owner::runtime(),
        source,
        kind,
        item.id,
        text,
    );
    dismiss_keyboard();
    update(ui, Open::begin_save);
    crate::wiring::tasks::spawn_in_app(async move {
        finish(ui, &s, generation, result.await);
    });
}

fn text_value(ui: Ui) -> String {
    state(ui, |s| s.editor.text.clone())
        .flatten()
        .unwrap_or_default()
}
fn set_text(ui: Ui, text: &str) {
    update(ui, |s| s.editor.text = Some(text.to_owned()))
}
fn cancel(ui: Ui) {
    if matches!(placement(ui), Placement::RenameGroup { .. }) {
        close(ui);
        return;
    }
    update(ui, |s| s.editor.text = None);
    dismiss_keyboard();
}
fn label(edit: Edit, hidden: bool, blocked: bool, groups: bool) -> &'static str {
    match edit {
        Edit::Visibility => match (hidden, groups) {
            (true, true) => tr("Show group"),
            (false, true) => tr("Hide group"),
            (true, false) => tr("Show channel"),
            (false, false) => tr("Hide channel"),
        },
        Edit::Rename => tr("Rename"),
        Edit::ResetName => tr("Use provider name"),
        Edit::Earlier => tr("Move earlier"),
        Edit::Later => tr("Move later"),
        Edit::First => tr("Move to first"),
        Edit::Last => tr("Move to last"),
        Edit::ResetOrder => tr("Restore provider order"),
        Edit::Select => tr("Select / deselect for bulk visibility"),
        Edit::HideSelected => tr("Hide selected"),
        Edit::ShowSelected => tr("Show selected"),
        Edit::HideAll => tr("Hide all"),
        Edit::ShowAll => tr("Show all"),
        Edit::Blocking => match (blocked, groups) {
            (true, true) => tr("Unblock group"),
            (false, true) => tr("Block group"),
            (true, false) => tr("Unblock channel"),
            (false, false) => tr("Block channel"),
        },
        Edit::Channels => tr("Manage channels in this group"),
        Edit::Reorder => tr("Reorder groups"),
        Edit::SortOptions if groups => tr("Group order"),
        Edit::SortOptions | Edit::SortChannels => tr("Channel order"),
        Edit::Sort(sort) => sort_label(sort),
    }
}

pub(crate) fn sort_label(sort: Sort) -> &'static str {
    match sort {
        Sort::Provider => tr("Playlist order"),
        Sort::Name => tr("Name (A–Z)"),
        Sort::NameDescending => tr("Name (Z–A)"),
        Sort::LastWatched => tr("Last watched (newest first)"),
        Sort::OldestWatched => tr("Last watched (oldest first)"),
        Sort::MostWatched => tr("Watch time (most first)"),
        Sort::LeastWatched => tr("Watch time (least first)"),
        Sort::Manual => tr("Custom order"),
    }
}
/// The screen's title for what it arranges.
fn screen_title(s: &Open) -> &'static str {
    match s.editor.mode {
        Mode::Visibility => return tr("Manage hidden channels…"),
        Mode::Blocking => return tr("Manage blocked channels…"),
        Mode::Reorder if s.scope.is_none() => return tr("Reorder playlists"),
        Mode::Reorder if s.kind() == Some(&Kind::Groups) => return tr("Reorder groups"),
        Mode::Reorder => return tr("Reorder channels"),
        _ => {}
    }
    match s.kind() {
        Some(Kind::Groups) => tr("Organize groups"),
        Some(Kind::Favorites) => tr("Order favorites"),
        Some(Kind::GroupChannels(_)) => tr("Organize group channels"),
        Some(Kind::CustomChannels(_)) => tr("Order custom-group members"),
        Some(Kind::Channels) => tr("Organize channels"),
        None => tr("Reorder playlists"),
    }
}

/// Under the title: what the order, the names and the visibility apply to.
fn screen_caption(s: &Open) -> &'static str {
    match s.editor.mode {
        Mode::Visibility => return tr("OK changes visibility."),
        Mode::Blocking => return tr("OK blocks or unblocks the focused channel."),
        _ => {}
    }
    match s.kind() {
        Some(Kind::Favorites) => {
            tr("Order applies to favorites. Names and visibility apply across this playlist.")
        }
        Some(Kind::GroupChannels(_)) => tr("Order applies only to this group. \
                Names and visibility apply across this playlist."),
        Some(Kind::CustomChannels(_)) => {
            tr("Order applies only to these members. Edit membership in Custom groups.")
        }
        _ => tr("Includes hidden items. OK opens actions; \
                select items for bulk hide/show. Back returns."),
    }
}

/// Row `at` of the items: its name, then whether it is selected and
/// whether it is hidden. A press opens its actions.
fn item_row(ui: Ui, size: Scale, at: Position, focus: Memo<Option<usize>>) -> impl IntoElement {
    let item = move || state(ui, |s| s.editor.rows().get(at.index()).cloned()).flatten();
    let focused = move || focus.get() == Some(at.index());
    let grabbed = move || {
        state(ui, |s| {
            s.editor
                .reorder
                .as_ref()
                .and_then(|order| order.selected())
                .is_some_and(|selected| Some(&selected.id) == item().as_ref().map(|item| &item.id))
        })
        .unwrap_or(false)
    };
    let mark = move || Mark::of(ui.keys_shown() && focused() && !grabbed(), false);
    let visibility = move || state(ui, |s| s.editor.mode == Mode::Visibility) == Some(true);
    let shown = move || item().is_some_and(|item| !item.hidden);
    let status = move || {
        ui.app.settings.get();
        item()
            .map(|i| {
                if i.hidden && state(ui, |s| s.scope.is_none()) == Some(true) {
                    return tr("Hidden").to_owned();
                }
                if reordering(ui) || visibility() {
                    return String::new();
                }
                let blocked = state(ui, |s| block_state(s, &i.id)).unwrap_or_default();
                let blocking = state(ui, |s| s.editor.mode == Mode::Blocking).unwrap_or(false);
                if blocking {
                    let status = match blocked {
                        Block::None => tr("Unblocked"),
                        Block::Channel => tr("Blocked"),
                        Block::Group => tr("Blocked by group"),
                    };
                    return status.to_owned();
                }
                let selected = state(ui, |s| s.editor.selected.contains(&i.id)).unwrap_or(false);
                let shown = if i.hidden {
                    tr("Hidden")
                } else {
                    tr("Visible")
                };
                let shown = if blocked != Block::None {
                    trf("{state} · Blocked", &[("state", shown.into())])
                } else {
                    shown.to_owned()
                };
                if selected {
                    trf("Selected · {state}", &[("state", shown.as_str().into())])
                } else {
                    shown
                }
            })
            .unwrap_or_default()
    };
    let is_selected = move || {
        state(ui, |s| {
            item().is_some_and(|i| s.editor.selected.contains(&i.id))
        })
        .unwrap_or(false)
    };
    list_rows::row(size, ROW, mark)
        .test_id(&format!("organization-row-{}", at.index()))
        .role(move || {
            if visibility() {
                AccessibilityRole::Switch
            } else {
                AccessibilityRole::Row
            }
        })
        .accessibility_state(move || AccessibilityState {
            selected: is_selected() || grabbed(),
            checked: visibility().then(|| {
                if shown() {
                    Checked::True
                } else {
                    Checked::False
                }
            }),
            ..Default::default()
        })
        .flex_row()
        .items_center()
        .on_press(move || {
            ui.touched();
            if let Some(i) = item() {
                let mut edit = None;
                update(ui, |s| edit = s.activate_row(i.id));
                if let Some(edit) = edit {
                    perform(ui, edit);
                }
            }
        })
        .child(
            ui_kit::label(size, move || item().map(|i| i.name).unwrap_or_default())
                .flex_1()
                .text_color(move || mark().text()),
        )
        .child(when(visibility, move || visibility_switch(ui, size, shown)))
        .child(ui_kit::detail(size, status).text_color(move || mark().secondary()))
        .child(when(
            move || {
                reordering(ui)
                    && (ui.drag_editing() || state(ui, |s| s.scope.is_none()) == Some(true))
            },
            move || reorder_drag::handle(size, at.index()),
        ))
        .child(order_outline(ui, grabbed))
}

fn visibility_switch(ui: Ui, size: Scale, shown: impl Fn() -> bool + Copy + 'static) -> View {
    view()
        .relative()
        .w(move || size.px(40.0))
        .h(move || size.px(22.0))
        .rounded(px(20.0))
        .accessibility_elements_hidden(true)
        .bg(move || {
            if shown() {
                rgb(ui.app.theme.with(|theme| theme.accent.light))
            } else {
                crate::tint(0xFFFFFF, 0x40)
            }
        })
        .child(
            view()
                .absolute()
                .top(move || size.px(3.0))
                .left(move || size.px(if shown() { 21.0 } else { 3.0 }))
                .size(move || size.px(16.0))
                .rounded(px(12.0))
                .bg(rgb(0xFFFFFF)),
        )
}

/// The focused item's actions on a card over the dimmed screen; a press
/// beside the card goes back.
fn actions_sheet(ui: Ui, size: Scale) -> impl IntoElement {
    let name = move || {
        state(ui, |s| s.editor.focused().map(|i| i.name.clone()))
            .flatten()
            .unwrap_or_default()
    };
    view()
        .accessibility_elements_hidden(move || {
            state(ui, |screen| {
                screen.editor.text.is_some()
                    || screen.editor.sort_focus.is_some()
                    || screen.bulk_visibility.is_some()
            })
            .unwrap_or(false)
        })
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .child(
            view()
                .absolute()
                .inset_0()
                .bg(crate::tint(0, 160))
                .on_press(move || {
                    back(ui);
                }),
        )
        .child(
            ui_kit::card(ui, size, 620.0)
                .child(ui_kit::title(size, name))
                .child(actions_list(ui, size)),
        )
}

/// The list of actions, scrolled to the one in focus.
fn actions_list(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    effect(move || {
        if let Some(at) = state(ui, |s| {
            s.editor
                .action
                .and_then(|edit| s.editor.actions().iter().position(|action| *action == edit))
        })
        .flatten()
        {
            rows.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
        }
    });
    virtual_list(
        move || state(ui, |s| s.editor.actions().len()).unwrap_or(0),
        move |at| action_row(ui, size, at),
    )
    .element_ref(&rows)
    .estimated_item_size(list_rows::pitch(size, ROW))
    .h(move || size.px(420.0))
}

/// Action `at` for the focused item (Hide reads Show for a hidden one).
fn action_row(ui: Ui, size: Scale, at: Position) -> impl IntoElement {
    let edit = move || state(ui, |s| s.editor.actions().get(at.index()).copied()).flatten();
    ui_kit::action(
        ui,
        size,
        move || {
            ui.app.settings.get();
            state(ui, |s| {
                let item = s.editor.focused()?;
                let blocked = match (block_kind(s.kind()?), block_state(s, &item.id)) {
                    (BlockKind::Channel, Block::Group) if edit() == Some(Edit::Blocking) => {
                        return Some(tr("Blocked by group"));
                    }
                    (_, Block::None) => false,
                    _ => true,
                };
                Some(label(
                    edit()?,
                    item.hidden,
                    blocked,
                    s.kind() == Some(&Kind::Groups),
                ))
            })
            .flatten()
            .unwrap_or("")
        },
        move || state(ui, |s| s.editor.action) == Some(edit()),
        0xFFFFFF,
        move || {
            if let Some(edit) = edit() {
                perform(ui, edit);
            }
        },
    )
}

fn modal(ui: Ui, size: Scale, title: &'static str, contents: impl IntoElement) -> impl IntoElement {
    view()
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .child(
            view()
                .absolute()
                .inset_0()
                .bg(crate::tint(0, 160))
                .on_press(move || {
                    back(ui);
                }),
        )
        .child(
            ui_kit::card(ui, size, 620.0)
                .child(ui_kit::title(size, title))
                .child(contents),
        )
}

fn sort_sheet(ui: Ui, size: Scale) -> impl IntoElement {
    let title = match state(ui, |s| {
        (
            matches!(s.placement, Placement::Sort(_)),
            s.kind() == Some(&Kind::Groups),
        )
    }) {
        Some((true, true)) => tr("Group order"),
        Some((true, false)) => tr("Channel order"),
        _ => tr("Sort order"),
    };
    let scroll = ui_kit::FocusScroll::new();
    let sorts = state(ui, |s| s.editor.sorts()).unwrap_or_default();
    modal(
        ui,
        size,
        title,
        scroll
            .view()
            .max_h(move || px(window().height.0 * 0.67))
            .children(sorts.iter().copied().map(|sort| {
                let focused = move || state(ui, |s| s.editor.sort_focus) == Some(Some(sort));
                scroll.control(
                    ui_kit::action(
                        ui,
                        size,
                        move || {
                            if state(ui, |s| s.editor.sort == sort).unwrap_or(false) {
                                format!("✓ {}", sort_label(sort))
                            } else {
                                sort_label(sort).to_owned()
                            }
                        },
                        focused,
                        0xFFFFFF,
                        move || perform(ui, Edit::Sort(sort)),
                    ),
                    focused,
                )
            })),
    )
}

fn resolve_bulk_visibility(ui: Ui, confirm: bool) {
    let mut requested = None;
    update(ui, |s| requested = s.dismiss_bulk_visibility());
    if let Some(edit) = requested.filter(|_| confirm) {
        apply_edit(ui, edit);
    }
}

fn bulk_visibility_sheet(ui: Ui, size: Scale) -> impl IntoElement {
    let title = if state(ui, |s| s.bulk_visibility)
        .flatten()
        .map(|(edit, _)| edit)
        == Some(Edit::HideAll)
    {
        tr("Hide all")
    } else {
        tr("Show all")
    };
    modal(
        ui,
        size,
        title,
        view().children([false, true].map(|confirm| {
            ui_kit::action(
                ui,
                size,
                if confirm { tr("Confirm") } else { tr("Cancel") },
                move || {
                    state(ui, |s| s.bulk_visibility)
                        .flatten()
                        .is_some_and(|(_, focus)| focus == confirm)
                },
                0xFFFFFF,
                move || resolve_bulk_visibility(ui, confirm),
            )
        })),
    )
}

fn confirm_order_sheet(ui: Ui, size: Scale) -> impl IntoElement {
    modal(
        ui,
        size,
        tr("Save the new order?"),
        view().children(LEAVE_CHOICES.map(|choice| {
            let label = match choice {
                LeaveChoice::Save => tr("Save order"),
                LeaveChoice::Discard => tr("Discard changes"),
                LeaveChoice::Stay => tr("Keep editing"),
            };
            ui_kit::action(
                ui,
                size,
                label,
                move || state(ui, |s| s.editor.leave_choice) == Some(choice),
                0xFFFFFF,
                move || resolve_order(ui, choice),
            )
        })),
    )
}

fn reorder_hint(ui: Ui) -> &'static str {
    if !reordering(ui) {
        ""
    } else if guide_order_open(ui) {
        if ui.drag_editing() && !ui.keys_shown() {
            tr("Drag a handle to move a channel.")
        } else if idiom() == nsh_core::platform::Idiom::Desktop {
            tr("Enter picks up or places · ↑/↓ move · ←/→ jump to either end")
        } else {
            tr("OK picks up or places · ↑/↓ move · ←/→ jump to either end")
        }
    } else if ui.drag_editing() && !ui.keys_shown() {
        tr("Drag the handles to reorder.")
    } else {
        let selected = order_selection(ui).is_some();
        match (idiom() == nsh_core::platform::Idiom::Desktop, selected) {
            (true, true) => tr("Use the arrows to move. Press Enter to place."),
            (true, false) => tr("Press Enter to select a row."),
            (false, true) => tr("Use the arrows to move. Press OK to place."),
            (false, false) => tr("Press OK to select a row."),
        }
    }
}

pub(crate) fn order_outline(ui: Ui, shown: impl Fn() -> bool + 'static) -> View {
    view()
        .absolute()
        .inset_0()
        .rounded(px(crate::theme::RADIUS))
        .border(px(2.0))
        .border_color(move || rgb(ui.app.theme.with(|t| t.accent.light)))
        .opacity(move || if shown() { 1.0_f32 } else { 0.0 })
        .pointer_events(PointerEvents::None)
}

fn order_selection(ui: Ui) -> Option<usize> {
    state(ui, |s| {
        let order = s.editor.reorder.as_ref()?;
        order.selected()?;
        order.focus_index()
    })
    .flatten()
}

pub(crate) fn guide_order_focus(ui: Ui) -> Option<(usize, bool)> {
    state(ui, |s| {
        if !matches!(s.placement, Placement::Guide { .. }) {
            return None;
        }
        let order = s.editor.reorder.as_ref()?;
        Some((order.focus_index()?, order.selected().is_some()))
    })
    .flatten()
}

pub(crate) fn guide_order_scope(ui: Ui) -> Option<(SourceId, ListId)> {
    state(ui, |s| {
        let Placement::Guide { list, .. } = &s.placement else {
            return None;
        };
        Some((s.scope.as_ref()?.0, list.clone()))
    })
    .flatten()
}

pub(crate) fn guide_order_keys(ui: Ui, source: SourceId, list: &ListId) -> HashMap<String, usize> {
    ui.app.data_version.get();
    let catalog = crate::wiring::runtime_owner::schedule();
    ui.guide.list.with(|channels| {
        if &ListId::of(channels.entry(), &*catalog) != list {
            return HashMap::new();
        }
        channels
            .channels()
            .iter()
            .filter_map(|&ch| {
                let target = crate::screens::parental_view::live_target(ui, ch)?;
                (target.source == source.0).then_some((target.id, ch))
            })
            .collect()
    })
}

/// The draft only changes grid rendering. Playback and focus restoration
/// keep using the ordinary guide list and its stable channel keys.
pub(crate) fn guide_order(ui: Ui, keys: &HashMap<String, usize>) -> Option<Vec<usize>> {
    state(ui, |s| {
        if !matches!(s.placement, Placement::Guide { .. }) {
            return None;
        }
        let order = s.editor.reorder.as_ref()?;
        order
            .items()
            .iter()
            .map(|item| keys.get(&item.id).copied())
            .collect()
    })
    .flatten()
}

pub(crate) fn invalidate_changed_guide_order(ui: Ui, keys: &HashMap<String, usize>) {
    let stale = state(ui, |s| {
        matches!(s.placement, Placement::Guide { .. })
            && !s.saving
            && s.editor.reorder.as_ref().is_some_and(|order| {
                order.items().len() != keys.len()
                    || order
                        .items()
                        .iter()
                        .any(|item| !keys.contains_key(&item.id))
            })
    }) == Some(true);
    if stale {
        close(ui);
        crate::chrome::toast::notify(tr(
            "The list could not be reloaded. Reopen it and try again.",
        ));
    }
}

pub(crate) fn guide_order_open(ui: Ui) -> bool {
    matches!(placement(ui), Placement::Guide { .. })
}

pub(crate) fn group_column_order_open(ui: Ui) -> bool {
    placement(ui) == Placement::Groups
}

pub(crate) fn inline_order_open(ui: Ui) -> bool {
    matches!(placement(ui), Placement::Groups | Placement::Guide { .. })
}

pub(crate) fn guide_order_accessible(ui: Ui) -> bool {
    guide_order_open(ui)
        && ui.top_overlay() == Some(crate::chrome::overlays::Overlay::Organization)
        && !contents_hidden(ui)
}

fn placement(ui: Ui) -> Placement {
    state(ui, |s| s.placement.clone()).unwrap_or_default()
}

fn reorder_panel(ui: Ui, size: Scale) -> impl IntoElement {
    let title = move || state(ui, screen_title).unwrap_or("");
    let saves = move || reordering(ui) && !contents_hidden(ui);
    let scope = move || {
        ui.guide_data();
        state(ui, |s| {
            let Placement::Panel(list) = &s.placement else {
                return String::new();
            };
            if s.kind() == Some(&Kind::Groups) {
                return String::new();
            }
            let catalog = crate::wiring::runtime_owner::schedule();
            crate::guide::lists::entry_name(list.find(&*catalog), &*catalog)
        })
        .unwrap_or_default()
    };
    view()
        .test_id("organization-screen")
        .absolute()
        .inset_0()
        .accessibility_elements_hidden(move || {
            crate::chrome::overlays::Overlay::Organization.accessibility_covered(ui)
        })
        .child(
            view()
                .absolute()
                .inset_0()
                .bg(crate::tint(0, 128))
                .on_press(move || {
                    back(ui);
                }),
        )
        .child(
            view()
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .right(px(0.0))
                .w_full()
                .max_w(move || size.px(560.0))
                .pt_safe_area()
                .px(move || size.px(16.0))
                .py(move || size.px(14.0))
                .gap(move || size.px(8.0))
                .bg(move || rgb(ui.app.theme.with(|t| t.background.panel)))
                .child(
                    view()
                        .accessibility_elements_hidden(move || contents_hidden(ui))
                        .gap(px(4.0))
                        .child(ui_kit::title(size, title))
                        .child(when(
                            move || !scope().is_empty(),
                            move || ui_kit::label(size, scope),
                        ))
                        .child(ui_kit::caption(size, move || reorder_hint(ui))),
                )
                .child(editor_rows(ui, size))
                .child(when(
                    move || saves() && ui.drag_editing(),
                    move || {
                        view()
                            .flex_row()
                            .justify_between()
                            .child(ui_kit::action(
                                ui,
                                size,
                                tr("Close"),
                                || false,
                                0xFFFFFF,
                                move || {
                                    back(ui);
                                },
                            ))
                            .child(ui_kit::action(
                                ui,
                                size,
                                tr("Done"),
                                || false,
                                0xFFFFFF,
                                move || save_order(ui),
                            ))
                    },
                )),
        )
        .child(when(
            move || confirming_order(ui),
            move || confirm_order_sheet(ui, size),
        ))
}

fn inline_order(ui: Ui, size: Scale) -> impl IntoElement {
    let groups = move || group_column_order_open(ui);
    view()
        .test_id("organization-screen")
        .absolute()
        .inset_0()
        .pointer_events(PointerEvents::BoxNone)
        .accessibility_elements_hidden(move || {
            crate::chrome::overlays::Overlay::Organization.accessibility_covered(ui)
        })
        .child(
            view()
                .absolute()
                .right(move || size.px(20.0))
                .bottom(move || {
                    px(window().height.0
                        - safe_area().top.0
                        - crate::playback::panels::header_now().0
                        + size.of(12.0))
                })
                .rounded(px(crate::theme::RADIUS))
                .bg(move || rgb(ui.app.theme.with(|t| t.background.panel)))
                .p(size.px(12.0))
                .accessibility_elements_hidden(move || contents_hidden(ui))
                .child(ui_kit::label(size, move || {
                    state(ui, screen_title).unwrap_or("")
                }))
                .max_w(move || px((window().width.0 - size.of(40.0)).max(0.0)))
                .child(ui_kit::caption(size, move || reorder_hint(ui)))
                .child(when(
                    move || {
                        ui.top_overlay() == Some(crate::chrome::overlays::Overlay::Organization)
                            && ui.drag_editing()
                            && reordering(ui)
                            && !contents_hidden(ui)
                            && state(ui, |s| !s.dragging && !s.saving) == Some(true)
                    },
                    move || {
                        view()
                            .flex_row()
                            .justify_end()
                            .gap(move || size.px(10.0))
                            .child(ui_kit::action(
                                ui,
                                size,
                                move || {
                                    if state(ui, |s| s.saved_playlist_positions.is_some())
                                        == Some(true)
                                    {
                                        tr("Close")
                                    } else {
                                        tr("Cancel")
                                    }
                                },
                                || false,
                                0xFFFFFF,
                                move || {
                                    back(ui);
                                },
                            ))
                            .child(ui_kit::action(
                                ui,
                                size,
                                tr("Save"),
                                || false,
                                0xFFFFFF,
                                move || save_order(ui),
                            ))
                    },
                )),
        )
        .child(when(groups, move || {
            view()
                .absolute()
                .top(move || px(safe_area().top.0 + crate::playback::panels::header_now().0))
                .bottom(px(0.0))
                .left(move || px(crate::chrome::rail::width(ui)))
                .w(move || px(crate::playback::panels::groups_width(ui)))
                .px(px(8.0))
                .py(px(6.0))
                .bg(move || rgb(ui.app.theme.with(|t| t.background.window)))
                .child(editor_rows(ui, size))
        }))
        .child(when(
            move || confirming_order(ui),
            move || confirm_order_sheet(ui, size),
        ))
}

pub(crate) mod reorder_drag;

fn editor_rows(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    let drag = reorder_drag::DragList::new(ui, size, rows);
    let focus = computed(move || state(ui, |s| s.editor.focus_index()).flatten());
    effect(move || {
        if let Some(at) = focus.get()
            && !untracked(|| drag.active())
        {
            rows.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
        }
    });
    drag.with_grips(
        view()
            .flex_1()
            .overflow_hidden()
            .accessibility_elements_hidden(move || contents_hidden(ui))
            .child(
                virtual_list(
                    move || state(ui, |s| s.editor.rows().len()).unwrap_or(0),
                    move |at| item_row(ui, size, at, focus),
                )
                .estimated_item_size(list_rows::pitch(size, ROW))
                .element_ref(&rows)
                .on_scroll(move |e: &ScrollEvent| drag.scrolled(*e))
                .flex_1(),
            ),
    )
}

/// The sheet that edits an item's display name.
fn name_sheet(ui: Ui) -> TextSheet {
    TextSheet {
        label: tr("Display name"),
        placeholder: if matches!(
            placement(ui),
            Placement::RenameGroup {
                restore_name: false
            }
        ) {
            tr("Enter text")
        } else {
            tr("Empty restores the provider name")
        },
        url: false,
        masked: false,
        text: text_value,
        set_text,
        commit,
        cancel,
    }
}

fn contents_hidden(ui: Ui) -> bool {
    state(ui, |screen| {
        screen.bulk_visibility.is_some()
            || screen.editor.action.is_some()
            || screen.editor.text.is_some()
            || screen.editor.sort_focus.is_some()
            || screen
                .editor
                .reorder
                .as_ref()
                .is_some_and(|order| order.is_confirming())
    })
    .unwrap_or(false)
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    effect(move || {
        ui.app.library.version.get();
        ui.app.settings.get();
        if untracked(|| state(ui, |s| s.scope.is_none() && !s.saving)) == Some(true) {
            check_playlist_order_catalog(ui);
            return;
        }
        if let Some((source, kind)) = untracked(|| {
            state(ui, |s| {
                s.scope
                    .clone()
                    .filter(|_| !s.saving && s.editor.reorder.is_none())
            })
            .flatten()
        }) {
            let generation = ui.screens.organization.epoch();
            crate::wiring::tasks::spawn_in_app(async move {
                if let Ok(loaded) = items(source, &kind, None).await
                    && still_open(ui, generation)
                {
                    update(ui, |s| s.refresh(loaded));
                }
            });
        }
    });
    when(
        move || is_open(ui),
        move || {
            view()
                .absolute()
                .inset_0()
                .pointer_events(PointerEvents::BoxNone)
                .child(when(
                    move || matches!(placement(ui), Placement::Groups | Placement::Guide { .. }),
                    move || inline_order(ui, size),
                ))
                .child(when(
                    move || matches!(placement(ui), Placement::Panel(_)),
                    move || reorder_panel(ui, size),
                ))
                .child(when(
                    move || matches!(placement(ui), Placement::Sort(_)),
                    move || {
                        view()
                            .absolute()
                            .inset_0()
                            .test_id("organization-sort")
                            .accessibility_elements_hidden(move || {
                                crate::chrome::overlays::Overlay::Organization
                                    .accessibility_covered(ui)
                            })
                            .child(when(
                                move || {
                                    state(ui, |s| s.editor.sort_focus.is_some()).unwrap_or(false)
                                },
                                move || sort_sheet(ui, size),
                            ))
                    },
                ))
                .child(when(
                    move || placement(ui) == Placement::Screen,
                    move || {
                        ui_kit::screen(ui, size)
                            .test_id("organization-screen")
                            .accessibility_elements_hidden(move || {
                                crate::chrome::overlays::Overlay::Organization
                                    .accessibility_covered(ui)
                            })
                            .child(
                                ui_kit::header(
                                    ui,
                                    size,
                                    move || {
                                        state(ui, screen_title).unwrap_or(tr("Organize channels"))
                                    },
                                    |ui| {
                                        back(ui);
                                    },
                                )
                                .accessibility_elements_hidden(move || contents_hidden(ui)),
                            )
                            .child(
                                ui_kit::caption(size, move || {
                                    if reordering(ui) {
                                        reorder_hint(ui)
                                    } else {
                                        state(ui, screen_caption).unwrap_or("")
                                    }
                                })
                                .accessibility_elements_hidden(move || contents_hidden(ui)),
                            )
                            .child(when(
                                move || state(ui, |s| s.editor.items.is_empty()).unwrap_or(true),
                                move || {
                                    ui_kit::body(
                                        size,
                                        tr("This playlist has no items to organize."),
                                    )
                                    .accessibility_elements_hidden(move || contents_hidden(ui))
                                },
                            ))
                            .child(when(
                                move || {
                                    state(ui, |s| s.editor.mode == Mode::Visibility) == Some(true)
                                },
                                move || {
                                    view()
                                        .items_end()
                                        .px(move || size.px(list_rows::INSET))
                                        .child(ui_kit::caption(size, tr("Show in guide")))
                                },
                            ))
                            .child(editor_rows(ui, size))
                            .child(when(
                                move || {
                                    state(ui, |s| s.editor.mode == Mode::Visibility)
                                        .unwrap_or(false)
                                },
                                move || {
                                    view()
                                        .flex_row()
                                        .gap(px(6.0))
                                        .accessibility_elements_hidden(move || contents_hidden(ui))
                                        .child(ui_kit::action(
                                            ui,
                                            size,
                                            tr("Show all"),
                                            || false,
                                            0xFFFFFF,
                                            move || perform(ui, Edit::ShowAll),
                                        ))
                                        .child(ui_kit::action(
                                            ui,
                                            size,
                                            tr("Hide all"),
                                            || false,
                                            0xFFFFFF,
                                            move || perform(ui, Edit::HideAll),
                                        ))
                                },
                            ))
                            .child(when(
                                move || reordering(ui) && ui.drag_editing(),
                                move || {
                                    view()
                                        .items_end()
                                        .accessibility_elements_hidden(move || contents_hidden(ui))
                                        .child(ui_kit::action(
                                            ui,
                                            size,
                                            tr("Done"),
                                            || false,
                                            0xFFFFFF,
                                            move || save_order(ui),
                                        ))
                                },
                            ))
                            .child(when(
                                move || confirming_order(ui),
                                move || confirm_order_sheet(ui, size),
                            ))
                            .child(when(
                                move || state(ui, |s| s.editor.action.is_some()).unwrap_or(false),
                                move || actions_sheet(ui, size),
                            ))
                            .child(when(
                                move || state(ui, |s| s.bulk_visibility.is_some()) == Some(true),
                                move || bulk_visibility_sheet(ui, size),
                            ))
                            .child(when(
                                move || {
                                    state(ui, |s| s.editor.sort_focus.is_some()).unwrap_or(false)
                                },
                                move || sort_sheet(ui, size),
                            ))
                    },
                ))
                .child(when(
                    move || state(ui, |s| s.editor.text.is_some()).unwrap_or(false),
                    move || {
                        entry::text_sheet(ui, name_sheet(ui)).test_id("organization-name-sheet")
                    },
                ))
        },
    )
}
