//! Custom groups and filtered bulk membership editing. Provider membership
//! remains imported data; moves are offered only between custom groups.
use crate::i18n::tr;

use crate::runtime::Error;
use crate::{
    Ui, data,
    scale::Scale,
    ui_kit,
    wizard_view::entry::{self, Sheet},
};
use iptv_core::membership::Membership;
use iptv_library::{ChannelScope, CustomGroup, CustomGroupId, SourceId};
use iptv_nav::Action;
use iptv_providers::ContentId;
use nsh_core::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Page {
    Groups,
    Detail(CustomGroupId),
    Members,
    Destination(bool),
    Delete(CustomGroupId),
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Text {
    Create,
    Rename(CustomGroupId),
    Filter,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    source: SourceId,
    groups: Vec<CustomGroup>,
    page: Page,
    scope: ChannelScope,
    members: Membership,
    /// Toolbar or menu row; None means a channel identity has focus.
    control: Option<usize>,
    editing: Option<(Text, String)>,
    status: String,
}
impl Open {
    fn reload_groups(&mut self, groups: Vec<CustomGroup>) {
        let offset = match self.page {
            Page::Groups => Some(2),
            Page::Destination(_) => Some(1),
            _ => None,
        };
        let focused = offset
            .and_then(|offset| self.control.and_then(|at| at.checked_sub(offset)))
            .and_then(|at| {
                if matches!(self.page, Page::Destination(_)) {
                    self.groups
                        .iter()
                        .filter(|g| Some(g.id) != origin(self))
                        .nth(at)
                } else {
                    self.groups.get(at)
                }
            })
            .map(|g| g.id);
        self.groups = groups;
        if let (Some(id), Some(offset)) = (focused, offset) {
            let at = if matches!(self.page, Page::Destination(_)) {
                self.groups
                    .iter()
                    .filter(|g| Some(g.id) != origin(self))
                    .position(|g| g.id == id)
            } else {
                self.groups.iter().position(|g| g.id == id)
            };
            self.control = Some(at.map_or(0, |at| at + offset));
        }
        if matches!(self.page,Page::Detail(g)|Page::Delete(g) if !self.groups.iter().any(|r|r.id==g))
        {
            self.page = Page::Groups;
            self.control = Some(0);
        }
    }
}
fn read<R>(ui: Ui, f: impl FnOnce(&Open) -> R) -> Option<R> {
    ui.custom_groups.with(|s| s.as_ref().map(f))
}
fn update(ui: Ui, f: impl FnOnce(&mut Open)) {
    ui.custom_groups.update(|s| {
        if let Some(s) = s {
            f(s)
        }
    });
}
pub fn is_open(ui: Ui) -> bool {
    ui.custom_groups.with(Option::is_some)
}
thread_local! { static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
fn advance() {
    GENERATION.with(|v| v.set(v.get().wrapping_add(1)));
}
fn request<R: Send + 'static>(
    ui: Ui,
    operation: impl std::future::Future<Output = Result<R, Error>> + 'static,
    done: impl FnOnce(Ui, Result<R, Error>) + 'static,
) {
    let generation = GENERATION.get();
    let result = operation;
    data::spawn_in_app(async move {
        let result = result.await;
        if GENERATION.get() == generation && is_open(ui) {
            done(ui, result);
        }
    });
}
fn mutate<R: Send + 'static>(
    ui: Ui,
    source: SourceId,
    operation: impl std::future::Future<Output = Result<R, Error>> + 'static,
    done: impl FnOnce(Ui, Result<R, Error>) + 'static,
) {
    let generation = GENERATION.get();
    let result = operation;
    data::spawn_in_app(async move {
        let result = result.await;
        // A committed edit affects the guide even if its editor was closed.
        if result.is_ok() {
            data::organization::changed(source);
        }
        if GENERATION.get() == generation && is_open(ui) {
            done(ui, result);
        }
    });
}
pub fn open(ui: Ui, source: SourceId, browse: bool) {
    advance();
    ui.custom_groups.set(Some(Open {
        source,
        groups: Vec::new(),
        page: Page::Groups,
        scope: ChannelScope::All,
        members: Membership::default(),
        control: Some(0),
        editing: None,
        status: String::new(),
    }));
    request(
        ui,
        crate::runtime::commands::custom_groups(source),
        move |ui, result| match result {
            Ok(groups) => {
                update(ui, |s| s.reload_groups(groups));
                if browse {
                    show_members(ui, ChannelScope::All);
                }
            }
            Err(error) => {
                log::warn!("iptv custom groups: {error}");
                close(ui);
            }
        },
    );
}
fn close(ui: Ui) {
    advance();
    dismiss_keyboard();
    ui.custom_groups.set(None);
}
fn origin(s: &Open) -> Option<CustomGroupId> {
    if let ChannelScope::Custom(g) = s.scope {
        Some(g)
    } else {
        None
    }
}
fn reload(ui: Ui) {
    let Some((source, scope)) = read(ui, |s| (s.source, s.scope.clone())) else {
        return;
    };
    request(
        ui,
        crate::runtime::commands::group_members(source, scope),
        |ui, result| {
            if let Ok((groups, rows)) = result {
                update(ui, |s| {
                    s.reload_groups(groups);
                    s.members.reload(rows);
                });
            } else {
                update(ui, |s| {
                    s.status = "The list could not be reloaded. Reopen it and try again.".into()
                });
            }
        },
    );
}
fn report(ui: Ui, result: Result<(), Error>) {
    match result {
        Ok(()) => {
            reload(ui);
        }
        Err(_) => update(ui, |s| {
            s.status = "The change could not be saved. Reopen the list and try again.".into()
        }),
    }
}
fn show_members(ui: Ui, scope: ChannelScope) {
    advance();
    let Some(source) = read(ui, |s| s.source) else {
        return;
    };
    let loaded_scope = scope.clone();
    request(
        ui,
        crate::runtime::commands::members(source, loaded_scope),
        move |ui, result| match result {
            Ok(rows) => update(ui, |s| {
                s.scope = scope;
                s.members = Membership::new(rows);
                s.page = Page::Members;
                s.control = Some(0);
                s.status.clear();
            }),
            Err(error) => report(ui, Err(error)),
        },
    );
}
pub fn back(ui: Ui) -> bool {
    advance();
    let Some(s) = read(ui, Clone::clone) else {
        return false;
    };
    if s.editing.is_some() {
        cancel_text(ui);
        return true;
    }
    match s.page {
        Page::Groups => close(ui),
        Page::Destination(moving) => update(ui, |s| {
            s.page = Page::Members;
            s.control = Some(if moving { 4 } else { 3 });
        }),
        Page::Delete(g) => update(ui, |s| {
            s.page = Page::Detail(g);
            s.control = Some(3);
        }),
        Page::Members => update(ui, |s| {
            s.page = origin(s).map_or(Page::Groups, Page::Detail);
            s.control = Some(if origin(s).is_some() { 0 } else { 1 });
        }),
        Page::Detail(g) => update(ui, |s| {
            s.page = Page::Groups;
            s.control = Some(
                s.groups
                    .iter()
                    .position(|r| r.id == g)
                    .map_or(0, |at| at + 2),
            );
        }),
    }
    true
}
fn begin_text(ui: Ui, kind: Text) {
    update(ui, |s| {
        let value = match kind {
            Text::Filter => s.members.query.clone(),
            Text::Rename(g) => s
                .groups
                .iter()
                .find(|r| r.id == g)
                .map(|g| g.name.clone())
                .unwrap_or_default(),
            Text::Create => String::new(),
        };
        s.status.clear();
        s.editing = Some((kind, value));
    });
}
fn cancel_text(ui: Ui) {
    update(ui, |s| s.editing = None);
    dismiss_keyboard();
}
fn text(ui: Ui) -> String {
    read(ui, |s| {
        s.editing.as_ref().map(|e| e.1.clone()).unwrap_or_default()
    })
    .unwrap_or_default()
}
fn set_text(ui: Ui, value: &str) {
    update(ui, |s| {
        if let Some(e) = s.editing.as_mut() {
            e.1 = value.into();
        }
    });
}
fn commit_text(ui: Ui) {
    let Some(s) = read(ui, Clone::clone) else {
        return;
    };
    let Some((kind, value)) = s.editing else {
        return;
    };
    if kind == Text::Filter {
        update(ui, |s| s.members.filter(value));
        cancel_text(ui);
        return;
    }
    let name = value.trim();
    if name.is_empty() {
        update(ui, |s| s.status = "Enter a group name.".into());
        return;
    }
    let name = name.to_owned();
    mutate(
        ui,
        s.source,
        crate::runtime::commands::save_custom_group(
            s.source,
            match kind {
                Text::Rename(group) => Some(group),
                _ => None,
            },
            name,
        ),
        |ui, result| {
            if result.is_ok() {
                cancel_text(ui);
                update(ui, |s| s.status = "Group saved.".into());
            }
            report(ui, result);
        },
    );
}

fn labels(s: &Open) -> Vec<String> {
    match s.page {
        Page::Groups => ["New group".into(), "Browse all channels".into()]
            .into_iter()
            .chain(
                s.groups
                    .iter()
                    .map(|g| format!("{} · {} channels", g.name, g.channel_count)),
            )
            .collect(),
        Page::Detail(_) => [
            "Edit members",
            "Rename group",
            "Reorder members",
            "Delete group",
            tr("Back"),
        ]
        .map(str::to_owned)
        .to_vec(),
        Page::Members => {
            let mut labels = vec![
                format!(
                    "Search · {}",
                    if s.members.query.is_empty() {
                        "all channels"
                    } else {
                        &s.members.query
                    }
                ),
                "Select filtered".into(),
                "Clear selection".into(),
                "Copy selected".into(),
            ];
            if origin(s).is_some() {
                labels.extend(["Move selected".into(), "Remove selected".into()]);
            }
            labels
        }
        Page::Destination(_) => std::iter::once("New group".into())
            .chain(
                s.groups
                    .iter()
                    .filter(|g| Some(g.id) != origin(s))
                    .map(|g| format!("{} · {} channels", g.name, g.channel_count)),
            )
            .collect(),
        Page::Delete(_) => vec![tr("Cancel").into(), "Delete group and memberships".into()],
    }
}
fn act(ui: Ui, index: usize) {
    let Some(s) = read(ui, Clone::clone) else {
        return;
    };
    match s.page {
        Page::Groups => match index {
            0 => begin_text(ui, Text::Create),
            1 => show_members(ui, ChannelScope::All),
            _ => {
                if let Some(g) = s.groups.get(index - 2) {
                    update(ui, |s| {
                        s.page = Page::Detail(g.id);
                        s.control = Some(0);
                    });
                }
            }
        },
        Page::Detail(g) => match index {
            0 => show_members(ui, ChannelScope::Custom(g)),
            1 => begin_text(ui, Text::Rename(g)),
            2 => crate::organization_view::open(
                ui,
                s.source,
                crate::organization_view::Kind::CustomChannels(g),
                None,
            ),
            3 => update(ui, |s| {
                s.page = Page::Delete(g);
                s.control = Some(0);
            }),
            _ => {
                back(ui);
            }
        },
        Page::Members => match index {
            0 => begin_text(ui, Text::Filter),
            1 => update(ui, |s| s.members.select_filtered()),
            2 => update(ui, |s| s.members.selected.clear()),
            3 | 4 if !s.members.selected.is_empty() => update(ui, |s| {
                s.page = Page::Destination(index == 4);
                s.control = Some(0);
            }),
            5 if origin(&s).is_some() => {
                let ids: Vec<_> = s
                    .members
                    .selection()
                    .into_iter()
                    .map(ContentId::from_stored)
                    .collect();
                let origin = origin(&s).unwrap();
                mutate(
                    ui,
                    s.source,
                    crate::runtime::commands::remove_custom_members(origin, ids),
                    report,
                );
            }
            _ => {}
        },
        Page::Destination(moving) => {
            if index == 0 {
                begin_text(ui, Text::Create);
                return;
            }
            let Some(target) = s
                .groups
                .iter()
                .filter(|g| Some(g.id) != origin(&s))
                .nth(index - 1)
            else {
                return;
            };
            let ids: Vec<_> = s
                .members
                .selection()
                .into_iter()
                .map(ContentId::from_stored)
                .collect();
            let target = target.id;
            mutate(
                ui,
                s.source,
                crate::runtime::commands::transfer_custom_members(
                    s.source,
                    origin(&s),
                    target,
                    ids,
                    moving,
                ),
                move |ui, result| match result {
                    Ok(count) => {
                        update(ui, |s| {
                            s.members.selected.clear();
                            s.page = Page::Members;
                            s.control = Some(3);
                            s.status = format!(
                                "{count} channels {}.",
                                if moving { "moved" } else { "copied" }
                            );
                        });
                        report(ui, Ok(()));
                    }
                    Err(e) => report(ui, Err(e)),
                },
            );
        }
        Page::Delete(g) => {
            if index == 0 {
                back(ui);
            } else {
                mutate(
                    ui,
                    s.source,
                    crate::runtime::commands::delete_custom_group(g),
                    |ui, result| {
                        if result.is_ok() {
                            update(ui, |s| {
                                s.page = Page::Groups;
                                s.control = Some(0);
                            });
                        }
                        report(ui, result);
                    },
                );
            }
        }
    }
}
pub fn key(ui: Ui, action: Action) {
    let Some((editing, control, focus)) = read(ui, |s| {
        (s.editing.is_some(), s.control, s.members.focus.clone())
    }) else {
        return;
    };
    if action == Action::Back {
        back(ui);
        return;
    }
    if editing {
        if action == Action::Select {
            commit_text(ui);
        }
        return;
    }
    match action {
        Action::Select => {
            if let Some(index) = control {
                act(ui, index);
            } else if let Some(id) = focus {
                update(ui, |s| s.members.toggle(&id));
            }
        }
        Action::MoveUp | Action::MoveDown => {
            let by = if action == Action::MoveDown { 1 } else { -1 };
            let count = read(ui, |s| labels(s).len()).unwrap_or(0);
            update(ui, |s| {
                if let Some(at) = s.control {
                    if by > 0
                        && at + 1 >= count
                        && s.page == Page::Members
                        && s.members.visible_len() > 0
                    {
                        s.control = None;
                    } else {
                        s.control = Some(
                            (at as isize + by).clamp(0, count.saturating_sub(1) as isize) as usize,
                        );
                    }
                } else if by < 0
                    && s.members
                        .visible_at(0)
                        .is_some_and(|r| Some(&r.id) == s.members.focus.as_ref())
                {
                    s.control = Some(count.saturating_sub(1));
                } else {
                    s.members.step(by);
                }
            });
        }
        _ => {}
    }
}
fn entry_sheet(ui: Ui) -> impl IntoElement {
    use std::rc::Rc;
    let filter = read(ui, |s| {
        s.editing.as_ref().is_some_and(|e| e.0 == Text::Filter)
    })
    .unwrap_or(false);
    entry::panel(
        ui,
        Sheet {
            label: if filter {
                "Find channels"
            } else {
                "Group name"
            }
            .into(),
            placeholder: "Enter text",
            keyboard: KeyboardType::Default,
            masked: false,
            current: Rc::new(move || text(ui)),
            note: Rc::new(move || read(ui, |s| s.status.clone()).unwrap_or_default()),
            set: Rc::new(move |value| set_text(ui, value)),
            done: commit_text,
            cancel: cancel_text,
        },
    )
}
/// Under the screen's title: what copying and moving do.
const CAPTION: &str = "Copy adds membership. Move transfers between custom groups. \
                       Channels and provider groups are retained.";

/// The line over the lists: the selection on a members page, the question
/// on the delete page, else how many groups there are.
fn summary(s: &Open) -> String {
    match s.page {
        Page::Members | Page::Destination(_) => format!(
            "{} selected · {} matching · {} total",
            s.members.selected.len(),
            s.members.visible_len(),
            s.members.rows.len()
        ),
        Page::Delete(_) => "Delete this group? Channels are kept.".into(),
        _ => format!("{} custom groups", s.groups.len()),
    }
}

/// Whether the page shows a group's members under its controls.
fn on_members_page(ui: Ui) -> bool {
    read(ui, |s| s.page == Page::Members).unwrap_or(false)
}

/// Row `at` of the page's controls (its actions and the groups).
fn control_row(ui: Ui, size: Scale, at: Position) -> impl IntoElement {
    let label = move || {
        read(ui, |s| {
            labels(s).get(at.index()).cloned().unwrap_or_default()
        })
        .unwrap_or_default()
    };
    let focused = move || read(ui, |s| s.control == Some(at.index())).unwrap_or(false);
    ui_kit::action(ui, size, label, focused, 0xFFFFFF, move || {
        act(ui, at.index())
    })
    .test_id(&format!("custom-control-{}", at.index()))
}

/// Row `at` of the members shown: its name, checked while selected. A
/// press focuses the member and toggles its selection.
fn member_row(ui: Ui, size: Scale, at: Position) -> impl IntoElement {
    let item = move || read(ui, |s| s.members.visible_at(at.index()).cloned()).flatten();
    let label = move || {
        item()
            .map(|r| {
                let selected = read(ui, |s| s.members.selected.contains(&r.id)).unwrap_or(false);
                format!("{}{}", if selected { "✓ " } else { "" }, r.name)
            })
            .unwrap_or_default()
    };
    let focused = move || {
        item().is_some_and(|r| {
            read(ui, |s| {
                s.control.is_none() && s.members.focus.as_ref() == Some(&r.id)
            })
            .unwrap_or(false)
        })
    };
    let toggle = move || {
        if let Some(row) = item() {
            update(ui, |s| {
                s.control = None;
                s.members.focus = Some(row.id.clone());
                s.members.toggle(&row.id);
            });
        }
    };
    ui_kit::action(ui, size, label, focused, 0xFFFFFF, toggle)
        .test_id(&format!("custom-member-{}", at.index()))
}

pub fn layer(ui: Ui) -> impl IntoElement {
    effect(move || {
        ui.library.version.get();
        untracked(|| reload(ui));
    });
    let size = Scale::new();
    when(
        move || is_open(ui),
        move || {
            let controls = element_ref();
            let members = element_ref();
            effect(move || {
                if let Some((control, member)) = read(ui, |s| {
                    (
                        s.control,
                        s.members
                            .visible()
                            .iter()
                            .position(|r| Some(&r.id) == s.members.focus.as_ref()),
                    )
                }) {
                    if let Some(at) = control {
                        controls.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
                    } else if let Some(at) = member {
                        members.scroll_to_index(at as u32, percentage(0.5), px(0.0), false);
                    }
                }
            });
            ui_kit::screen(ui, size)
                .test_id("custom-groups-screen")
                .child(ui_kit::header(ui, size, "Custom groups", close))
                .child(ui_kit::caption(size, CAPTION))
                .child(ui_kit::body(size, move || {
                    read(ui, |s| s.status.clone()).unwrap_or_default()
                }))
                .child(ui_kit::heading(size, move || {
                    read(ui, summary).unwrap_or_default()
                }))
                .child(
                    virtual_list(
                        move || read(ui, |s| labels(s).len()).unwrap_or(0),
                        move |at| control_row(ui, size, at),
                    )
                    .element_ref(&controls)
                    .estimated_item_size(crate::list_rows::pitch(size, crate::list_rows::ROW))
                    .h(move || size.px(if on_members_page(ui) { 260.0 } else { 480.0 })),
                )
                .child(when(
                    move || on_members_page(ui),
                    move || {
                        virtual_list(
                            move || read(ui, |s| s.members.visible_len()).unwrap_or(0),
                            move |at| member_row(ui, size, at),
                        )
                        .element_ref(&members)
                        .estimated_item_size(crate::list_rows::pitch(size, crate::list_rows::ROW))
                        .flex_1()
                    },
                ))
                .child(when(
                    move || read(ui, |s| s.editing.is_some()).unwrap_or(false),
                    move || entry_sheet(ui),
                ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn group(id: i64) -> CustomGroup {
        CustomGroup {
            id: CustomGroupId(id),
            source_id: SourceId(1),
            name: "Same name".into(),
            channel_count: 0,
        }
    }
    fn screen() -> Open {
        Open {
            source: SourceId(1),
            groups: vec![group(1), group(2), group(3)],
            page: Page::Groups,
            scope: ChannelScope::All,
            members: Membership::default(),
            control: Some(3),
            editing: None,
            status: String::new(),
        }
    }
    #[test]
    fn duplicate_named_group_focus_tracks_identity_through_refresh() {
        let mut s = screen();
        s.reload_groups(vec![group(2), group(3), group(1)]);
        assert_eq!(s.control, Some(2));
        s.reload_groups(vec![group(3)]);
        assert_eq!(s.control, Some(0));
        s.page = Page::Detail(CustomGroupId(3));
        s.reload_groups(vec![]);
        assert_eq!(s.page, Page::Groups);
        assert_eq!(s.control, Some(0));
    }
    #[test]
    fn destination_excludes_source_and_retains_focused_target_identity() {
        let mut s = screen();
        s.scope = ChannelScope::Custom(CustomGroupId(1));
        s.page = Page::Destination(true);
        s.control = Some(2);
        s.reload_groups(vec![group(3), group(1), group(2)]);
        assert_eq!(s.control, Some(1));
        assert_eq!(labels(&s).len(), 3);
        s.page = Page::Members;
        assert!(labels(&s).iter().any(|l| l == "Move selected"));
        s.scope = ChannelScope::All;
        assert!(!labels(&s).iter().any(|l| l == "Move selected"));
        s.page = Page::Delete(CustomGroupId(1));
        assert_eq!(labels(&s)[0], "Cancel");
    }
}
