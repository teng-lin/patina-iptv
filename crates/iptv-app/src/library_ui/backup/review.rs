//! Setup confirmation before the runtime stages a chosen backup.

use std::cell::Cell;
use std::rc::Rc;

use futures_channel::oneshot;
use iptv_nav::Action;
use nsh_core::prelude::*;

use crate::Ui;
use crate::kit::fonts::MEDIUM;
use crate::kit::i18n::tr;
use crate::kit::overlay::Overlay;
use crate::kit::scale::Scale;
use crate::kit::ui_kit::icons::{self, Icon};
use crate::kit::ui_kit::{self, FocusScroll};
use crate::library_ui::setup_view;
use crate::theme::{LIST_TEXT, RADIUS};

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

pub(super) async fn confirm(screens: crate::ScreensUi, filename: Option<String>) -> bool {
    let (response, answer) = oneshot::channel();
    screens.restore_review.set(Some(RestoreReview {
        filename,
        focus: Choice::Cancel,
        reveal: false,
        scrolling: FocusScroll::new(),
        response: Rc::new(Cell::new(Some(response))),
    }));
    answer.await.is_ok_and(|choice| choice == Choice::Schedule)
}

pub(crate) fn is_open(screens: crate::ScreensUi) -> bool {
    screens.restore_review.with(Option::is_some)
}

fn focus(screens: crate::ScreensUi) -> Option<Choice> {
    screens
        .restore_review
        .with(|review| review.as_ref().map(|review| review.focus))
}

fn finish(screens: crate::ScreensUi, choice: Choice) -> bool {
    let response = screens
        .restore_review
        .with_untracked(|review| review.as_ref().and_then(|review| review.response.take()));
    if let Some(response) = response {
        screens.restore_review.set(None);
        let _ = response.send(choice);
        true
    } else {
        false
    }
}

pub(crate) fn back(screens: crate::ScreensUi) -> bool {
    finish(screens, Choice::Cancel)
}

pub(crate) fn key(ui: Ui, action: Action) {
    let choice = match action {
        Action::MoveUp if focus(ui.screens) == Some(Choice::Schedule) => {
            scroll(ui.screens, -1.0);
            return;
        }
        Action::MoveDown if focus(ui.screens) == Some(Choice::Cancel) => {
            scroll(ui.screens, 1.0);
            return;
        }
        Action::MoveLeft | Action::MoveUp => Choice::Schedule,
        Action::MoveRight | Action::MoveDown => Choice::Cancel,
        Action::Select => {
            if let Some(choice) = focus(ui.screens) {
                finish(ui.screens, choice);
            }
            return;
        }
        Action::Back => {
            back(ui.screens);
            return;
        }
        _ => return,
    };
    ui.screens.restore_review.update(|review| {
        if let Some(review) = review {
            review.focus = choice;
            review.reveal = true;
        }
    });
}

fn scroll(screens: crate::ScreensUi, direction: f32) {
    let mut scrolling = None;
    screens.restore_review.update(|review| {
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
    when(move || is_open(ui.screens), move || screen(ui))
}

fn screen(ui: Ui) -> View {
    let size = Scale::new();
    let scrolling = ui
        .screens
        .restore_review
        .with_untracked(|review| review.as_ref().expect("an open restore review").scrolling);
    let schedule = move || focus(ui.screens) == Some(Choice::Schedule);
    let cancel = move || focus(ui.screens) == Some(Choice::Cancel);
    let reveal = move || {
        ui.keys_shown()
            && ui
                .screens
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
                            finish(ui.screens, Choice::Schedule);
                        },
                    ),
                    move || reveal() && schedule(),
                ))
                .child(scrolling.control(
                    ui_kit::button(ui, size, move || tr("Cancel"), cancel, move || {
                        back(ui.screens);
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
        ui.screens
            .restore_review
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
            svg(icons::src(Icon::Archive))
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
