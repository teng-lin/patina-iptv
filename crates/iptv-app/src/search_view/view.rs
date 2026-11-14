//! The search screen: the field, the recent searches and the results, in
//! the lists' look (`list_rows.rs`): channel names in Roboto and programme
//! titles in Roboto Condensed at the guide's sizes, the focus its light
//! pill.

use crate::i18n::tr;

use super::*;
use crate::fonts::{CAPTION, CONDENSED, SANS};
use crate::list_rows::{self, Mark, ROW, ROW_TWO};
use crate::scale::Scale;
use crate::ui_kit;

fn state<R>(ui: Ui, f: impl FnOnce(&Screen) -> R) -> Option<R> {
    ui.search.with(|s| s.as_ref().map(f))
}

/// A row's mark: the focus while the keys are shown.
fn mark(ui: Ui, focused: bool) -> Mark {
    Mark::of(focused && ui.keys_shown(), false)
}

pub fn layer(ui: Ui) -> impl IntoElement {
    // Back from playback (full screen left): the suspended screen returns.
    effect(move || {
        let left = ui.mode.get() != ViewMode::Fullscreen;
        if left
            && ui
                .search
                .with_untracked(|s| s.as_ref().is_some_and(|s| s.suspended))
        {
            resume(ui);
        }
    });
    // New guide data while open: fresh indexes, the query searched again.
    effect(move || {
        ui.data_version.get();
        if ui.search.with_untracked(Option::is_some) {
            prepare(ui);
            search_again(ui);
        }
    });
    when(move || is_open(ui), move || screen(ui))
}

fn screen(ui: Ui) -> impl IntoElement {
    let size = Scale::new();
    let idle = computed(move || state(ui, |s| s.state.phase() == Phase::Idle).unwrap_or(true));
    ui_kit::screen(ui, size)
        .test_id("search-screen")
        .accessibility_elements_hidden(move || {
            crate::overlays::Overlay::Search.accessibility_covered(ui)
        })
        .child(header(ui, size))
        .child(ui_kit::caption(size, move || hint(ui)))
        .child(
            view().flex_1().child(
                when(move || idle.get(), move || history(ui, size))
                    .otherwise(move || results(ui, size)),
            ),
        )
}

fn hint(ui: Ui) -> &'static str {
    let idle = state(ui, |s| s.state.phase() == Phase::Idle).unwrap_or(true);
    // Search and add: a result goes to multiview.
    let add = pick::picking(ui);
    match (ui.keys_shown(), idle) {
        (true, true) => "OK type   ▼ recent searches   Back close",
        (true, false) if add => {
            "▲▼ move   ◀▶ channels / programmes   OK add   Back keyboard, then close"
        }
        (true, false) => {
            "▲▼ move   ◀▶ channels / programmes   OK watch   Back keyboard, then close"
        }
        (false, true) => "Type a channel or programme, or tap a recent search",
        (false, false) if add => "Tap a result to add its channel to multiview",
        (false, false) => "Tap a result to watch it",
    }
}

fn button(ui: Ui, size: Scale, label: &'static str, act: fn(Ui)) -> impl IntoElement {
    ui_kit::button(ui, size, move || tr(label), || false, move || act(ui))
}

fn header(ui: Ui, size: Scale) -> impl IntoElement {
    let field = element_ref();
    let want_keyboard = computed(move || {
        state(ui, |s| {
            s.state.keyboard_visible() && *s.state.focus() == Focus::Input
        })
        .unwrap_or(false)
    });
    effect(move || {
        if want_keyboard.get() {
            field.focus();
        } else {
            field.blur();
        }
    });
    // The platform closed the keyboard (Back on Android hides it first).
    let was_up = Rc::new(Cell::new(false));
    effect(move || {
        let up = keyboard_height() > px(0.0);
        if was_up.replace(up) && !up {
            apply(ui, SearchAction::KeyboardHidden);
        }
    });
    let field_focused = move || state(ui, |s| *s.state.focus() == Focus::Input).unwrap_or(false);
    view()
        .flex_row()
        .items_center()
        .gap(move || size.px(12.0))
        .child(ui_kit::title(size, move || {
            if pick::picking(ui) {
                tr("Search and add")
            } else {
                tr("Search")
            }
        }))
        .child(
            ui_kit::field(size, input())
                .element_ref(&field)
                .flex_1()
                .value(move || state(ui, |s| s.state.query().to_owned()).unwrap_or_default())
                .placeholder(tr("Channel, number or programme"))
                .auto_focus(want_keyboard.get_untracked())
                .auto_capitalize(AutoCapitalize::None)
                .auto_correct(false)
                .return_key_type(ReturnKeyType::Search)
                .test_id("search-field")
                .bg(move || ui_kit::field_fill(ui, field_focused()))
                .border(move || size.px(2.0))
                .px(move || size.px(12.0))
                .py(move || size.px(ui_kit::FIELD_PAD_Y - 2.0))
                .border_color(move || ui_kit::field_border(ui, field_focused()))
                .on_change(move |text: &str| {
                    let same = state(ui, |s| s.state.query() == text).unwrap_or(true);
                    if !same {
                        apply(ui, SearchAction::Type(text.to_owned()));
                    }
                })
                .on_submit(move |_: &str| apply(ui, SearchAction::Ok))
                .on_focus(move || {
                    if !state(ui, |s| s.state.keyboard_visible()).unwrap_or(true) {
                        apply(ui, SearchAction::ShowKeyboard);
                    }
                })
                .on_blur(move || {
                    if state(ui, |s| s.state.keyboard_visible()).unwrap_or(false) {
                        apply(ui, SearchAction::KeyboardHidden);
                    }
                }),
        )
        .child(button(ui, size, "Clear", |ui| {
            apply(ui, SearchAction::Clear)
        }))
        .child(button(ui, size, "Close", close))
}

fn history(ui: Ui, size: Scale) -> impl IntoElement {
    let count = move || state(ui, |s| s.state.history().len()).unwrap_or(0);
    let entry =
        move |i: usize| state(ui, |s| s.state.history().entries().get(i).cloned()).flatten();
    let row = move |at: Position| {
        let focused = move || {
            let e = entry(at.index());
            e.is_some() && state(ui, |s| s.state.focus().clone()) == e.map(Focus::History)
        };
        let mark = move || mark(ui, focused());
        list_rows::row(size, ROW, mark)
            .on_press(move || {
                ui.touched();
                if let Some(e) = entry(at.index()) {
                    apply(ui, SearchAction::Pick(Focus::History(e)));
                }
            })
            .child(
                list_rows::name(size, move || entry(at.index()).unwrap_or_default())
                    .text_color(move || mark().text()),
            )
    };
    view()
        .flex_1()
        .gap(move || size.px(8.0))
        .child(ui_kit::heading(size, move || {
            if !options(&crate::settings::read(ui)).show_history {
                ""
            } else if count() == 0 {
                tr("No recent searches yet")
            } else {
                tr("Recent searches")
            }
        }))
        .child(
            virtual_list(count, row)
                .estimated_item_size(list_rows::pitch(size, ROW))
                .flex_1(),
        )
}

fn tab_button(ui: Ui, tab: Tab, size: Scale) -> impl IntoElement {
    let selected = move || state(ui, |s| s.state.tab() == tab).unwrap_or(false);
    let focused =
        move || selected() && state(ui, |s| *s.state.focus() == Focus::Tabs) == Some(true);
    let label =
        move || state(ui, |s| tab_label(tab, s.state.count(tab), s.programmes)).unwrap_or_default();
    ui_kit::tab(ui, size, label, selected, focused, move || {
        apply(ui, SearchAction::ShowTab(tab));
    })
}

fn status(ui: Ui) -> String {
    state(ui, |s| match s.state.phase() {
        Phase::Typing => tr("Searching…").to_owned(),
        Phase::NoResults => format!("Nothing found for “{}”", s.state.query().trim()),
        Phase::Idle | Phase::Results => String::new(),
    })
    .unwrap_or_default()
}

/// The result at `i` of the shown tab, with its identity.
fn result_at(ui: Ui, i: usize) -> Option<(ResultId, Option<ProgrammeHit>)> {
    state(ui, |s| match s.state.tab() {
        Tab::Channels => s
            .state
            .channels()
            .get(i)
            .map(|h| (ResultId::Channel(h.channel), None)),
        Tab::Programs => s.state.programmes().get(i).map(|h| {
            let id = ResultId::Programme {
                channel: h.channel,
                start: h.start,
            };
            (id, Some(h.clone()))
        }),
    })
    .flatten()
}

fn results(ui: Ui, size: Scale) -> impl IntoElement {
    let list = element_ref();
    let count = move || state(ui, |s| s.state.count(s.state.tab())).unwrap_or(0);
    let focused_index = computed(move || {
        let focus = state(ui, |s| s.state.focus().clone())?;
        (0..count())
            .find(|&i| result_at(ui, i).map(|r| Focus::Result(r.0)).as_ref() == Some(&focus))
    });
    effect(move || {
        if let Some(i) = focused_index.get()
            && ui.keys_shown()
        {
            list.scroll_to_index(i as u32, percentage(0.5), px(0.0), false);
        }
    });
    view()
        .flex_1()
        .gap(move || size.px(8.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(move || size.px(8.0))
                .child(tab_button(ui, Tab::Channels, size))
                .child(tab_button(ui, Tab::Programs, size))
                .child(
                    ui_kit::caption(size, move || status(ui))
                        .flex_1()
                        .px(move || size.px(12.0))
                        .number_of_lines(1),
                ),
        )
        .child(
            virtual_list(count, move |at| result_row(ui, at, focused_index, size))
                .estimated_item_size(list_rows::pitch(size, ROW_TWO))
                .element_ref(&list)
                .flex_1(),
        )
}

fn result_row(
    ui: Ui,
    at: Position,
    focused_index: Memo<Option<usize>>,
    size: Scale,
) -> impl IntoElement {
    let item = move || result_at(ui, at.index());
    // Programme titles in the grid's type, channel names in the column's.
    let family = move || match item() {
        Some((_, Some(_))) => CONDENSED,
        _ => SANS,
    };
    let mark = move || mark(ui, focused_index.get() == Some(at.index()));
    let line1 = move || match item() {
        Some((ResultId::Channel(c), _)) => {
            let star = ui.favorites.with(|f| f.contains(&c));
            format!(
                "{}   {}{}",
                crate::schedule().channel_number(c),
                if star { "★ " } else { "" },
                channel_name(c)
            )
        }
        // A guide stored before badges were folded at import.
        Some((_, Some(h))) => iptv_epg::fold_superscripts(&h.title).into_owned(),
        _ => String::new(),
    };
    let line2 = move || match item() {
        Some((ResultId::Channel(c), _)) if ui.playing.get() == Some(c) => tr("Playing").to_owned(),
        Some((_, Some(h))) => {
            let epoch = guide_epoch();
            format!(
                "{} · {}–{}",
                channel_name(h.channel),
                clock(display_minute(h.start, epoch)),
                clock(display_minute(h.end, epoch))
            )
        }
        _ => String::new(),
    };
    let marker = move || match item() {
        Some((_, Some(h))) => badge(&h),
        _ => String::new(),
    };
    list_rows::row(size, ROW_TWO, mark)
        .flex_row()
        .items_center()
        .justify_start()
        .gap(px(16.0))
        .on_press(move || {
            ui.touched();
            if let Some((id, _)) = item() {
                apply(ui, SearchAction::Pick(Focus::Result(id)));
            }
        })
        .child(
            view()
                .flex_1()
                .child(
                    list_rows::name(size, line1)
                        .font_family(family)
                        .text_color(move || mark().text()),
                )
                .child(list_rows::second(size, line2, SANS).text_color(move || mark().secondary())),
        )
        .child(
            list_rows::small(size, marker, CAPTION).text_color(move || match item() {
                Some((_, Some(h))) if h.airing == Airing::Live && mark() != Mark::Focused => {
                    rgb(ui.theme.with(|t| t.accent.light))
                }
                _ => mark().secondary(),
            }),
        )
}
