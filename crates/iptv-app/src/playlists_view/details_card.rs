//! The provider details form on screen: one row per detail (logins and MAC
//! addresses hidden), tr("Check and save"), and what the check is doing or why
//! it refused the new details.

use crate::runtime::model::SourceId;
use nsh_core::prelude::*;

use super::details::Param;
use super::{ERROR, WHITE, mark, state, update, words};
use crate::data::Activity;
use crate::i18n::{tr, trf};
use crate::list_rows::{self, ROW_TWO};
use crate::scale::Scale;
use crate::ui_kit;
use crate::{Ui, rgb};

pub(super) fn card(ui: Ui, size: Scale, id: SourceId) -> impl IntoElement {
    let Some((kind, params)) = state(ui, |s| s.details().map(|f| (f.kind(), f.params()))).flatten()
    else {
        return view();
    };
    let checking = move || state(ui, |s| s.details().is_some_and(|f| f.checking)).unwrap_or(false);
    // The latest first: a notice from the last key, then the check.
    let status = move || {
        if let Some(n) = state(ui, |s| s.notice()).flatten() {
            return (words::notice(n).to_owned(), true);
        }
        match ui.library.activity.with(|a| a.get(&id).cloned()) {
            Some(Activity::Checking(progress)) if checking() => (
                trf(
                    "Checking: {stage}",
                    &[(
                        "stage",
                        (&crate::wizard_view::words::stage(progress)).into(),
                    )],
                ),
                false,
            ),
            Some(Activity::Rejected(f)) if !checking() => (words::rejected(f), true),
            _ => (String::new(), false),
        }
    };
    let row = move |p: Param| {
        let focused = move || state(ui, |s| s.details().is_some_and(|f| f.focus == Some(p)));
        let value = move || {
            state(ui, |s| s.details().map(|f| words::param_value(f, p)))
                .flatten()
                .unwrap_or_default()
        };
        let error = move || {
            state(ui, |s| s.details().and_then(|f| f.error))
                .flatten()
                .filter(|(at, _)| *at == p)
                .map(|(_, e)| crate::wizard_view::words::field_error(e))
        };
        let mark = move || mark(ui, focused().unwrap_or(false));
        // Two lines, and a third for a problem with the detail.
        let pitch = move || ROW_TWO + if error().is_some() { 18.0 } else { 0.0 };
        list_rows::row_with(size, pitch, mark)
            .on_press(move || {
                ui.touched();
                update(ui, |s| s.edit_param(p));
            })
            .child(ui_kit::detail(size, words::param(p)).text_color(move || mark().secondary()))
            .child(ui_kit::label(size, value).text_color(move || mark().text()))
            .child(when(
                move || error().is_some(),
                move || {
                    ui_kit::caption(size, move || error().unwrap_or(""))
                        .number_of_lines(1)
                        .text_color(rgb(ERROR))
                },
            ))
    };
    let check_focused =
        move || state(ui, |s| s.details().is_some_and(|f| f.focus.is_none())).unwrap_or(false);
    let check_label = move || {
        if checking() {
            tr("Checking…")
        } else {
            tr("Check and save")
        }
        .to_owned()
    };
    super::card(ui, size)
        .child(ui_kit::title(size, words::details_title(kind)))
        .child(ui_kit::body(
            size,
            tr("New details are tried first: the playlist keeps its old ones and its channels unless the new ones work."),
        ))
        .child(view().h(move || size.px(4.0)))
        .children(params.iter().copied().map(row))
        .child(when(
            move || state(ui, |s| s.details().is_some_and(|f| {
                f.params().iter().any(|p| p.is_link() && f.value(*p).trim().to_ascii_lowercase().starts_with("http://"))
            })).unwrap_or(false),
            move || ui_kit::caption(size, tr("Not encrypted: HTTP can expose your login and viewing activity.")),
        ))
        .child(ui_kit::action(ui, size, check_label, check_focused, WHITE, move || {
            update(ui, |s| s.check());
        }))
        .child(
            ui_kit::body(size, move || status().0)
                .text_color(move || if status().1 { rgb(ERROR) } else { ui_kit::secondary() }),
        )
}
