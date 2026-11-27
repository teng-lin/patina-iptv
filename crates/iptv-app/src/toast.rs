//! The app's persistent announcement and transient toast line.

use std::{cell::Cell, time::Duration};

use nsh_core::prelude::*;

use crate::{
    scale::Scale,
    theme::{LIST_TEXT, RADIUS},
    tint, ui_kit,
};

const NOTICE_FOR: Duration = Duration::from_millis(2_500);

thread_local! {
    /// The notice line of the layer made last (made with it, under its owner).
    static NOTICE: Cell<Option<Notice>> = const { Cell::new(None) };
}

/// The settings layer's notice line: its text, how many notices it has
/// shown, and the layer's owner, which its timers run under.
#[derive(Clone, Copy)]
pub(crate) struct Notice {
    pub(crate) text: Signal<Option<String>>,
    shown: Signal<u64>,
    owner: Owner,
}

impl Notice {
    /// Made with the layer, under its owner.
    pub(crate) fn new() -> Self {
        Self {
            text: signal(None),
            shown: signal(0),
            owner: Owner::current(),
        }
    }
}

pub(crate) fn layer() -> impl IntoElement {
    let state = Notice::new();
    NOTICE.with(|n| n.set(Some(state)));
    line("app-notice", move || state.text.get())
}

pub(crate) fn line(
    id: &'static str,
    text: impl Fn() -> Option<String> + Copy + 'static,
) -> impl IntoElement {
    let size = Scale::new();
    when(
        move || text().is_some(),
        move || {
            view()
                .test_id(id)
                .role(AccessibilityRole::Alert)
                .absolute()
                .left(px(0.0))
                .w_full()
                .bottom(move || size.px(28.0))
                .items_center()
                .pointer_events(PointerEvents::None)
                .child(
                    ui_kit::body(size, move || text().unwrap_or_default())
                        .px(move || size.px(18.0))
                        .py(move || size.px(12.0))
                        .rounded(px(RADIUS))
                        .text_color(rgb(LIST_TEXT))
                        .bg(tint(0x000000, 0xC0)),
                )
        },
    )
}

/// Shows `text` briefly at the bottom of the screen.
pub(crate) fn notify(text: &str) {
    announce_for_accessibility(text);
    let Some(notice) = NOTICE.with(Cell::get) else {
        return;
    };
    // A layer already gone (its window closed) shows nothing more.
    if !notice.text.try_set(Some(text.to_owned())) {
        return;
    }
    // The notice is told apart by when it was shown, not by its text: an
    // earlier notice with the same words (blocking a channel, then its
    // group) must not cut this one short when its own time is up.
    notice.shown.update(|n| *n = n.wrapping_add(1));
    let shown = notice.shown.get_untracked();
    // Under the layer's owner, not whatever runs now (a row that closes, a
    // task): NSH cancels the timer with its owner, so it neither stops early
    // nor writes the line after the layer is gone. Its handle is not needed.
    notice.owner.run(|| {
        drop(set_timeout(NOTICE_FOR, move || {
            if notice.shown.get_untracked() == shown {
                notice.text.set(None);
            }
        }));
    });
}
