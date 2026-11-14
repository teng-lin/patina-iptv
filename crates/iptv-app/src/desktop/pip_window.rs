//! The desktops' picture-in-picture window (`pip.rs`,
//! `iptv_core::pip_window`): a window of the app's own titled as browsers
//! title theirs, so the desktop's rules for those apply (Hyprland's
//! Omarchy defaults float, pin and place a window titled so; GNOME and
//! KDE users keep similar ones). It asks to float above the others on
//! every workspace (macOS's floating level; X11's keep-above,
//! `desktop/x11.rs`; on Wayland only the compositor can), without a frame,
//! resizable (keeping the stream's shape where the platform can: macOS),
//! opening 600 points wide in the stream's shape at the bottom right of
//! the display where the platform places windows (macOS).
//!
//! It shows the full-screen player's picture ([`VIEW`], `mpv/pip.rs`: the
//! same player, no second stream) and, while the pointer moves over it,
//! play/pause, back to the app and close. Dragging it moves it (a window
//! drag region; elsewhere the compositor's own move, Super+drag); a double
//! click or Escape brings the video back to the app; the system closing
//! it (Super+W, Alt+F4) closes it, as its ✕ does. Meanwhile the main
//! window shows [`placeholder`] where the video was.

use crate::i18n::tr;

use std::cell::Cell;
use std::time::Duration;

use iptv_core::pip_window::{self as pw, Exit, Hover};
use nsh_core::desktop::{
    WindowAttributes, WindowCloseCause, WindowFrameStyle, WindowLevel, WindowPosition, Workspaces,
};
use nsh_core::element::desktop::{OpenChange, display};
use nsh_core::geometry::point;
use nsh_core::prelude::*;

use crate::Ui;
use crate::play_bar::icons::{self, Icon};
use crate::tint;

/// The window's video view, a native component of the desktop players
/// (`mpv/pip.rs`).
pub use iptv_player_mpv::PIP_VIEW as VIEW;
/// The window's id.
pub const ID: &str = "pip";

const WHITE: u32 = 0xFFFFFF;
/// Its distance from the display's usable edges where it can be placed.
const MARGIN: f32 = 24.0;

/// The window, open while the player shows in it.
pub fn window(ui: Ui) -> impl IntoElement {
    when(move || crate::pip::in_window(ui), move || open(ui))
}

fn open(ui: Ui) -> impl IntoElement {
    let (w, h) = untracked(|| pw::size_for(ui.player.main().size.get()));
    let mut window = nsh_core::element::desktop::window()
        .id(ID)
        .title(pw::TITLE)
        .default_size(nsh_core::Size::new(px(w), px(h)))
        .attributes(attributes(w, h))
        .on_open_change(move |change: &OpenChange| closed(ui, change.reason))
        .content(move || content(ui));
    if let Some(at) = untracked(|| corner(w, h)) {
        window = window.default_position(at);
    }
    window
}

fn attributes(w: f32, h: f32) -> WindowAttributes {
    let mut a = WindowAttributes::default();
    a.frame = Some(WindowFrameStyle::None);
    a.level = Some(WindowLevel::Floating);
    a.workspaces = Some(Workspaces::All);
    a.minimizable = Some(false);
    a.maximizable = Some(false);
    a.close_on_escape = Some(true);
    a.taskbar = Some(false);
    // The main window keeps the keys, as with a browser's.
    a.activate = Some(false);
    a.min_size = Some(Some(nsh_core::Size::new(px(w * 0.4), px(h * 0.4))));
    a
}

/// The bottom right corner of the main window's display.
fn corner(w: f32, h: f32) -> Option<WindowPosition> {
    let state = super::with_window(|m| m.state()).flatten()?;
    let area = display(state.display?)?.visible_bounds;
    let x = area.origin.x.0 + area.size.width.0 - w - MARGIN;
    let y = area.origin.y.0 + area.size.height.0 - h - MARGIN;
    Some(WindowPosition::At(point(
        px(x.max(area.origin.x.0)),
        px(y.max(area.origin.y.0)),
    )))
}

/// The user or the system closed the window.
fn closed(ui: Ui, cause: WindowCloseCause) {
    let how = match cause {
        WindowCloseCause::Escape => Exit::BackToApp,
        WindowCloseCause::ClosePress => Exit::Close,
        // The app closes with the main window.
        _ => {
            log::info!("iptv pip: the window closed ({cause:?})");
            Exit::BackToApp
        }
    };
    crate::pip::exit(ui, how);
}

fn content(ui: Ui) -> impl IntoElement {
    let hover = signal(Hover::default());
    let shown = signal(false);
    let drags = signal(true);
    let last = Cell::new(None::<(f32, f32)>);
    view()
        .flex_1()
        .bg(rgb(0x000000))
        .on_mouse_move(move |e: &MouseEvent| {
            let at = (e.page_position.x.0, e.page_position.y.0);
            // A redraw under a resting pointer reports it again.
            let moved = last
                .replace(Some(at))
                .is_none_or(|(x, y)| (x - at.0).abs() + (y - at.1).abs() >= 2.0);
            if moved {
                pointer_moved(ui, hover, shown);
            }
        })
        .on_mouse_leave(move |_: &MouseEvent| {
            hover.update(Hover::left);
            shown.set(false);
        })
        .child(native(VIEW).absolute().inset_0())
        .child(
            view()
                .test_id("pip-video")
                .absolute()
                .inset_0()
                .label(move || tr("Video"))
                .window_drag(move || drags.get())
                .on_press(move |e: &PressEvent| clicked(ui, hover, drags, e.clicks)),
        )
        .child(when(move || shown.get(), move || controls(ui)))
}

/// The pointer moved over the window: the controls show, and hide after
/// a while without movement.
fn pointer_moved(ui: Ui, hover: Signal<Hover>, shown: Signal<bool>) {
    let now = ui.now_ms();
    hover.update(|h| h.moved(now));
    shown.set(true);
    // When they hide unless it moves again (`Hover::hide_at_ms`).
    let _task = set_timeout(Duration::from_millis(pw::HIDE_AFTER_MS), move || {
        let now = ui.now_ms();
        if untracked(|| hover.try_get()).is_some_and(|h| !h.shows(now)) {
            shown.try_set(false);
        }
    });
}

/// A click on the video: the second of a double click goes back to the
/// app; after the first, presses are clicks (not drags) for a moment.
fn clicked(ui: Ui, hover: Signal<Hover>, drags: Signal<bool>, clicks: u32) {
    let now = ui.now_ms();
    let mut double = false;
    hover.update(|h| double = h.click(now));
    if double || clicks >= 2 {
        crate::pip::exit(ui, Exit::BackToApp);
        return;
    }
    drags.set(false);
    // Once it can no longer become a double click
    // (`Hover::drags_again_at_ms`).
    let _task = set_timeout(Duration::from_millis(pw::DOUBLE_CLICK_MS + 1), move || {
        let now = ui.now_ms();
        if untracked(|| hover.try_get()).is_some_and(|h| h.drags(now)) {
            drags.try_set(true);
        }
    });
}

/// Back to the app at the top left, close at the top right, play/pause in
/// the middle.
fn controls(ui: Ui) -> impl IntoElement {
    let paused = move || ui.paused.get();
    view()
        .absolute()
        .inset_0()
        .pointer_events(PointerEvents::BoxNone)
        .bg(tint(0x000000, 0x40))
        .child(
            button(
                "pip-back",
                || tr("Back to the app"),
                || Icon::BackToApp,
                32.0,
                move || {
                    crate::pip::exit(ui, Exit::BackToApp);
                },
            )
            .absolute()
            .top(px(8.0))
            .left(px(8.0)),
        )
        .child(
            button(
                "pip-close",
                || tr("Close"),
                || Icon::Close,
                32.0,
                move || {
                    crate::pip::exit(ui, Exit::Close);
                },
            )
            .absolute()
            .top(px(8.0))
            .right(px(8.0)),
        )
        .child(
            view()
                .absolute()
                .inset_0()
                .items_center()
                .justify_center()
                .pointer_events(PointerEvents::BoxNone)
                .child(button(
                    "pip-play",
                    move || {
                        if paused() { tr("Play") } else { tr("Pause") }
                    },
                    move || if paused() { Icon::Play } else { Icon::Pause },
                    48.0,
                    move || ui.paused.update(|p| *p = !*p),
                )),
        )
}

fn button(
    id: &'static str,
    name: impl Fn() -> &'static str + 'static,
    icon: impl Fn() -> Icon + 'static,
    size: f32,
    act: impl Fn() + 'static,
) -> View {
    view()
        .role(AccessibilityRole::Button)
        .test_id(id)
        .w(px(size))
        .h(px(size))
        .rounded(px(size / 2.0))
        .items_center()
        .justify_center()
        .bg(tint(0x000000, 0x99))
        .hover(|s| s.bg(tint(0x303030, 0xE0)))
        .label(name)
        .on_press(move || {
            log::info!("iptv pip window: {id}");
            act();
        })
        .child(
            svg(move || icons::src(icon()))
                .size(px(size * 0.55))
                .text_color(rgb(WHITE)),
        )
}

/// The main window, where the video was while it plays in the window: a
/// placeholder with a way back.
pub fn placeholder(ui: Ui) -> impl IntoElement {
    when(
        move || crate::pip::in_window(ui),
        move || {
            let rect = move || crate::video::layout_rect(ui, 0);
            let size = crate::scale::Scale::new();
            view()
                .test_id("pip-placeholder")
                .accessibility_elements_hidden(move || ui.any_overlay_open())
                .absolute()
                .left(move || px(rect().0))
                .top(move || px(rect().1))
                .w(move || px(rect().2))
                .h(move || px(rect().3))
                .bg(rgb(0x101014))
                .items_center()
                .justify_center()
                .gap(px(10.0))
                .child(
                    svg(icons::src(Icon::PictureInPicture))
                        .size(px(32.0))
                        .text_color(tint(WHITE, 0xB0)),
                )
                .child(crate::ui_kit::caption(
                    size,
                    tr("Playing in picture-in-picture"),
                ))
                .child(
                    crate::ui_kit::button(
                        ui,
                        size,
                        tr("Back to the app"),
                        || false,
                        move || {
                            crate::pip::exit(ui, Exit::BackToApp);
                        },
                    )
                    .test_id("pip-return"),
                )
        },
    )
}
