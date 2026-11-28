//! Chooser layouts and their focused, identity-keyed rows.
use super::*;

/// The choosers' layer: `leave` for the question before leaving, over
/// every screen; else the others, over the video. Each shows only its own.
pub fn layer(ui: Ui, leave: bool) -> impl IntoElement {
    let open = move || is_open(ui, leave);
    let overlay = if leave {
        crate::chrome::overlays::Overlay::Leave
    } else {
        crate::chrome::overlays::Overlay::Chooser
    };
    let width = signal(None);
    if !leave {
        bind_more(ui, width);
    }
    let count = move || {
        if open() {
            ui.screens
                .chooser
                .with(|c| c.as_ref().map_or(0, |c| c.list.len))
        } else {
            0
        }
    };
    let size = Scale::new();
    let list = element_ref();
    let focus = computed(move || {
        open()
            .then(|| {
                ui.screens
                    .chooser
                    .with(|c| c.as_ref().map(|c| c.list.focus))
            })
            .flatten()
    });
    effect(move || {
        if let Some(i) = focus.get() {
            list.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    let kind = move || {
        open()
            .then(|| ui.screens.chooser.with(|c| c.as_ref().map(|c| c.kind)))
            .flatten()
    };
    // Mounted while closed: nothing in it says `PointerEvents::Auto`
    // (`channels_view::layer`).
    view()
        .absolute()
        .inset_0()
        // This layer still measures the local window when a short chooser
        // hides the bar, including in an auxiliary player window.
        .on_layout(move |bounds: &Bounds<Points>| {
            let measured = (bounds.size.width.0 > 0.0).then_some(bounds.size.width.0);
            if width.get_untracked() != measured {
                width.set(measured);
            }
        })
        // At the right, clear of the panels at the bottom.
        .items_end()
        .justify_center()
        .pt_safe_area()
        .p(px(40.0))
        // Clear of the play bar while it shows.
        .pb(move || {
            px(if crate::playback::play_bar::shows(ui) {
                crate::playback::play_bar::height(ui) + 16.0
            } else {
                40.0
            })
        })
        .test_id(if leave {
            "leave-layer"
        } else {
            "chooser-layer"
        })
        .accessibility_elements_hidden(move || overlay.accessibility_hidden(ui))
        .opacity(move || if open() { 1.0_f32 } else { 0.0 })
        .pointer_events(move || {
            if open() {
                PointerEvents::BoxNone
            } else {
                PointerEvents::None
            }
        })
        // A tap beside the chooser closes it (the play bar, over this
        // layer, keeps its own presses).
        .child(
            view()
                .absolute()
                .inset_0()
                .label(move || tr("Close the chooser"))
                .on_press(move || {
                    ui.touched();
                    close(ui);
                }),
        )
        .child(panel(ui, leave, size, list, count, kind))
}

fn bind_more(ui: Ui, width: Signal<Option<f32>>) {
    let more_open = computed(move || {
        ui.screens
            .chooser
            .with(|chooser| chooser.as_ref().is_some_and(|c| c.kind == Kind::More))
    });
    effect(move || {
        if !more_open.get() {
            return;
        }
        let Some(width) = width.get() else {
            return;
        };
        let controls = crate::playback::play_bar::overflow(ui, width);
        if controls.is_empty() {
            close(ui);
        } else if ui.screens.chooser.with_untracked(|chooser| {
            chooser
                .as_ref()
                .and_then(|c| c.more.as_ref())
                .is_some_and(|more| more.controls != controls)
        }) {
            ui.screens.chooser.update(|chooser| {
                if let Some(open) = chooser
                    && let Some(more) = &mut open.more
                {
                    open.list.replace_actions(&more.controls, &controls);
                    more.controls = controls;
                }
            });
        }
    });
}

fn row(ui: Ui, size: Scale, at: Position) -> View {
    // Cells are recycled while a long list scrolls: read `at` where used.
    let entry = move || {
        ui.screens.chooser.with(|c| {
            c.as_ref().and_then(|c| {
                let index = at.index();
                let label = match c.kind {
                    Kind::More => {
                        crate::playback::play_bar::label(ui, *c.more.as_ref()?.controls.get(index)?)
                            .to_owned()
                    }
                    Kind::Cast | Kind::History | Kind::Volume => c.labels.get(index)?.clone(),
                    Kind::Track(_) => c
                        .track_labels_in(crate::chrome::i18n::language())
                        .get(index)?
                        .clone(),
                    _ => entries(ui, c.kind).0.get(index)?.clone(),
                };
                let checked = match c.kind {
                    Kind::Cast => c
                        .values
                        .get(index)
                        .is_some_and(|value| crate::screens::casting::destination_selected(value)),
                    Kind::History | Kind::AirPlay | Kind::CastEnded | Kind::More | Kind::Leave => {
                        false
                    }
                    Kind::Volume => volume_index(ui) == index,
                    _ => c.list.selected == index,
                };
                Some((label, checked, c.list.focus == index))
            })
        })
    };
    let selected = move || entry().is_some_and(|(_, checked, _)| checked);
    let mark = move || Mark::of(ui.keys_shown() && entry().is_some_and(|(_, _, f)| f), false);
    let action = move || {
        ui.screens.chooser.with(|c| {
            c.as_ref().is_some_and(|c| {
                matches!(
                    c.kind,
                    Kind::Cast | Kind::AirPlay | Kind::CastEnded | Kind::More | Kind::Leave
                )
            })
        })
    };
    list_rows::row(size, ROW, mark)
        .role(move || {
            if action() {
                AccessibilityRole::Button
            } else {
                AccessibilityRole::Option
            }
        })
        .accessibility_state(move || AccessibilityState {
            selected: selected(),
            checked: (!action()).then(|| {
                if selected() {
                    Checked::True
                } else {
                    Checked::False
                }
            }),
            ..Default::default()
        })
        .on_press(move || {
            let index = at.index();
            let channel_zap = ui.screens.chooser.with_untracked(|c| {
                c.as_ref()
                    .filter(|c| c.kind == Kind::More)
                    .and_then(|c| c.more.as_ref()?.controls.get(index).copied())
                    .is_some_and(|control| {
                        matches!(
                            control,
                            iptv_presentation::play_bar::Control::PreviousChannel
                                | iptv_presentation::play_bar::Control::NextChannel
                        )
                    })
            });
            if channel_zap {
                crate::playback::touch_zap::touched(ui);
            } else {
                ui.touched();
            }
            // The row unmounts on selection. Playback actions and timers
            // belong to the window where the chooser opened.
            ui.owner.run(move || pick(ui, index));
        })
        .child(
            list_rows::name(size, move || {
                entry().map_or(String::new(), |(label, selected, _)| {
                    format!("{}{label}", if selected { "✓  " } else { "    " })
                })
            })
            .text_color(move || mark().text()),
        )
}

fn panel(
    ui: Ui,
    leave: bool,
    size: Scale,
    list: ElementRef,
    count: impl Fn() -> usize + Copy + 'static,
    kind: impl Fn() -> Option<Kind> + Copy + 'static,
) -> View {
    view()
        .test_id(if leave {
            "leave-panel"
        } else {
            "chooser-panel"
        })
        // Never wider than the window (an iPad's narrowest windows).
        .max_w(percentage(1.0))
        // Nor taller than the room between the safe area and the
        // play bar: the list scrolls in what is left.
        .max_h(percentage(1.0))
        .w(move || {
            size.px(if kind() == Some(Kind::History) {
                520.0
            } else {
                380.0
            })
        })
        .p(px(12.0))
        .gap(px(2.0))
        .rounded(px(RADIUS_LARGE))
        // Translucent over moving video, like the reference's choosers.
        .bg(move || tint(ui.app.theme.with(|t| t.background.panel), 0xD0))
        .child(
            ui_kit::heading(size, move || kind().map_or("", title))
                .px(move || size.px(list_rows::INSET))
                .py(move || size.px(8.0)),
        )
        .child(if leave {
            view()
        } else {
            crate::screens::airplay::picker(ui, move || kind() == Some(Kind::AirPlay))
        })
        .child(
            virtual_list(count, move |at| row(ui, size, at))
                // Preserve a press across updates to the action list.
                .keys(move || {
                    let len = count();
                    ui.screens
                        .chooser
                        .with(|chooser| row_keys(chooser.as_ref(), len))
                })
                .estimated_item_size(list_rows::pitch(size, ROW))
                .element_ref(&list)
                .test_id(if leave {
                    "leave-items"
                } else {
                    "chooser-items"
                })
                .flex_shrink(1.0)
                .min_h(px(0.0))
                .h(move || {
                    let content =
                        count().clamp(1, MAX_ROWS) as f32 * list_rows::pitch(size, ROW)().0;
                    // An auxiliary window can mount before its host
                    // publishes a viewport. Never collapse its list.
                    let available = window().height.0;
                    px(if available > 0.0 {
                        content.min(available * 0.7)
                    } else {
                        content
                    })
                }),
        )
        .child(native_picker(ui, leave, kind))
}

fn native_picker(
    ui: Ui,
    leave: bool,
    kind: impl Fn() -> Option<Kind> + 'static,
) -> impl IntoElement {
    when(
        move || {
            !leave
                && kind() == Some(Kind::Cast)
                && crate::wiring::runtime_owner::runtime()
                    .native_cast_view()
                    .is_some()
        },
        move || {
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .px(px(16.0))
                .child(crate::screens::airplay::button(
                    ui,
                    48.0,
                    "chooser-native-device-picker",
                ))
                .child(text(move || tr("Choose a device")))
        },
    )
}
