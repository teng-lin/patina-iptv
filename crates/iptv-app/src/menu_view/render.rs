//! Menu panels and the rendered data of their recycled rows.
use super::*;

pub fn layer(ui: Ui) -> impl IntoElement {
    bind_live_rows(ui);
    bind_playlist_rows(ui);
    when(
        move || !playlist_popup_open(ui),
        move || menu_layer(ui, crate::overlays::Overlay::Menu),
    )
}

pub(crate) fn playlist_layer(ui: Ui) -> impl IntoElement {
    when(
        move || playlist_popup_open(ui),
        move || menu_layer(ui, crate::overlays::Overlay::PlaylistMenu),
    )
}

fn menu_layer(ui: Ui, overlay: crate::overlays::Overlay) -> impl IntoElement {
    let open = move || ui.screens.menu.with(Option::is_some);
    let fixed_heading = move || {
        ui.screens.menu.with(|menu| {
            let menu = menu.as_ref()?;
            if menu.rows.first() == Some(&Row::Heading(Heading::PlaylistOrder)) {
                return None;
            }
            match menu.target {
                Target::Channel { .. } => Some(Heading::Channel),
                Target::Group => Some(Heading::Group),
                Target::Program { .. } => None,
            }
        })
    };
    let count = move || {
        ui.screens
            .menu
            .with(|m| m.as_ref().map_or(0, |m| m.rows.len()))
    };
    let list = element_ref();
    let size = Scale::new();
    let focus = computed(move || ui.screens.menu.with(|m| m.as_ref().map(|m| m.focus)));
    effect(move || {
        if let Some(i) = focus.get() {
            list.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    let playlists = computed(move || {
        ui.app.library.version.get();
        if ui
            .screens
            .menu
            .with(|menu| menu.as_ref().is_some_and(is_playlist_menu))
        {
            crate::runtime().playlists().unwrap_or_default()
        } else {
            Vec::new()
        }
    });
    let playlist_by_id = move |id: SourceId| {
        playlists.with(|playlists| {
            playlists
                .iter()
                .find(|playlist| playlist.id == id)
                .map(|playlist| (playlist.name.clone(), playlist.policy.enabled))
        })
    };
    let menu_opacity = move || if open() { 1.0_f32 } else { 0.0 };
    view()
        .absolute()
        .inset_0()
        .flex_row()
        // The description popover is in it and shows without the menu.
        .test_id("menu-layer")
        .accessibility_elements_hidden(move || {
            (!open() && !ui.screens.description.with(Option::is_some))
                || if overlay == crate::overlays::Overlay::Menu {
                    crate::overlays::Overlay::Description.accessibility_covered(ui)
                } else {
                    overlay.accessibility_covered(ui)
                }
        })
        .opacity(move || {
            if open() || ui.screens.description.with(Option::is_some) {
                1.0_f32
            } else {
                0.0
            }
        })
        .pointer_events(move || {
            if open() {
                PointerEvents::Auto
            } else {
                PointerEvents::None
            }
        })
        .child(
            view()
                .flex_1()
                .bg(tint(0x000000, 0x80))
                .accessibility_elements_hidden(move || overlay.accessibility_hidden(ui))
                .opacity(menu_opacity)
                .on_press(move || {
                    ui.touched();
                    close(ui);
                }),
        )
        .child(
            view()
                .test_id("menu-panel")
                .w(move || px(size.whole(PANEL_W).min(window().width.0)))
                .accessibility_elements_hidden(move || overlay.accessibility_hidden(ui))
                .opacity(menu_opacity)
                .pt_safe_area()
                .px(px(12.0))
                .py(px(16.0))
                .bg(move || rgb(ui.app.theme.with(|t| t.background.panel)))
                .child(when(
                    move || fixed_heading().is_some(),
                    move || {
                        view()
                            .flex_shrink(0.0)
                            .px(move || size.px(list_rows::INSET))
                            .py(move || size.px(12.0))
                            .bg(move || rgb(ui.app.theme.with(|theme| theme.background.alt)))
                            .child(
                                crate::ui_kit::title(size, move || {
                                    fixed_heading()
                                        .map_or_else(String::new, |h| heading_text(ui, h))
                                })
                                .role(AccessibilityRole::Header)
                                .number_of_lines(1),
                            )
                    },
                ))
                .child(
                    virtual_list(count, move |at| {
                        menu_row(ui, size, focus, playlist_by_id, at)
                    })
                    .keys(move || {
                        ui.screens.menu.with(|menu| {
                            menu.iter()
                                .flat_map(|menu| &menu.rows)
                                .map(|row| match *row {
                                    Row::Action { item, .. } => Ok(item),
                                    Row::Heading(heading) => Err(heading),
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .estimated_item_size(list_rows::pitch(size, ROW))
                    .element_ref(&list)
                    .flex_1(),
                ),
        )
        .child(description(ui))
}

fn menu_row(
    ui: Ui,
    size: Scale,
    focus: Memo<Option<usize>>,
    playlist_by_id: impl Fn(SourceId) -> Option<(String, bool)> + Copy + 'static,
    at: Position,
) -> View {
    let row = move || {
        ui.screens
            .menu
            .with(|m| m.as_ref().and_then(|m| m.rows.get(at.index()).copied()))
    };
    let focused = move || ui.keys_shown() && focus.get() == Some(at.index());
    let mark = move || Mark::of(focused(), false);
    let heading = move || matches!(row(), Some(Row::Heading(_)));
    let playlist = move || {
        let Some(Row::Action {
            item: MenuItem::TogglePlaylist(id),
            ..
        }) = row()
        else {
            return None;
        };
        playlist_by_id(id)
    };
    let title = move || row_title(ui, row(), playlist);
    let subtitle = move || {
        let Some(Row::Action { item, .. }) = row() else {
            return String::new();
        };
        action_detail(ui, item)
    };
    let accessible_label = move || {
        let subtitle = subtitle();
        if subtitle.is_empty() {
            title()
        } else {
            format!("{} · {}", title(), subtitle)
        }
    };
    let wraps = move || {
        matches!(
            row(),
            Some(Row::Action {
                item: MenuItem::SortGroups
                    | MenuItem::SortChannels
                    | MenuItem::ShowFavoritesOnly
                    | MenuItem::GroupExternalPlayer
                    | MenuItem::GroupEpgOffset,
                ..
            })
        )
    };
    let name = list_rows::name(size, title)
        .number_of_lines(move || if wraps() { 0_u32 } else { 1 })
        .min_w(px(0.0))
        // Headings in the lists' heading size.
        .text_size(move || size.text_px(if heading() { SECTION } else { LIST_ITEM }))
        .text_color(move || match row() {
            Some(Row::Action { enabled: true, .. }) => mark().text(),
            Some(Row::Heading(_)) => mark().secondary(),
            _ => mark().disabled(),
        });
    list_rows::row_minimum(
        size,
        move || {
            if subtitle().is_empty() {
                ROW
            } else {
                list_rows::ROW_TWO
            }
        },
        mark,
    )
    .justify_center()
    .py(move || size.px(if wraps() { 5.0 } else { 0.0 }))
    .role(move || match row() {
        Some(Row::Heading(_)) => AccessibilityRole::Header,
        Some(Row::Action {
            item: MenuItem::TogglePlaylist(_),
            ..
        }) => AccessibilityRole::Checkbox,
        _ => AccessibilityRole::Menuitem,
    })
    .label(accessible_label)
    .disabled(move || matches!(row(), Some(Row::Action { enabled: false, .. })))
    .accessibility_state(move || AccessibilityState {
        selected: focused(),
        checked: playlist().map(|(_, on)| if on { Checked::True } else { Checked::False }),
        ..Default::default()
    })
    .child(
        view()
            .flex_row()
            .items_center()
            .child(when(
                move || playlist().is_some(),
                move || {
                    crate::ui_kit::playlist_checkbox(
                        ui,
                        size,
                        || true,
                        move || playlist().is_some_and(|(_, on)| on),
                        mark,
                    )
                    .mr(move || size.px(12.0))
                },
            ))
            .child(name.flex_1()),
    )
    .child(when(
        move || !subtitle().is_empty(),
        move || crate::ui_kit::caption(size, subtitle).text_color(move || mark().secondary()),
    ))
    .on_press(move || {
        ui.touched();
        let picked = ui.screens.menu.with(|m| m.as_ref()?.action_at(at.index()));
        if let Some(item) = picked {
            perform(ui, item);
        }
    })
}

fn row_title(ui: Ui, row: Option<Row>, playlist: impl Fn() -> Option<(String, bool)>) -> String {
    match row {
        Some(Row::Heading(h)) => heading_text(ui, h),
        Some(Row::Action {
            item: MenuItem::TogglePlaylist(_),
            ..
        }) => playlist().map(|(name, _)| name).unwrap_or_default(),
        Some(Row::Action { item, .. }) => {
            ui.guide_data();
            ui.screens.menu.with(|menu| {
                let Some(menu) = menu.as_ref() else {
                    return String::new();
                };
                let channel = menu_channel(&menu.target);
                let favorite = item == MenuItem::ToggleFavorite
                    && channel.is_some_and(|ch| ui.app.favorites.with(|f| f.contains(&ch)));
                let blocked = directly_blocked(&menu.list, item, channel);
                if let Some(preference) = preference_label(item, &menu.list) {
                    return preference;
                }
                match (item, &menu.target) {
                    (MenuItem::WatchInNewWindow, Target::Channel { .. }) => {
                        tr("Open in new window")
                    }
                    _ => label(item, favorite, blocked),
                }
                .to_owned()
            })
        }
        None => String::new(),
    }
}
