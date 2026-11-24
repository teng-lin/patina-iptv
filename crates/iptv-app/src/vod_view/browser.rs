//! Poster browsing: navigation, category preview, focused metadata and a
//! bounded, virtualized grid. No provider URL is passed to the image host.
use super::*;
use iptv_core::vod::Card;
use iptv_presentation::browse::RailItem;
use iptv_presentation::vod::Destination;

fn all_label(ui: Ui) -> &'static str {
    if ui.vod.with(|s| s.query.kind == Kind::Series) {
        tr("All shows")
    } else {
        tr("All movies")
    }
}

pub(super) fn activate_navigation(ui: Ui) {
    match ui.vod.with_untracked(|s| s.destination) {
        Destination::Live => close(ui),
        Destination::Movies | Destination::Shows => {
            let kind = if ui
                .vod
                .with_untracked(|s| s.destination == Destination::Shows)
            {
                Kind::Series
            } else {
                Kind::Movie
            };
            if ui.vod.with_untracked(|s| s.query.kind == kind) {
                ui.vod.update(|s| s.open_categories());
            } else {
                open(ui, kind);
            }
        }
        Destination::Recordings => {
            close(ui);
            crate::recordings_view::open(ui);
        }
    }
}

/// Whether the rail widens to show its labels: while the remote is on it,
/// except beside the one-pane empty catalog, which keeps the width.
fn rail_labels(ui: Ui) -> bool {
    ui.vod
        .with(|s| s.control == Control::Navigation && !s.one_pane())
}

fn navigation(ui: Ui, size: Scale) -> View {
    view()
        .w(move || size.px(if rail_labels(ui) { 275.0 } else { 70.0 }))
        .h_full()
        .flex_shrink_0()
        .bg(move || rgb(ui.app.theme.with(|t| t.background.panel)))
        .py(move || size.px(24.0))
        .gap(move || size.px(10.0))
        .child(
            view()
                .size(move || size.px(ui_kit::TOUCH))
                .items_center()
                .justify_center()
                .role(AccessibilityRole::Button)
                .label(tr("Close"))
                .on_press(move || {
                    ui.touched();
                    close(ui);
                })
                .child(
                    svg(crate::play_bar::icons::src(
                        crate::play_bar::icons::Icon::Close,
                    ))
                    .size(move || size.px(24.0))
                    .text_color(rgb(crate::theme::LIST_TEXT)),
                ),
        )
        .child(view().flex_1())
        .children(
            [
                (Destination::Live, RailItem::Tv, tr("Live TV")),
                (Destination::Movies, RailItem::Movies, tr("Movies")),
                (Destination::Shows, RailItem::Shows, tr("Shows")),
                (
                    Destination::Recordings,
                    RailItem::Recordings,
                    tr("Recordings"),
                ),
            ]
            .into_iter()
            .map(move |(destination, item, label)| {
                // Only what can show anything here (`Screen::set_navigation`).
                let offered = move || ui.vod.with(|s| s.destinations().contains(&destination));
                when(offered, move || {
                    let focused = move || {
                        ui.vod.with(|s| {
                            s.control == Control::Navigation && s.destination == destination
                        })
                    };
                    view()
                        .h(move || size.px(52.0))
                        .mx(move || size.px(6.0))
                        .items_center()
                        .justify_center()
                        .rounded(move || size.px(crate::theme::RADIUS))
                        .bg(move || {
                            if focused() {
                                rgb(crate::theme::FOCUSED_CELL)
                            } else {
                                crate::tint(0, 0)
                            }
                        })
                        .role(AccessibilityRole::Tab)
                        .label(label)
                        .accessibility_state(move || AccessibilityState {
                            selected: ui.vod.with(|s| match destination {
                                Destination::Movies => s.query.kind == Kind::Movie,
                                Destination::Shows => s.query.kind == Kind::Series,
                                _ => false,
                            }),
                            ..Default::default()
                        })
                        .on_press(move || {
                            ui.touched();
                            ui.vod.update(|s| s.destination = destination);
                            activate_navigation(ui);
                        })
                        .flex_row()
                        .gap(move || size.px(14.0))
                        .child(
                            svg(crate::rail::icon(item))
                                .size(move || size.px(26.0))
                                .text_color(move || {
                                    rgb(if focused() {
                                        crate::theme::FOCUSED_TEXT
                                    } else {
                                        crate::theme::LIST_TEXT
                                    })
                                }),
                        )
                        .child(when(
                            move || rail_labels(ui),
                            move || {
                                ui_kit::label(size, label).number_of_lines(1).text_color(
                                    move || {
                                        rgb(if focused() {
                                            crate::theme::FOCUSED_TEXT
                                        } else {
                                            crate::theme::LIST_TEXT
                                        })
                                    },
                                )
                            },
                        ))
                })
            }),
        )
        .child(view().flex_1())
}

fn choose_category(ui: Ui, category: Option<(i64, String)>, continuing: bool) {
    ui.touched();
    ui.vod.update(|s| {
        s.control = Control::Category;
        s.query.category = category;
        s.query.continue_watching = continuing;
    });
    reload(ui, false);
}

fn category_button(
    ui: Ui,
    size: Scale,
    label: impl IntoReactive<SharedString>,
    focused: impl Fn() -> bool + Copy + 'static,
    pressed: impl Fn() + 'static,
) -> View {
    let label = label.into_reactive();
    let spoken = label.clone();
    let mark = move || crate::list_rows::Mark::of(focused(), false);
    view()
        .min_h(move || size.px(ui_kit::TOUCH))
        .px(move || size.px(16.0))
        .justify_center()
        .rounded(move || size.px(crate::theme::RADIUS))
        .bg(move || mark().bg())
        .role(AccessibilityRole::Button)
        .label(move || spoken.get().to_string())
        .on_press(move || {
            ui.touched();
            pressed();
        })
        .child(
            ui_kit::label(size, label)
                .number_of_lines(1)
                .text_color(move || mark().text()),
        )
}

fn categories(ui: Ui, size: Scale) -> impl IntoElement {
    when(
        move || ui.vod.with(|s| s.shows_categories()),
        move || {
            let rows = element_ref();
            let chosen = move || {
                ui.vod.with(|s| {
                    if s.query.continue_watching {
                        0
                    } else {
                        s.query
                            .category
                            .as_ref()
                            .and_then(|key| {
                                s.categories
                                    .iter()
                                    .position(|c| (c.source, c.id.clone()) == *key)
                            })
                            .map_or(1, |at| at + 2)
                    }
                })
            };
            effect(move || {
                rows.scroll_to_index(chosen() as u32, percentage(0.5), px(0.0), false);
            });
            view()
                .w(move || size.px(320.0))
                .flex_shrink_0()
                .h_full()
                .px(move || size.px(20.0))
                .pt(move || size.px(210.0))
                .child(category_button(
                    ui,
                    size,
                    move || {
                        ui.vod.with(|s| {
                            s.query
                                .source
                                .and_then(|id| s.sources.iter().find(|v| v.id == id))
                                .map(|v| v.name.clone())
                                .unwrap_or_else(|| tr("All playlists").into())
                        })
                    },
                    move || ui.vod.with(|s| s.control == Control::Source),
                    move || {
                        ui.vod.update(|s| s.control = Control::Source);
                        cycle_source(ui);
                    },
                ))
                .child(
                    virtual_list(
                        move || ui.vod.with(|s| s.categories.len() + 2),
                        move |at| {
                            let category = move || {
                                ui.vod.with(|s| {
                                    at.index()
                                        .checked_sub(2)
                                        .and_then(|i| s.categories.get(i))
                                        .cloned()
                                })
                            };
                            let selected = move || {
                                ui.vod.with(|s| match at.index() {
                                    0 => s.query.continue_watching,
                                    1 => !s.query.continue_watching && s.query.category.is_none(),
                                    _ => category().is_some_and(|c| {
                                        s.query.category.as_ref() == Some(&(c.source, c.id))
                                    }),
                                })
                            };
                            category_button(
                                ui,
                                size,
                                move || match at.index() {
                                    0 => tr("Continue watching").into(),
                                    1 => all_label(ui).into(),
                                    _ => category().map(|c| c.name).unwrap_or_default(),
                                },
                                move || {
                                    selected() && ui.vod.with(|s| s.control == Control::Category)
                                },
                                move || {
                                    choose_category(
                                        ui,
                                        category().map(|c| (c.source, c.id)),
                                        at.index() == 0,
                                    );
                                },
                            )
                            .test_id({
                                if at.index() == 0 {
                                    "vod-continue"
                                } else {
                                    "vod-category"
                                }
                            })
                            .accessibility_state(move || {
                                AccessibilityState {
                                    selected: selected(),
                                    ..Default::default()
                                }
                            })
                        },
                    )
                    .starting_at(Some(untracked(chosen)))
                    .element_ref(&rows)
                    .estimated_item_size(move || size.px(ui_kit::TOUCH))
                    .flex_1(),
                )
        },
    )
}

pub(super) fn focused_card(ui: Ui) -> Option<Card> {
    ui.vod.with(|s| {
        if let Some(details) = &s.details {
            return Some(details.item.clone());
        }
        if let Some(preview) = &s.preview
            && s.focused.as_ref() == Some(&preview.item.key)
        {
            return Some(preview.item.clone());
        }
        s.focused
            .as_ref()
            .and_then(|key| s.cards.iter().find(|c| &c.key == key))
            .cloned()
    })
}

pub(super) fn summary(card: &Card) -> String {
    let mut parts = Vec::new();
    if let Some(rating) = card.rating_x10 {
        parts.push(format!("{:.1}", f32::from(rating) / 10.0));
    }
    if let Some(year) = card.year {
        parts.push(year.to_string());
    }
    if let Some(duration) = card.duration_secs {
        parts.push(trf("{minutes} min", &[("minutes", (duration / 60).into())]));
    }
    parts.push(card.source_name.clone());
    parts.join(" · ")
}

fn tools(ui: Ui, size: Scale) -> View {
    view().flex_row().gap(move || size.px(12.0)).children(
        [Control::Search, Control::Sort]
            .into_iter()
            .map(move |control| {
                let focused = move || ui.vod.with(|s| s.control == control);
                view()
                    .size(move || size.px(ui_kit::TOUCH))
                    .rounded(move || size.px(crate::theme::RADIUS))
                    .role(AccessibilityRole::Button)
                    .label(move || {
                        if control == Control::Search {
                            tr("Search").to_owned()
                        } else {
                            ui.vod.with(|s| {
                                format!(
                                    "{} · {}",
                                    tr("Sort"),
                                    match s.query.sort {
                                        Sort::Name => tr("Name"),
                                        Sort::Newest => tr("Recently added"),
                                        Sort::Year => tr("Year"),
                                        Sort::Rating => tr("Rating"),
                                    }
                                )
                            })
                        }
                    })
                    .items_center()
                    .justify_center()
                    .bg(move || {
                        if focused() {
                            rgb(crate::theme::FOCUSED_CELL)
                        } else {
                            crate::tint(0, 0)
                        }
                    })
                    .on_press(move || {
                        ui.touched();
                        ui.vod.update(|s| s.control = control);
                        activate(ui);
                    })
                    .child(
                        svg(if control == Control::Search {
                            crate::rail::icon(RailItem::Search)
                        } else {
                            crate::play_bar::icons::src(crate::play_bar::icons::Icon::Sort)
                        })
                        .size(move || size.px(26.0))
                        .text_color(move || {
                            rgb(if focused() {
                                crate::theme::FOCUSED_TEXT
                            } else {
                                crate::theme::LIST_TEXT
                            })
                        }),
                    )
            }),
    )
}

pub(super) fn hero(ui: Ui, size: Scale) -> View {
    view()
        // The one-pane empty catalog has no title to describe: only the
        // tools' row, so its message and action stay in the window.
        .h(move || {
            size.px(if ui.vod.with(|s| s.one_pane()) {
                32.0 + ui_kit::TOUCH
            } else {
                300.0
            })
        })
        .flex_shrink_0()
        .pt(move || size.px(30.0))
        .gap(move || size.px(12.0))
        .test_id("vod-focused-metadata")
        .child(when(
            move || ui.vod.with(|s| s.details.is_none()),
            move || {
                view()
                    .absolute()
                    .right(px(0.0))
                    .top(move || size.px(20.0))
                    .child(tools(ui, size))
            },
        ))
        .child(
            ui_kit::library_title(size, move || {
                focused_card(ui).map(|c| c.name).unwrap_or_default()
            })
            .pr(move || size.px(130.0)),
        )
        .child(ui_kit::body(size, move || {
            let mut line = focused_card(ui).map(|c| summary(&c)).unwrap_or_default();
            let seasons = ui.vod.with(|s| {
                s.details
                    .as_ref()
                    .or_else(|| {
                        s.preview
                            .as_ref()
                            .filter(|d| s.focused.as_ref() == Some(&d.item.key))
                    })
                    .filter(|d| d.item.key.kind == Kind::Series)
                    .map(|d| d.seasons().len())
            });
            if let Some(count) = seasons.filter(|count| *count > 0) {
                let label = if count == 1 {
                    tr("1 season").to_owned()
                } else {
                    trf("{count} seasons", &[("count", count.into())])
                };
                line.push_str(&format!(" · {label}"));
            }
            line
        }))
        .child(
            ui_kit::body(size, move || {
                focused_card(ui).and_then(|c| c.plot).unwrap_or_default()
            })
            .number_of_lines(4),
        )
}

fn poster(
    ui: Ui,
    size: Scale,
    index: impl Fn() -> usize + Copy + 'static,
    width: Signal<f32>,
) -> View {
    let card = move || ui.vod.with(|s| s.cards.get(index()).cloned());
    let focused = move || {
        card().is_some_and(|c| {
            ui.vod
                .with(|s| s.control == Control::Catalog && s.focused.as_ref() == Some(&c.key))
        })
    };
    view()
        .flex_1()
        .min_w(px(0.0))
        .rounded(move || size.px(crate::theme::RADIUS_LARGE))
        .overflow_hidden()
        .border(move || size.px(2.0))
        .border_color(move || {
            if focused() {
                rgb(crate::theme::LIST_TEXT)
            } else {
                crate::tint(0, 0)
            }
        })
        .role(AccessibilityRole::Button)
        .test_id("vod-title")
        .label(move || card().map(|c| card_label(&c, false)).unwrap_or_default())
        .accessibility_state(move || AccessibilityState {
            selected: card().is_some_and(|c| ui.vod.with(|s| s.focused.as_ref() == Some(&c.key))),
            ..Default::default()
        })
        .on_press(move || {
            ui.touched();
            if let Some(c) = card() {
                select(ui, c.key);
            }
        })
        .child(
            view()
                .h(move || px(width.get() * 1.5))
                .bg(move || rgb(ui.app.theme.with(|t| t.background.panel)))
                .child(
                    when(
                        move || {
                            card().is_some_and(|c| c.poster.as_ref().is_some_and(|p| !p.is_empty()))
                        },
                        move || {
                            let source = crate::content_image::source(move || {
                                card().and_then(|c| c.poster).unwrap_or_default()
                            });
                            image(source)
                                .absolute()
                                .inset_0()
                                .object_fit(ResizeMode::Cover)
                        },
                    )
                    .otherwise(move || {
                        view()
                            .absolute()
                            .inset_0()
                            .items_center()
                            .justify_center()
                            .child(
                                svg(crate::rail::icon(
                                    if card().is_some_and(|c| c.key.kind == Kind::Series) {
                                        RailItem::Shows
                                    } else {
                                        RailItem::Movies
                                    },
                                ))
                                .size(move || size.px(40.0))
                                .text_color(ui_kit::secondary()),
                            )
                    }),
                )
                .child(when(
                    move || card().is_some_and(|c| c.rating_x10.is_some()),
                    move || {
                        ui_kit::caption(size, move || {
                            card()
                                .and_then(|c| c.rating_x10)
                                .map(|r| format!("{:.1}", f32::from(r) / 10.0))
                                .unwrap_or_default()
                        })
                        .absolute()
                        .left(move || size.px(6.0))
                        .bottom(move || size.px(6.0))
                        .px(move || size.px(4.0))
                        .rounded(move || size.px(3.0))
                        .bg(move || rgb(ui.app.theme.with(|t| t.accent.main)))
                        .text_color(rgb(crate::theme::LIST_TEXT))
                    },
                )),
        )
        .child(
            ui_kit::body(size, move || card().map(|c| c.name).unwrap_or_default())
                .text_color(move || {
                    rgb(if focused() {
                        crate::theme::FOCUSED_TEXT
                    } else {
                        crate::theme::LIST_TEXT
                    })
                })
                .number_of_lines(1)
                .px(move || size.px(6.0))
                .py(move || size.px(6.0))
                .bg(move || {
                    rgb(if focused() {
                        crate::theme::FOCUSED_CELL
                    } else {
                        ui.app.theme.with(|t| t.background.panel)
                    })
                }),
        )
}

fn grid(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = element_ref();
    let width = signal(size.of(150.0));
    let focused_row = computed(move || {
        ui.vod
            .with(|s| s.focused_index().map(|at| at / s.columns.max(1)))
    });
    effect(move || {
        if let Some(at) = focused_row.get() {
            rows.scroll_to_index(at as u32, percentage(0.0), px(0.0), false);
        }
    });
    view()
        .flex_1()
        .min_h(px(0.0))
        .test_id("vod-poster-grid")
        .on_layout(move |b: &Bounds<Points>| {
            // Seven cards at 960dp on TV, fewer while categories are open.
            let columns = ((b.size.width.0 + size.of(10.0)) / size.of(150.0))
                .floor()
                .clamp(1.0, 12.0) as usize;
            width.set(
                ((b.size.width.0 - size.of(10.0) * (columns - 1) as f32) / columns as f32
                    - size.of(4.0))
                .max(1.0),
            );
            if ui.vod.with_untracked(|s| s.columns != columns) {
                ui.vod.update(|s| s.columns = columns);
            }
        })
        .child(
            virtual_list(
                move || ui.vod.with(|s| s.cards.len().div_ceil(s.columns.max(1))),
                move |row| {
                    view()
                        .flex_row()
                        .gap(move || size.px(10.0))
                        .pb(move || size.px(10.0))
                        .children((0..12).map(move |column| {
                            when(
                                move || ui.vod.with(|s| column < s.columns.max(1)),
                                move || {
                                    let index = move || {
                                        row.index() * ui.vod.with(|s| s.columns.max(1)) + column
                                    };
                                    when(
                                        move || ui.vod.with(|s| index() < s.cards.len()),
                                        move || poster(ui, size, index, width),
                                    )
                                    .otherwise(|| view().flex_1())
                                },
                            )
                        }))
                },
            )
            .starting_at(focused_row.get_untracked())
            .element_ref(&rows)
            .estimated_item_size(move || px(width.get() * 1.5 + size.of(50.0)))
            .flex_1(),
        )
}

fn category_heading(ui: Ui, size: Scale) -> impl IntoElement {
    // The one-pane empty catalog has nothing to filter.
    when(
        move || ui.vod.with(|s| !s.one_pane()),
        move || category_heading_button(ui, size),
    )
}

fn category_heading_button(ui: Ui, size: Scale) -> View {
    category_button(
        ui,
        size,
        move || {
            ui.vod.with(|s| {
                if s.query.continue_watching {
                    tr("Continue watching").into()
                } else {
                    s.query
                        .category
                        .as_ref()
                        .and_then(|(source, id)| {
                            s.categories
                                .iter()
                                .find(|c| c.source == *source && c.id == *id)
                        })
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| all_label(ui).into())
                }
            })
        },
        || false,
        move || ui.vod.update(|s| s.open_categories()),
    )
    .px(px(0.0))
}

pub(super) fn screen(ui: Ui, size: Scale) -> View {
    super::empty::bind(ui);
    // The layout class at the current text size (`crate::layout`):
    // a compact window shows the empty catalog as its one pane.
    let compact = computed(move || {
        use crate::layout::Class;
        let text = size.of(1.0) * crate::settings::text_factor();
        Class::of(window().width.0, text) == Class::Compact
    });
    effect(move || {
        let compact = compact.get();
        ui.vod.update(|s| s.set_compact(compact));
    });
    let offered = computed(move || {
        ui.app.library.version.get();
        (
            !crate::lists::live_only(),
            crate::capabilities::recordings_library(ui),
        )
    });
    effect(move || {
        let (catalogs, recordings) = offered.get();
        ui.vod.update(|s| s.set_navigation(catalogs, recordings));
    });
    let preview = computed(move || ui.vod.with(|s| (s.focused.clone(), s.generation)));
    effect(move || {
        let (Some(key), generation) = preview.get() else {
            return;
        };
        let owner = crate::runtime();
        let cancel = iptv_import::CancelToken::new();
        let on_drop = cancel.clone();
        cleanup(move || on_drop.cancel());
        crate::data::spawn_in_app(async move {
            timer(std::time::Duration::from_millis(250)).await;
            if cancel.is_canceled()
                || !ui.vod.with_untracked(|s| {
                    s.open
                        && s.details.is_none()
                        && s.generation == generation
                        && s.focused.as_ref() == Some(&key)
                })
            {
                return;
            }
            if let Ok(details) = owner.vod_preview(key, cancel).await {
                // No await between the access check and UI publication: a
                // revoked grant must not restore metadata after redaction.
                if owner.parental_allowed(&iptv_runtime::vod::target(&details.item.key)) {
                    ui.vod.update(|s| {
                        s.publish_preview(generation, details);
                    });
                }
            }
        });
    });
    let field = element_ref();
    crate::input::bind_field_focus(field, move || {
        ui.vod
            .with(|s| s.control == Control::Search && s.search_open)
    });
    view()
        .absolute()
        .inset_0()
        .pt_safe_area()
        .pb_safe_area()
        .flex_row()
        .test_id("vod-screen")
        .bg(move || rgb(ui.app.theme.with(|t| t.background.window)))
        .accessibility_elements_hidden(move || {
            crate::overlays::Overlay::Vod.accessibility_covered(ui)
        })
        .child(when(
            move || ui.vod.with(|s| s.shows_navigation()),
            move || navigation(ui, size),
        ))
        .child(categories(ui, size))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .px(move || size.px(30.0))
                .child(hero(ui, size))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(move || size.px(10.0))
                        .child(category_heading(ui, size))
                        .child(view().flex_1())
                        .child(ui_kit::caption(size, move || {
                            ui.vod.with(|s| {
                                s.focused_index()
                                    .map(|at| {
                                        format!(
                                            "{} / {}{}",
                                            at + 1,
                                            s.cards.len(),
                                            if s.has_more { "+" } else { "" }
                                        )
                                    })
                                    .unwrap_or_default()
                            })
                        })),
                )
                .child(when(
                    move || ui.vod.with(|s| s.search_open || !s.query.text.is_empty()),
                    move || {
                        ui_kit::field(size, input())
                            .element_ref(&field)
                            .test_id("vod-search")
                            .value(move || ui.vod.with(|s| s.query.text.clone()))
                            .placeholder(tr("Search movies and series"))
                            .on_change(move |text: &str| {
                                ui.vod.update(|s| s.query.text = text.into());
                                reload(ui, false);
                            })
                            .auto_correct(false)
                            .auto_capitalize(AutoCapitalize::None)
                            .return_key_type(ReturnKeyType::Search)
                            .on_submit(move |_: &str| ui.vod.update(|s| s.close_search()))
                            .on_focus(move || ui.vod.update(|s| s.control = Control::Search))
                    },
                ))
                .child(ui_kit::caption(size, move || status_message(ui)))
                .child(when(
                    move || ui.vod.with(|s| s.retry.is_some() && !s.busy),
                    move || {
                        button(ui, size, Control::Retry, tr("Retry"), move || retry(ui))
                            .test_id("vod-retry")
                    },
                ))
                .child(super::empty::screen(ui, size))
                .child(grid(ui, size))
                .child(when(
                    move || ui.vod.with(|s| s.has_more),
                    move || {
                        ui_kit::button(
                            ui,
                            size,
                            tr("Load more"),
                            || false,
                            move || {
                                if !ui.vod.with_untracked(|s| s.busy) {
                                    reload(ui, true);
                                }
                            },
                        )
                    },
                )),
        )
}
