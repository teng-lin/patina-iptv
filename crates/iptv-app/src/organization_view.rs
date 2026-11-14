//! Channel and provider-group organization, including hidden-item recovery.
use crate::list_rows::{self, Mark, ROW};
use crate::runtime::{Error, model::SourceId};
use crate::scale::Scale;
use crate::wizard_view::entry::{self, TextSheet};
use crate::{Ui, data, ui_kit};
use iptv_core::organization::{ACTIONS, Action as Edit, Editor, Item};
use iptv_nav::Action;
use nsh_core::prelude::*;

pub use iptv_library::OrganizationScope as Kind;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    source: SourceId,
    kind: Kind,
    editor: Editor,
}
async fn items(source: SourceId, kind: &Kind) -> Result<Vec<Item>, Error> {
    crate::runtime::commands::organization_items(
        source,
        kind.clone(),
        crate::favorites::ordered_keys(),
    )
    .await
}

thread_local! { static OPEN_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
fn next_open() -> u64 {
    OPEN_GENERATION.with(|generation| {
        let next = generation.get().wrapping_add(1);
        generation.set(next);
        next
    })
}

pub fn open(ui: Ui, source: SourceId, kind: Kind, focus: Option<String>) {
    let generation = next_open();
    ui.organization.set(Some(Open {
        source,
        kind: kind.clone(),
        editor: Editor::new(Vec::new(), focus.clone()),
    }));
    data::spawn_in_app(async move {
        let loaded = items(source, &kind).await;
        if OPEN_GENERATION.get() != generation || !is_open(ui) {
            return;
        }
        match loaded {
            Ok(items) => update(ui, |state| state.editor = Editor::new(items, focus)),
            Err(error) => {
                log::warn!("iptv organization: {error}");
                close(ui);
            }
        }
    });
}
pub fn is_open(ui: Ui) -> bool {
    ui.organization.with(Option::is_some)
}
fn state<R>(ui: Ui, f: impl FnOnce(&Open) -> R) -> Option<R> {
    ui.organization.with(|s| s.as_ref().map(f))
}
fn update(ui: Ui, f: impl FnOnce(&mut Open)) {
    ui.organization.update(|s| {
        if let Some(s) = s {
            f(s)
        }
    })
}
pub fn back(ui: Ui) -> bool {
    let mut close = false;
    update(ui, |s| close = !s.editor.back());
    dismiss_keyboard();
    if close {
        next_open();
        ui.organization.set(None);
    }
    true
}
fn close(ui: Ui) {
    next_open();
    ui.organization.set(None);
    dismiss_keyboard();
}
pub fn key(ui: Ui, action: Action) {
    match action {
        Action::MoveUp => update(ui, |s| s.editor.step(-1)),
        Action::MoveDown => update(ui, |s| s.editor.step(1)),
        Action::Back => {
            back(ui);
        }
        Action::Select => {
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
fn perform(ui: Ui, edit: Edit) {
    let generation = OPEN_GENERATION.get();
    let Some(s) = state(ui, Clone::clone) else {
        return;
    };
    let Some(item) = s.editor.focused().cloned() else {
        return;
    };
    if edit == Edit::Rename {
        update(ui, |s| s.editor.text = Some(item.name));
        return;
    }
    if edit == Edit::Select {
        update(ui, |s| s.editor.toggle_selection());
        return;
    }
    if s.kind == Kind::Favorites
        && matches!(edit, Edit::Earlier | Edit::Later | Edit::First | Edit::Last)
    {
        let keys = crate::favorites::ordered_keys();
        let Some(at) = keys.iter().position(|key| key == &item.id) else {
            return;
        };
        let to = match edit {
            Edit::Earlier => at.saturating_sub(1),
            Edit::Later => (at + 1).min(keys.len().saturating_sub(1)),
            Edit::First => 0,
            _ => keys.len().saturating_sub(1),
        };
        crate::favorites::reorder(ui, &item.id, to);
        finish(ui, &s, generation, Ok(()));
        return;
    }
    if s.kind == Kind::Favorites && edit == Edit::ResetOrder {
        let keys = crate::favorites::ordered_keys();
        let source = s.source;
        let ordered = crate::runtime::commands::favorite_provider_order(source, keys);
        data::spawn_in_app(async move {
            let ordered = ordered.await;
            if OPEN_GENERATION.get() != generation {
                return;
            }
            let result = ordered.map(|keys| {
                for (at, key) in keys.iter().enumerate() {
                    crate::favorites::reorder(ui, key, at);
                }
            });
            finish(ui, &s, generation, result);
        });
        return;
    }
    let finished = s.clone();
    let result =
        crate::runtime::commands::edit_organization(s.source, s.kind, s.editor, item, edit);
    data::spawn_in_app(async move {
        finish(ui, &finished, generation, result.await);
    });
}
fn finish(ui: Ui, s: &Open, generation: u64, result: Result<(), Error>) {
    if let Err(e) = result {
        log::warn!("iptv organization: change refused: {e}");
        crate::settings_view::notify(
            "The change could not be saved. Reopen the list and try again.",
        );
        return;
    }
    let expected = s.clone();
    data::spawn_in_app(async move {
        if let Ok(items) = items(expected.source, &expected.kind).await
            && OPEN_GENERATION.get() == generation
        {
            update(ui, |s| {
                s.editor.reload(items);
                if s.editor.action == expected.editor.action {
                    s.editor.action = None;
                }
                if s.editor.text == expected.editor.text {
                    s.editor.text = None;
                }
            });
        }
        data::organization::changed(expected.source);
    });
}

fn commit(ui: Ui) {
    let generation = OPEN_GENERATION.get();
    let Some(s) = state(ui, Clone::clone) else {
        return;
    };
    let Some(item) = s.editor.focused().cloned() else {
        return;
    };
    let Some(text) = s.editor.text.clone() else {
        return;
    };
    let target = s.clone();
    let result = crate::runtime::commands::rename_organization_item(
        target.source,
        target.kind,
        item.id,
        text,
    );
    dismiss_keyboard();
    data::spawn_in_app(async move {
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
    update(ui, |s| s.editor.text = None);
    dismiss_keyboard();
}
fn label(edit: Edit, hidden: bool) -> &'static str {
    match edit {
        Edit::Visibility => {
            if hidden {
                "Show item"
            } else {
                "Hide item"
            }
        }
        Edit::Rename => "Rename",
        Edit::ResetName => "Use provider name",
        Edit::Earlier => "Move earlier",
        Edit::Later => "Move later",
        Edit::First => "Move to first",
        Edit::Last => "Move to last",
        Edit::ResetOrder => "Restore provider order",
        Edit::Select => "Select / deselect for bulk visibility",
        Edit::HideSelected => "Hide selected",
        Edit::ShowSelected => "Show selected",
        Edit::HideAll => "Hide all",
        Edit::ShowAll => "Show all",
    }
}
/// The screen's title for what it arranges.
fn screen_title(kind: &Kind) -> &'static str {
    match kind {
        Kind::Groups => "Organize groups",
        Kind::Favorites => "Order favourites",
        Kind::GroupChannels(_) => "Organize group channels",
        Kind::CustomChannels(_) => "Order custom-group members",
        Kind::Channels => "Organize channels",
    }
}

/// Under the title: what the order, the names and the visibility apply to.
fn screen_caption(kind: &Kind) -> &'static str {
    match kind {
        Kind::Favorites => {
            "Order applies to favourites. Names and visibility apply across this playlist."
        }
        Kind::GroupChannels(_) => {
            "Order applies only to this group. \
             Names and visibility apply across this playlist."
        }
        Kind::CustomChannels(_) => {
            "Order applies only to these members. Edit membership in Custom groups."
        }
        _ => {
            "Includes hidden items. OK opens actions; \
             select items for bulk hide/show. Back returns."
        }
    }
}

/// Row `at` of the items: its name, then whether it is selected and
/// whether it is hidden. A press opens its actions.
fn item_row(ui: Ui, size: Scale, at: Position) -> impl IntoElement {
    let item = move || state(ui, |s| s.editor.items.get(at.index()).cloned()).flatten();
    let focused = move || {
        state(ui, |s| {
            s.editor.focus.as_ref() == item().as_ref().map(|i| &i.id)
        })
        .unwrap_or(false)
    };
    let mark = move || Mark::of(ui.keys_shown() && focused(), false);
    let status = move || {
        item()
            .map(|i| {
                let selected = state(ui, |s| s.editor.selected.contains(&i.id)).unwrap_or(false);
                format!(
                    "{}{}",
                    if selected { "Selected · " } else { "" },
                    if i.hidden { "Hidden" } else { "Visible" }
                )
            })
            .unwrap_or_default()
    };
    list_rows::row(size, ROW, mark)
        .test_id(&format!("organization-row-{}", at.index()))
        .flex_row()
        .items_center()
        .on_press(move || {
            ui.touched();
            if let Some(i) = item() {
                update(ui, |s| s.editor.open_actions(i.id));
            }
        })
        .child(
            ui_kit::label(size, move || item().map(|i| i.name).unwrap_or_default())
                .flex_1()
                .text_color(move || mark().text()),
        )
        .child(ui_kit::detail(size, status).text_color(move || mark().secondary()))
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
        if let Some(edit) = state(ui, |s| s.editor.action).flatten()
            && let Some(at) = ACTIONS.iter().position(|a| *a == edit)
        {
            rows.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
        }
    });
    virtual_list(move || ACTIONS.len(), move |at| action_row(ui, size, at))
        .element_ref(&rows)
        .estimated_item_size(list_rows::pitch(size, ROW))
        .h(move || size.px(420.0))
}

/// Action `at` for the focused item (Hide reads Show for a hidden one).
fn action_row(ui: Ui, size: Scale, at: Position) -> impl IntoElement {
    let edit = ACTIONS[at.index()];
    let hidden =
        move || state(ui, |s| s.editor.focused().is_some_and(|i| i.hidden)).unwrap_or(false);
    ui_kit::action(
        ui,
        size,
        move || label(edit, hidden()),
        move || state(ui, |s| s.editor.action) == Some(Some(edit)),
        0xFFFFFF,
        move || perform(ui, edit),
    )
}

/// The sheet that edits an item's display name.
fn name_sheet() -> TextSheet {
    TextSheet {
        label: "Display name",
        placeholder: "Empty restores the provider name",
        url: false,
        masked: false,
        text: text_value,
        set_text,
        commit,
        cancel,
    }
}

pub fn layer(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    effect(move || {
        ui.library.version.get();
        if let Some(s) = untracked(|| state(ui, Clone::clone)) {
            let generation = OPEN_GENERATION.get();
            data::spawn_in_app(async move {
                if let Ok(items) = items(s.source, &s.kind).await
                    && OPEN_GENERATION.get() == generation
                {
                    update(ui, |s| s.editor.reload(items));
                }
            });
        }
    });
    when(
        move || is_open(ui),
        move || {
            let rows = element_ref();
            effect(move || {
                if let Some(at) = state(ui, |s| {
                    s.editor
                        .items
                        .iter()
                        .position(|i| Some(&i.id) == s.editor.focus.as_ref())
                })
                .flatten()
                {
                    rows.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
                }
            });
            ui_kit::screen(ui, size)
                .test_id("organization-screen")
                .child(ui_kit::header(
                    ui,
                    size,
                    move || state(ui, |s| screen_title(&s.kind)).unwrap_or("Organize channels"),
                    close,
                ))
                .child(ui_kit::caption(size, move || {
                    state(ui, |s| screen_caption(&s.kind)).unwrap_or("")
                }))
                .child(when(
                    move || state(ui, |s| s.editor.items.is_empty()).unwrap_or(true),
                    move || ui_kit::body(size, "This playlist has no items to organize."),
                ))
                .child(
                    virtual_list(
                        move || state(ui, |s| s.editor.items.len()).unwrap_or(0),
                        move |at| item_row(ui, size, at),
                    )
                    .estimated_item_size(list_rows::pitch(size, ROW))
                    .element_ref(&rows)
                    .flex_1(),
                )
                .child(when(
                    move || state(ui, |s| s.editor.action.is_some()).unwrap_or(false),
                    move || actions_sheet(ui, size),
                ))
                .child(when(
                    move || state(ui, |s| s.editor.text.is_some()).unwrap_or(false),
                    move || entry::text_sheet(ui, name_sheet()),
                ))
        },
    )
}
