//! Setup confirmation before the runtime stages a chosen backup.

use std::cell::Cell;
use std::rc::Rc;

use futures_channel::oneshot;
use iptv_nav::Action;
use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;

use crate::Ui;
use crate::fonts::MEDIUM;
use crate::i18n::tr;
use crate::overlays::Overlay;
use crate::scale::Scale;
use crate::setup_view;
use crate::theme::{LIST_TEXT, RADIUS};
use crate::ui_kit::{self, FocusScroll};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    Schedule,
    Cancel,
}

#[derive(Clone)]
pub(crate) struct RestoreReview {
    filename: Option<String>,
    focus: Choice,
    reveal: bool,
    scrolling: FocusScroll,
    response: Rc<Cell<Option<oneshot::Sender<Choice>>>>,
}

pub(super) async fn confirm(ui: Ui, filename: Option<String>) -> bool {
    let (response, answer) = oneshot::channel();
    ui.restore_review.set(Some(RestoreReview {
        filename,
        focus: Choice::Cancel,
        reveal: false,
        scrolling: FocusScroll::new(),
        response: Rc::new(Cell::new(Some(response))),
    }));
    answer.await.is_ok_and(|choice| choice == Choice::Schedule)
}

pub(crate) fn is_open(ui: Ui) -> bool {
    ui.restore_review.with(Option::is_some)
}

fn focus(ui: Ui) -> Option<Choice> {
    ui.restore_review
        .with(|review| review.as_ref().map(|review| review.focus))
}

fn finish(ui: Ui, choice: Choice) -> bool {
    let response = ui
        .restore_review
        .with_untracked(|review| review.as_ref().and_then(|review| review.response.take()));
    if let Some(response) = response {
        ui.restore_review.set(None);
        let _ = response.send(choice);
        true
    } else {
        false
    }
}

pub(crate) fn back(ui: Ui) -> bool {
    finish(ui, Choice::Cancel)
}

pub(crate) fn key(ui: Ui, action: Action) {
    let choice = match action {
        Action::MoveUp if focus(ui) == Some(Choice::Schedule) => {
            scroll(ui, -1.0);
            return;
        }
        Action::MoveDown if focus(ui) == Some(Choice::Cancel) => {
            scroll(ui, 1.0);
            return;
        }
        Action::MoveLeft | Action::MoveUp => Choice::Schedule,
        Action::MoveRight | Action::MoveDown => Choice::Cancel,
        Action::Select => {
            if let Some(choice) = focus(ui) {
                finish(ui, choice);
            }
            return;
        }
        Action::Back => {
            back(ui);
            return;
        }
        _ => return,
    };
    ui.restore_review.update(|review| {
        if let Some(review) = review {
            review.focus = choice;
            review.reveal = true;
        }
    });
}

fn scroll(ui: Ui, direction: f32) {
    let mut scrolling = None;
    ui.restore_review.update(|review| {
        if let Some(review) = review {
            review.reveal = false;
            scrolling = Some(review.scrolling);
        }
    });
    if let Some(scrolling) = scrolling {
        scrolling.scroll_by(direction * Scale::new().of(96.0));
    }
}

pub(crate) fn layer(ui: Ui) -> impl IntoElement {
    when(move || is_open(ui), move || screen(ui))
}

fn screen(ui: Ui) -> View {
    let size = Scale::new();
    let scrolling = ui
        .restore_review
        .with_untracked(|review| review.as_ref().expect("an open restore review").scrolling);
    let schedule = move || focus(ui) == Some(Choice::Schedule);
    let cancel = move || focus(ui) == Some(Choice::Cancel);
    let reveal = move || {
        ui.keys_shown()
            && ui
                .restore_review
                .with(|review| review.as_ref().is_some_and(|review| review.reveal))
    };
    let intro = setup_view::intro(
        size,
        move || tr("Restore a backup"),
        move || tr("Review the selected backup before scheduling the restore."),
    );
    let content = view()
        .gap(move || size.px(14.0))
        .child(archive(ui, size))
        .child(
            ui_kit::body(size, move || {
                tr("When you reopen Patina, this backup replaces any current playlists, settings, favorites, groups, and watch history.")
            })
            .test_id("restore-warning")
            .text_color(rgb(LIST_TEXT)),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap(move || size.px(12.0))
                .child(scrolling.control(
                    setup_view::primary(
                        ui,
                        size,
                        move || tr("Schedule restore"),
                        schedule,
                        move || {
                            finish(ui, Choice::Schedule);
                        },
                    ),
                    move || reveal() && schedule(),
                ))
                .child(scrolling.control(
                    ui_kit::button(ui, size, move || tr("Cancel"), cancel, move || {
                        back(ui);
                    }),
                    move || reveal() && cancel(),
                )),
        );
    setup_view::frame(
        ui,
        size,
        move || tr("Library setup"),
        scrolling,
        setup_view::columns(size, intro, content),
    )
    .test_id("restore-review")
    .accessibility_elements_hidden(move || Overlay::RestoreReview.accessibility_covered(ui))
}

fn archive(ui: Ui, size: Scale) -> View {
    let filename = move || {
        ui.restore_review
            .with(|review| review.as_ref().and_then(|review| review.filename.clone()))
    };
    let shown = move || filename().unwrap_or_else(|| tr("Patina backup").to_owned());
    view()
        .test_id("restore-file")
        .rounded(px(RADIUS))
        .p(move || size.px(14.0))
        .flex_row()
        .items_center()
        .gap(move || size.px(14.0))
        .bg(move || rgb(ui.app.theme.with(|theme| theme.background.alt)))
        .child(
            svg(ImageSrc::svg_markup(
                "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' width='24' height='24' \
                 fill='none' stroke='currentColor' stroke-width='2' stroke-linejoin='round'>\
                 <path d='M3 4h18v5H3zM5 9v11h14V9m-9 5h4'/></svg>",
            ))
            .size(move || size.px(20.0))
            .flex_shrink_0()
            .text_color(move || rgb(ui.app.theme.with(|theme| theme.accent.light)))
            .accessibility_elements_hidden(true),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(move || size.px(4.0))
                .child(
                    ui_kit::body(size, shown)
                        .font_weight(MEDIUM)
                        .text_color(rgb(LIST_TEXT)),
                )
                .child(when(
                    move || filename().is_some(),
                    move || ui_kit::caption(size, move || tr("Patina backup")),
                )),
        )
}
