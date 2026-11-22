//! Wheels and touchpads pan the timeline on the desktops. NSH hands
//! wheels to scroll views only, so the program area sits in a horizontal
//! scroll view whose content is far wider than it, with the grid stuck to
//! its left edge (`sticky`): a horizontal wheel, a two-finger sideways
//! swipe and Shift + wheel scroll it, and the changes of its offset pan the
//! timeline ([`Pan::wheeled`]); a plain vertical wheel goes to the channel
//! list inside, along its axis. Once the scroll rests it goes back to the
//! middle. Touch drags are the grid's own pan (`pan.rs`), which takes them
//! before this scroll view can.
//!
//! Android and iPadOS are touch first: there the content is as wide as
//! the area and nothing scrolls, so a finger's drag is always the grid's
//! pan (a UIKit scroll view would take it first). A browser TV has only a
//! remote: no wheel to take, and its platform does not apply a scroll
//! view's initial content offset or sticky children, so the padded
//! content would sit 20,000 px off screen there.

use nsh_core::prelude::*;

use super::Pan;

/// Room to scroll either way before the view goes back to the middle.
const PAD: f32 = if crate::host::REMOTE_KEYS || crate::host::IPAD {
    0.0
} else {
    20_000.0
};

pub(super) fn trackball(pan: Pan, grid: impl IntoElement + 'static) -> Scroll {
    let width = signal(0.0_f32);
    let last = stored(PAD);
    // A swipe's glide after the fingers lift: it pans on, but it is not the
    // viewer's input, so it never takes the app out of Keys mode.
    let coasting = stored(false);
    let handle = element_ref();
    let rest = move || {
        pan.settle_wheel();
        if last.with(|x| (*x - PAD).abs() > PAD / 2.0) {
            last.with(|x| *x = PAD);
            handle.scroll_to_offset(px(PAD), false);
        }
    };
    scroll()
        .horizontal()
        .scrollbar(false)
        .bounces(false)
        .scrollable(PAD > 0.0)
        .content_offset(Point::new(px(PAD), px(0.0)))
        .element_ref(&handle)
        .test_id("guide-timeline")
        .on_layout(move |b: &Bounds<Points>| width.set(b.size.width.0))
        .on_scroll(move |e: &ScrollEvent| {
            let x = e.offset.x.0;
            let dx = x - last.with(|l| std::mem::replace(l, x));
            // A jump (the offset first set, the scroll recentered) is no
            // wheel: only the steps are.
            if dx != 0.0 && dx.abs() < PAD / 4.0 {
                if !coasting.with(|c| *c) {
                    crate::pointer_input(pan.ui());
                }
                pan.wheeled(dx);
            }
        })
        .on_momentum_start(move |_: &Point<Points>| coasting.with(|c| *c = true))
        .on_scroll_end(move |_: &Point<Points>| {
            coasting.with(|c| *c = false);
            rest()
        })
        .on_momentum_end(move |_: &Point<Points>| {
            coasting.with(|c| *c = false);
            rest()
        })
        .child(
            view()
                .h_full()
                .w(move || px(width.get() + 2.0 * PAD))
                .child(
                    view()
                        .sticky()
                        .left(px(0.0))
                        .top(px(0.0))
                        .w(move || px(width.get()))
                        .h_full()
                        .child(grid),
                ),
        )
}
