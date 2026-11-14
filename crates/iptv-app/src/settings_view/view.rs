//! The settings screen drawn: the pages on the left, the chosen page's
//! rows on the right (a label, the value beside it, a second line when
//! the row says why it does nothing), the focused row's description
//! below, the editors over it (`editors`), and the notice line. In the
//! lists' type and pitch with their focus pill (`ui_kit`, `list_rows`).

use crate::i18n::{tr, trf};

use super::*;
use crate::fonts::{LIST_ITEM, SANS, SECTION};
use crate::list_rows::{self, Mark, ROW, ROW_TWO};
use crate::scale::Scale;
use crate::theme::LIST_TEXT;
use crate::ui_kit::{self, HEADING_ROW};
use nsh_core::Checked;

const PANEL_W: f32 = 860.0;
const PAGES_W: f32 = 230.0;
/// The description under the rows: two lines.
const FOOTER_H: f32 = 46.0;

pub(super) fn secondary() -> Rgba {
    ui_kit::secondary()
}

/// The settings screen and the notice line.
pub fn layer(ui: Ui) -> impl IntoElement {
    let notice = signal(None::<String>);
    NOTICE.with(|n| n.set(Some(notice)));
    let size = Scale::new();
    view()
        .test_id("settings-layer")
        .accessibility_elements_hidden(move || {
            crate::overlays::Overlay::Settings.accessibility_covered(ui)
        })
        .absolute()
        .inset_0()
        .pointer_events(PointerEvents::BoxNone)
        .child(when(move || is_open(ui), move || screen(ui, size)))
        .child(when(
            move || notice.with(Option::is_some),
            move || {
                view()
                    .role(AccessibilityRole::Alert)
                    .absolute()
                    .left(px(0.0))
                    .w_full()
                    .bottom(move || size.px(28.0))
                    .items_center()
                    .pointer_events(PointerEvents::None)
                    .child(
                        ui_kit::body(size, move || notice.get().unwrap_or_default())
                            .px(move || size.px(18.0))
                            .py(move || size.px(12.0))
                            .rounded(px(RADIUS))
                            .text_color(rgb(LIST_TEXT))
                            .bg(tint(0x000000, 0xC0)),
                    )
            },
        ))
}

fn screen(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = computed(move || {
        let settings = crate::settings::read(ui);
        state(ui, |s| {
            s.rows(&Ctx {
                settings: &settings,
                offer: &crate::settings::offer,
            })
        })
        .unwrap_or_default()
    });
    let editing = computed(move || state(ui, |s| s.editor().is_some()).unwrap_or(false));
    view()
        .test_id("settings-screen")
        .absolute()
        .inset_0()
        .flex_row()
        // The guide (or video) stays visible, dimmed; a tap there closes.
        .child(
            view()
                .flex_1()
                .accessibility_elements_hidden(move || editing.get())
                .bg(tint(0x000000, 0x90))
                .on_press(move || {
                    ui.touched();
                    close(ui);
                }),
        )
        .child(
            view()
                .test_id("settings-content")
                .accessibility_elements_hidden(move || editing.get())
                .w(move || size.px(PANEL_W))
                .max_w(percentage(0.92))
                .pt_safe_area()
                .px(move || size.px(20.0))
                .py(move || size.px(16.0))
                .gap(move || size.px(10.0))
                .bg(move || rgb(ui.theme.with(|t| t.background.panel)))
                .child(header(ui, size))
                .child(
                    view()
                        .flex_1()
                        .flex_row()
                        .gap(move || size.px(14.0))
                        .child(pages(ui, size))
                        .child(rows_list(ui, rows, size)),
                )
                .child(footer(ui, size)),
        )
        .child(when(move || editing.get(), move || editors::layer(ui)))
}

fn header(ui: Ui, size: Scale) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(move || size.px(16.0))
        .child(ui_kit::title(size, move || tr("Settings")))
        .child(
            ui_kit::caption(size, move || hint(ui))
                .flex_1()
                .number_of_lines(1),
        )
        .child(when(
            move || ui.back_controls(),
            move || ui_kit::button(ui, size, move || tr("Close"), || false, move || close(ui)),
        ))
}

fn hint(ui: Ui) -> &'static str {
    if !ui.keys_shown() {
        return tr("Tap a page, then a setting to change it");
    }
    let at = state(ui, |s| (s.pane(), s.editor().cloned())).unwrap_or((Pane::Pages, None));
    match at {
        (_, Some(Editor::Choice { .. })) => tr("▲▼ choose   OK keep   Back cancel"),
        (_, Some(Editor::Number { .. })) => tr("◀▶ change   OK keep   Back cancel"),
        (_, Some(Editor::Text { .. })) => tr("Type, then OK to keep   Back cancel"),
        (_, Some(Editor::Confirm { .. })) => tr("OK go ahead   Back cancel"),
        (Pane::Pages, None) => tr("▲▼ page   ▶ OK its settings   Back close"),
        (Pane::Rows, None) => tr("▲▼ move   OK change   ◀ Back pages"),
    }
}

fn pages(ui: Ui, size: Scale) -> impl IntoElement {
    let list = element_ref();
    let chosen = computed(move || state(ui, Screen::page_index));
    effect(move || {
        if let Some(i) = chosen.get() {
            list.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    let entry = move |at: Position| {
        let page = move || PAGES.get(at.index());
        let is_chosen = move || chosen.get() == Some(at.index());
        let focused = move || {
            ui.keys_shown()
                && is_chosen()
                && state(ui, |s| s.pane() == Pane::Pages && s.editor().is_none()) == Some(true)
        };
        let mark = move || Mark::of(focused(), is_chosen());
        list_rows::row(size, ROW, mark)
            .role(AccessibilityRole::Tab)
            .accessibility_state(move || AccessibilityState {
                selected: is_chosen(),
                ..Default::default()
            })
            .on_press(move || {
                ui.touched();
                let i = at.index();
                update(ui, move |screen, _| {
                    screen.tap_page(i);
                    Effect::None
                });
            })
            .child(
                list_rows::name(size, move || page().map_or("", |p| words::page(p.key)))
                    .text_color(move || {
                        crate::settings::read(ui);
                        match mark() {
                            Mark::Focused => Mark::Focused.text(),
                            Mark::Current => rgb(ui.theme.with(|t| t.accent.light)),
                            Mark::Plain if page().is_some_and(lit_page) => rgb(LIST_TEXT),
                            Mark::Plain => tint(WHITE, DIM_ALPHA),
                        }
                    }),
            )
    };
    virtual_list(|| PAGES.len(), entry)
        .estimated_item_size(list_rows::pitch(size, ROW))
        .element_ref(&list)
        .w(move || size.px(PAGES_W))
}

fn rows_list(ui: Ui, rows: Memo<Vec<Row>>, size: Scale) -> impl IntoElement {
    let list = element_ref();
    let focused_index = computed(move || {
        let key = state(ui, |s| {
            (s.pane() == Pane::Rows).then(|| s.focus()).flatten()
        })
        .flatten()?;
        rows.with(|r| r.iter().position(|row| row.key() == Some(key)))
    });
    effect(move || {
        if let Some(i) = focused_index.get()
            && ui.keys_shown()
        {
            list.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    // Another page starts at its top.
    let page = computed(move || state(ui, Screen::page_index));
    effect(move || {
        page.get();
        list.scroll_to_index(0, percentage(0.0), px(0.0), false);
    });
    virtual_list(
        move || rows.with(Vec::len),
        move |at| row_view(ui, rows, at, focused_index, size),
    )
    .estimated_item_size(list_rows::pitch(size, ROW))
    .element_ref(&list)
    .flex_1()
}

/// The colour of a palette value, for its swatch.
pub(super) fn swatch_colour(ty: &ValueType, variant: &str) -> Option<u32> {
    let ValueType::Colour { palette, .. } = ty else {
        return None;
    };
    let key = |name: &str| crate::settings::palette_key(name);
    match palette.family {
        "accent" => crate::theme::ACCENTS
            .iter()
            .find(|a| key(a.name) == variant)
            .map(|a| a.light),
        _ => crate::theme::BACKGROUNDS
            .iter()
            .find(|b| key(b.name) == variant)
            .map(|b| b.panel),
    }
}

fn row_view(
    ui: Ui,
    rows: Memo<Vec<Row>>,
    at: Position,
    focused_index: Memo<Option<usize>>,
    size: Scale,
) -> impl IntoElement {
    let row = move || rows.with(|r| r.get(at.index()).copied());
    let heading = move || matches!(row(), Some(Row::Heading { .. }));
    let lit = move || {
        let settings = crate::settings::read(ui);
        row().is_some_and(|r| matches!(status(&settings, &r), Status::Active | Status::ReadOnly))
    };
    let focused = move || ui.keys_shown() && focused_index.get() == Some(at.index());
    let mark = move || Mark::of(focused(), false);
    let title = move || match row() {
        Some(Row::Heading {
            section,
            subsection,
        }) => words::heading(section, subsection),
        Some(r) => r.key().map_or("", words::label),
        None => "",
    };
    let note = move || {
        let settings = crate::settings::read(ui);
        row().map(|r| note(&settings, &r)).unwrap_or_default()
    };
    let current = move || {
        let settings = crate::settings::read(ui);
        match row() {
            Some(Row::Setting(s)) => {
                let value = settings.get(s.key).unwrap_or_else(|_| s.default_value());
                let swatch = match &value {
                    Value::Colour(v) => swatch_colour(&s.ty, v),
                    _ => None,
                };
                (words::value(s, &value), swatch)
            }
            Some(Row::Command(c)) => {
                ui.data_version.get();
                ui.library.version.get();
                (command_value(c.key), None)
            }
            _ => (String::new(), None),
        }
    };
    // A heading's row, a one-line row, or two lines with the note.
    let pitch = move || match (heading(), note().is_empty()) {
        (true, _) => HEADING_ROW,
        (false, true) => ROW,
        (false, false) => ROW_TWO,
    };
    let is_toggle = move || match row() {
        Some(Row::Setting(s)) => matches!(s.ty, ValueType::Bool { .. }),
        _ => false,
    };
    let bool_val = move || {
        let settings = crate::settings::read(ui);
        match row() {
            Some(Row::Setting(s)) => match s.ty {
                ValueType::Bool { .. } => settings.get(s.key) == Ok(Value::Bool(true)),
                _ => false,
            },
            _ => false,
        }
    };
    list_rows::row_with(size, pitch, mark)
        .role(move || {
            if heading() {
                AccessibilityRole::Header
            } else if is_toggle() {
                AccessibilityRole::Switch
            } else {
                AccessibilityRole::Button
            }
        })
        .accessibility_state(move || {
            if heading() {
                AccessibilityState::default()
            } else if is_toggle() {
                AccessibilityState {
                    disabled: !lit(),
                    checked: Some(if bool_val() {
                        Checked::True
                    } else {
                        Checked::False
                    }),
                    ..Default::default()
                }
            } else {
                AccessibilityState {
                    disabled: !lit(),
                    ..Default::default()
                }
            }
        })
        .flex_row()
        .items_center()
        .justify_start()
        .gap(move || size.px(12.0))
        .on_press(move || {
            ui.touched();
            if let Some(key) = row().and_then(|r| r.key()) {
                update(ui, |screen, ctx| screen.tap_row(key, ctx));
            }
        })
        .child(
            view()
                .flex_1()
                .child(
                    list_rows::name(size, title)
                        .pt(move || size.px(if heading() { 8.0 } else { 0.0 }))
                        .text_size(move || {
                            size.text_px(if heading() { SECTION } else { LIST_ITEM })
                        })
                        .text_color(move || match (heading(), mark(), lit()) {
                            (true, _, _) => secondary(),
                            (false, Mark::Focused, true) => Mark::Focused.text(),
                            (false, Mark::Focused, false) => Mark::Focused.disabled(),
                            (false, _, true) => rgb(LIST_TEXT),
                            (false, _, false) => tint(WHITE, DIM_ALPHA),
                        }),
                )
                .child(when(
                    move || !note().is_empty(),
                    move || ui_kit::detail(size, note).text_color(move || mark().secondary()),
                )),
        )
        .child(
            view()
                .w(move || size.px(16.0))
                .h(move || size.px(16.0))
                .rounded(px(3.0))
                .border(px(1.0))
                .border_color(move || mark().text())
                .bg(move || current().1.map_or(tint(WHITE, 0), rgb))
                .opacity(move || if current().1.is_some() { 1.0_f32 } else { 0.0 }),
        )
        .child(
            list_rows::second(size, move || current().0, SANS).text_color(move || {
                match (mark(), lit()) {
                    (Mark::Focused, true) => Mark::Focused.text(),
                    (_, true) => rgb(ui.theme.with(|t| t.accent.light)),
                    (Mark::Focused, false) => Mark::Focused.disabled(),
                    (_, false) => tint(WHITE, TEXT_DISABLED_ALPHA),
                }
            }),
        )
}

fn footer(ui: Ui, size: Scale) -> impl IntoElement {
    let description = move || {
        let settings = crate::settings::read(ui);
        let ctx = Ctx {
            settings: &settings,
            offer: &crate::settings::offer,
        };
        state(ui, |s| match s.focused_row(&ctx) {
            Some(Row::Setting(st)) => st.summary.replace('`', ""),
            Some(Row::Command(c)) => words::description(c.key).unwrap_or(c.summary).to_owned(),
            _ => match counts(s.page()) {
                (_, 0) | (0, 1) => String::new(),
                (0, all) => trf(
                    "{count, plural, one {# setting here, not available yet} \
                     other {# settings here, none available yet}}",
                    &[("count", all.into())],
                ),
                (active, all) => trf(
                    "{active} of {all} settings here can be changed now",
                    &[("active", active.into()), ("all", all.into())],
                ),
            },
        })
        .unwrap_or_default()
    };
    ui_kit::body(size, description)
        .h(move || size.px(FOOTER_H))
        .px(move || size.px(4.0))
        .number_of_lines(2)
}
