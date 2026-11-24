//! The question before a desktop quits during a capture (#275): quitting
//! stops the recording, so Patina asks first. On Windows and Linux it asks
//! when the main window is closed, so no recorder runs without a window;
//! on macOS the window closes and the recording goes on with the Dock icon,
//! and it asks on Quit. Stop and quit ends each capture as the viewer's
//! Stop does, keeping what was recorded, then quits; Keep recording leaves
//! everything as it was.

use crate::i18n::tr;
use crate::{Ui, ui_kit};
use iptv_nav::Action;
use nsh_core::prelude::*;

/// The question's answers, the focused one in the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    StopAndQuit,
    Keep,
}

/// Asks, the window shown and in front (on macOS it may be hidden).
pub(crate) fn open(ui: Ui) {
    super::show_main();
    ui.quit_confirm.set(Some(Choice::Keep));
}

pub(crate) fn is_open(ui: Ui) -> bool {
    ui.quit_confirm.with(Option::is_some)
}

/// Back keeps recording.
pub(crate) fn back(ui: Ui) -> bool {
    ui.quit_confirm.set(None);
    true
}

fn choose(ui: Ui, choice: Choice) {
    ui.quit_confirm.set(None);
    if choice == Choice::StopAndQuit {
        super::stop_recordings_and_quit(ui);
    }
}

pub(crate) fn key(ui: Ui, action: Action) {
    match action {
        Action::MoveLeft | Action::MoveUp => ui.quit_confirm.set(Some(Choice::StopAndQuit)),
        Action::MoveRight | Action::MoveDown => ui.quit_confirm.set(Some(Choice::Keep)),
        Action::Select => {
            if let Some(choice) = ui.quit_confirm.get_untracked() {
                choose(ui, choice);
            }
        }
        Action::Back => {
            back(ui);
        }
        _ => {}
    }
}

pub(crate) fn layer(ui: Ui) -> impl IntoElement {
    when(move || is_open(ui), move || sheet(ui))
}

fn sheet(ui: Ui) -> View {
    let size = crate::scale::Scale::new();
    let focused = move |choice| move || ui.quit_confirm.get() == Some(choice);
    view()
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .bg(crate::tint(0, 190))
        .test_id("quit-confirm")
        .child(
            ui_kit::card(ui, size, 560.0)
                .role(AccessibilityRole::Alert)
                .child(ui_kit::title(size, move || tr("Patina is recording")))
                .child(ui_kit::body(size, move || {
                    tr("Quitting stops the recording. What was recorded until then is kept.")
                }))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap(move || size.px(12.0))
                        .pt(move || size.px(8.0))
                        .child(
                            ui_kit::button(
                                ui,
                                size,
                                move || tr("Stop recording and quit"),
                                focused(Choice::StopAndQuit),
                                move || choose(ui, Choice::StopAndQuit),
                            )
                            .test_id("quit-confirm-stop"),
                        )
                        .child(
                            ui_kit::button(
                                ui,
                                size,
                                move || tr("Keep recording"),
                                focused(Choice::Keep),
                                move || choose(ui, Choice::Keep),
                            )
                            .test_id("quit-confirm-keep"),
                        ),
                ),
        )
}
