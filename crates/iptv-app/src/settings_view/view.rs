//! The settings screen drawn: the pages on the left, the chosen page's
//! rows on the right (a label, the value beside it, a second line when
//! the row says why it does nothing), the focused row's description
//! below and the editors over it (`editors`). The global notice is mounted above
//! the screens so tasks started in setup can report their result.

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
/// Help keeps four lines at the same app/system text scale as its label.
const FOOTER_H: f32 = 72.0;

pub(super) fn secondary() -> Rgba {
    ui_kit::secondary()
}

/// The settings screen.
pub fn layer(ui: Ui) -> impl IntoElement {
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
}

pub(crate) fn notice_layer() -> impl IntoElement {
    let notice = Notice::new();
    NOTICE.with(|n| n.set(Some(notice)));
    let text = notice.text;
    let size = Scale::new();
    when(
        move || text.with(Option::is_some),
        move || {
            view()
                .test_id("app-notice")
                .role(AccessibilityRole::Alert)
                .absolute()
                .left(px(0.0))
                .w_full()
                .bottom(move || size.px(28.0))
                .items_center()
                .pointer_events(PointerEvents::None)
                .child(
                    ui_kit::body(size, move || text.get().unwrap_or_default())
                        .px(move || size.px(18.0))
                        .py(move || size.px(12.0))
                        .rounded(px(RADIUS))
                        .text_color(rgb(LIST_TEXT))
                        .bg(tint(0x000000, 0xC0)),
                )
        },
    )
}

fn screen(ui: Ui, size: Scale) -> impl IntoElement {
    let rows = computed(move || {
        let settings = crate::settings_wiring::read(ui);
        // `player.pip_on_leave` follows the host's report (`settings_offer::offer`).
        ui.pip.with(|p| p.supported);
        state(ui, |s| {
            s.rows(&Ctx {
                settings: &settings,
                offer: &crate::settings_offer::offer_now,
            })
        })
        .unwrap_or_default()
    });
    let editing = computed(move || state(ui, |s| s.editor().is_some()).unwrap_or(false));
    // What is stored changed (a playlist added, renamed, hidden, moved):
    // the Playlists page follows, after this change has reached the rows
    // already shown (a row it removes must not be read again).
    effect(move || {
        ui.app.library.version.get();
        let _later = set_timeout(Duration::ZERO, move || playlists_changed(ui));
    });
    // The pages with rows (reactive: a keyboard or D-pad attached or
    // detached, a setting that shows or hides rows). The list follows
    // only when they change, not when keys or touch are used, so a screen
    // reader's place in it holds.
    let shown = computed(move || {
        let settings = crate::settings_wiring::read(ui);
        let ctx = Ctx {
            settings: &settings,
            offer: &crate::settings_offer::offer_now,
        };
        PAGES
            .iter()
            .filter(|page| {
                iptv_presentation::settings::screen::rows(page, &ctx)
                    .iter()
                    .any(|row| row.key().is_some())
            })
            .map(|page| page.key)
            .collect::<Vec<_>>()
    });
    effect(move || {
        shown.with(|_| ());
        untracked(|| {
            update(ui, |screen, ctx| {
                screen.refresh(ctx);
                Effect::None
            })
        });
    });
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
                .bg(move || rgb(ui.app.theme.with(|t| t.background.panel)))
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
    // A playlist's sheet over the page: its own keys.
    if crate::playlists_view::task_open(ui) {
        return crate::playlists_view::hint(ui);
    }
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
        (Pane::Rows, None) if focused_playlist(ui).is_some() => {
            tr("▲▼ move   OK show or hide   Menu its settings   ◀ Back pages")
        }
        (Pane::Rows, None) => tr("▲▼ move   OK change   ◀ Back pages"),
    }
}

/// The playlist of the focused row, if it is one.
fn focused_playlist(ui: Ui) -> Option<i64> {
    match state(ui, |s| {
        (s.pane() == Pane::Rows).then(|| s.focus_id()).flatten()
    })
    .flatten()?
    {
        RowId::Playlist(id) => Some(id),
        RowId::Key(_) => None,
    }
}

/// Stored playlist `id` (reactive: the library's version).
fn stored_playlist(ui: Ui, id: i64) -> Option<crate::data::Playlist> {
    ui.app.library.version.get();
    crate::runtime()
        .playlists()
        .ok()?
        .into_iter()
        .find(|p| p.id.0 == id)
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
        let page = move || state(ui, |s| s.pages().get(at.index()).copied()).flatten();
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
                        crate::settings_wiring::read(ui);
                        match mark() {
                            Mark::Focused => Mark::Focused.text(),
                            Mark::Current => rgb(ui.app.theme.with(|t| t.accent.light)),
                            Mark::Plain if page().is_some_and(|p| lit_page(&p)) => rgb(LIST_TEXT),
                            Mark::Plain => tint(WHITE, DIM_ALPHA),
                        }
                    }),
            )
    };
    virtual_list(move || state(ui, |s| s.pages().len()).unwrap_or(0), entry)
        // By page: a page that comes or goes (a keyboard attached) leaves
        // the others' entries, and a screen reader's place on one, alone.
        .keys(move || {
            state(ui, |s| s.pages().iter().map(|p| p.key).collect::<Vec<_>>()).unwrap_or_default()
        })
        .estimated_item_size(list_rows::pitch(size, ROW))
        .element_ref(&list)
        .w(move || size.px(PAGES_W))
}

fn rows_list(ui: Ui, rows: Memo<Vec<Row>>, size: Scale) -> impl IntoElement {
    let list = element_ref();
    let focused_index = computed(move || {
        let id = state(ui, |s| {
            (s.pane() == Pane::Rows).then(|| s.focus_id()).flatten()
        })
        .flatten()?;
        rows.with(|r| r.iter().position(|row| row.id() == Some(id)))
    });
    effect(move || {
        if let Some(i) = focused_index.get()
            && ui.keys_shown()
        {
            list.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    // Another page starts at its top (by key: the page shown keeps its
    // place when a page before it comes or goes).
    let page = computed(move || state(ui, |s| s.page().key));
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

/// The color of a palette value, for its swatch.
pub(super) fn swatch_color(ty: &ValueType, variant: &str) -> Option<u32> {
    let ValueType::Color { palette, .. } = ty else {
        return None;
    };
    let key = |name: &str| crate::theme::palette_key(name);
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
        let settings = crate::settings_wiring::read(ui);
        row().is_some_and(|r| matches!(status(&settings, &r), Status::Active | Status::ReadOnly))
    };
    let focused = move || ui.keys_shown() && focused_index.get() == Some(at.index());
    let mark = move || Mark::of(focused(), false);
    let playlist = move || match row() {
        Some(Row::Playlist(p)) => Some(p),
        _ => None,
    };
    let title = move || match row() {
        Some(Row::Heading {
            section,
            subsection,
        }) => words::heading(section, subsection).to_owned(),
        Some(Row::Playlist(p)) => stored_playlist(ui, p.id)
            .map(|p| p.name)
            .unwrap_or_default(),
        Some(r) => r.key().map_or("", words::label).to_owned(),
        None => String::new(),
    };
    // Each binding reads the row once: a row that went answers one read.
    let note_of = move |row: Option<Row>| match row {
        Some(Row::Playlist(p)) => stored_playlist(ui, p.id)
            .map(|p| crate::playlists_view::summary(ui, &p).0)
            .unwrap_or_default(),
        Some(r) => note(&crate::settings_wiring::read(ui), &r),
        None => String::new(),
    };
    let note = move || note_of(row());
    let current = move || {
        let settings = crate::settings_wiring::read(ui);
        match row() {
            Some(Row::Setting(s)) => {
                let value = settings.get(s.key).unwrap_or_else(|_| s.default_value());
                let swatch = match &value {
                    Value::Color(v) => swatch_color(&s.ty, v),
                    _ => None,
                };
                (words::value(s, &value), swatch)
            }
            Some(Row::Command(c)) => {
                ui.app.data_version.get();
                ui.app.library.version.get();
                (command_value(c.key), None)
            }
            _ => (String::new(), None),
        }
    };
    // A heading's row, a one-line row, or two lines with the note.
    let pitch = move || match row() {
        Some(Row::Heading { .. }) => HEADING_ROW,
        r if note_of(r).is_empty() => ROW,
        _ => ROW_TWO,
    };
    list_rows::row_with(size, pitch, mark)
        .role(move || match row() {
            Some(Row::Heading { .. }) => AccessibilityRole::Header,
            Some(Row::Playlist(_)) => AccessibilityRole::Checkbox,
            Some(Row::Setting(s)) if matches!(s.ty, ValueType::Bool { .. }) => {
                AccessibilityRole::Switch
            }
            _ => AccessibilityRole::Button,
        })
        .accessible(true)
        .label(move || {
            [title(), current().0, note()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .accessibility_state(move || {
            let checked = |on: bool| Some(if on { Checked::True } else { Checked::False });
            let settings = crate::settings_wiring::read(ui);
            match row() {
                None | Some(Row::Heading { .. }) => AccessibilityState::default(),
                Some(Row::Playlist(p)) => AccessibilityState {
                    checked: checked(p.enabled),
                    ..Default::default()
                },
                Some(r) => {
                    let lit = matches!(status(&settings, &r), Status::Active | Status::ReadOnly);
                    let toggle = match r {
                        Row::Setting(s) if matches!(s.ty, ValueType::Bool { .. }) => {
                            Some(settings.get(s.key) == Ok(Value::Bool(true)))
                        }
                        _ => None,
                    };
                    AccessibilityState {
                        disabled: !lit,
                        checked: toggle.and_then(checked),
                        ..Default::default()
                    }
                }
            }
        })
        .flex_row()
        .items_center()
        .justify_start()
        .gap(move || size.px(12.0))
        .on_press(move || {
            ui.touched();
            if let Some(id) = row().and_then(|r| r.id()) {
                update(ui, |screen, ctx| screen.tap(id, ctx));
            }
        })
        .on_long_press(move || {
            ui.touched();
            if let Some(id) = row().and_then(|r| r.id()) {
                update(ui, |screen, ctx| screen.hold_row(id, ctx));
            }
        })
        .on_context_menu(move |_: &nsh_core::ContextMenuEvent| {
            ui.touched();
            if let Some(id) = row().and_then(|r| r.id()) {
                update(ui, |screen, ctx| screen.hold_row(id, ctx));
            }
        })
        // A playlist's check box: shown in lists, or hidden. Other rows
        // keep it at no width (a recycled row changes kind).
        .child(check_box(
            ui,
            size,
            move || playlist().is_some(),
            move || playlist().is_some_and(|p| p.enabled),
            mark,
        ))
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
                    (_, true) => rgb(ui.app.theme.with(|t| t.accent.light)),
                    (Mark::Focused, false) => Mark::Focused.disabled(),
                    (_, false) => tint(WHITE, TEXT_DISABLED_ALPHA),
                }
            }),
        )
}

/// A check box, filled with the accent and ticked while `checked`;
/// nothing (no width) unless `shown`.
fn check_box(
    ui: Ui,
    size: Scale,
    shown: impl Fn() -> bool + Copy + 'static,
    checked: impl Fn() -> bool + Copy + 'static,
    mark: impl Fn() -> Mark + Copy + 'static,
) -> impl IntoElement {
    let side = move || size.px(if shown() { 22.0 } else { 0.0 });
    view()
        .test_id("settings-playlist-check")
        .w(side)
        .h(side)
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .border(move || px(if shown() { 2.0 } else { 0.0 }))
        .border_color(move || mark().text())
        .bg(move || {
            if checked() {
                rgb(ui.app.theme.with(|t| t.accent.main))
            } else {
                tint(WHITE, 0)
            }
        })
        .child(
            list_rows::second(size, move || if checked() { "✓" } else { "" }, SANS)
                .text_color(rgb(WHITE)),
        )
}

fn footer(ui: Ui, size: Scale) -> impl IntoElement {
    let description = move || {
        let settings = crate::settings_wiring::read(ui);
        // `player.pip_on_leave` follows the host's report (`settings_offer::offer`).
        ui.pip.with(|p| p.supported);
        let ctx = Ctx {
            settings: &settings,
            offer: &crate::settings_offer::offer_now,
        };
        state(ui, |s| match s.focused_row(&ctx) {
            Some(Row::Setting(st)) => tr(st.summary).replace('`', ""),
            Some(Row::Command(c)) => words::description(c.key)
                .unwrap_or_else(|| tr(c.summary))
                .to_owned(),
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
        .test_id("settings-help")
        .h(move || px(size.of(FOOTER_H) * crate::settings::text_factor().max(1.0)))
        .flex_shrink_0()
        .px(move || size.px(4.0))
        .number_of_lines(4)
}
