//! The guide's timeline under a finger or a mouse drag: the core's
//! [`Timeline`] model, shown by sliding the rows' programmes, the ruler's
//! times and the now line with one animated offset, so a pan moves no cell
//! and rebuilds them only when its [`Band`] moves (`iptv_core::timeline`).
//!
//! A horizontal drag over the grid pans (NSH's pan activation decides the
//! axis on the platform's thread: past [`SLOP`] horizontally it is ours,
//! past it vertically first the channel list scrolls); released moving, it
//! flings on, one step per display frame. Taps stay taps. The view follows
//! the focus model's window while nothing pans (keys, ‹ ›, Now), and a pan
//! that came to rest hands its window back to it. Its bounds are the
//! guide's view ([`view_range`]: as far back as retained listings go).
//!
//! Units: NSH's pan reports the finger's movement and speed in dp (the
//! Android view divides its pixels by the density, 1.5 on a 1920 × 1200
//! tablet), the same unit as the programme area's laid-out width, so
//! `delta / ppm` follows the finger 1:1 and nothing is scaled twice.

use std::time::Instant;

use iptv_core::guide::view_range;
use iptv_core::timeline::{Band, MARGIN, SLOP, Timeline};
use nsh_core::prelude::*;

use crate::Ui;

/// One display frame, for the first step of a fling.
const FRAME_S: f32 = 1.0 / 60.0;

/// The panned view, shared by the grid's parts (`Copy`).
#[derive(Clone, Copy)]
pub(crate) struct Pan {
    ui: Ui,
    model: Stored<Timeline>,
    /// The programme area's width (dp): [`VISIBLE`] minutes span it.
    width: Signal<f32>,
    span: Memo<i32>,
    /// The minutes the rows hold cells for, and the minute a cell's start
    /// is clamped to (the window at rest, so its title shows; the band's
    /// start while panning).
    band: Signal<Band>,
    clip: Signal<i32>,
    /// The cells' slide (dp): minus the view's offset into the band, and
    /// the last value set (a key that keeps the window sends nothing).
    offset: AnimatedValue,
    slid: Stored<f32>,
    /// The view is away from now: the Now button shows.
    away: Signal<bool>,
    /// Bumped by each fling: an older one's frames stop.
    fling: Stored<u32>,
    /// A wheel scroll is under way (`wheel.rs`).
    wheeling: Stored<bool>,
}

impl Pan {
    /// The window's state this pan belongs to.
    pub(super) fn ui(&self) -> Ui {
        self.ui
    }

    pub(crate) fn new(ui: Ui, width: Signal<f32>) -> Pan {
        let (window, now) = ui.focus.with_untracked(|f| (f.window, f.now));
        let bounds = view_range(&crate::lists::view(ui), now);
        let model = Timeline::new(window, bounds);
        let band = Band::around(model.view());
        let pan = Pan {
            ui,
            model: stored(model),
            width,
            span: computed(move || ui.focus.with(|f| f.visible)),
            band: signal(band),
            clip: signal(window),
            offset: animated_value(0.0),
            slid: stored(0.0),
            away: signal(false),
            fling: stored(0),
            wheeling: stored(false),
        };
        pan.follow_focus();
        effect(move || {
            width.get();
            pan.show();
        });
        pan
    }

    /// Dp per minute now.
    pub(crate) fn ppm(&self) -> f32 {
        self.width.get_untracked() / self.span.get_untracked() as f32
    }

    /// Dp per minute (reactive: the cells' geometry).
    pub(crate) fn scale(&self) -> f32 {
        self.width.get() / self.span.get() as f32
    }

    pub(crate) fn minutes(&self) -> i32 {
        self.span.get() + 2 * MARGIN
    }

    pub(crate) fn band(&self) -> Band {
        self.band.get()
    }

    pub(crate) fn clip(&self) -> i32 {
        self.clip.get()
    }

    pub(crate) fn offset(&self) -> AnimatedValue {
        self.offset
    }

    pub(crate) fn away(&self) -> bool {
        self.away.get()
    }

    /// The view follows the focus model's window and the schedule's
    /// bounds while nothing pans.
    fn follow_focus(self) {
        let ui = self.ui;
        effect(move || {
            ui.guide_data();
            let (window, now, span) = ui.focus.with(|f| (f.window, f.now, f.visible));
            let bounds = untracked(|| view_range(&crate::lists::view(ui), now));
            self.model.with(|m| {
                m.set_span(span);
                m.set_bounds(bounds);
                m.follow(window);
            });
            untracked(|| self.show());
        });
    }

    /// Puts the model's view on screen: the band (cells rebuilt only when
    /// it moves), the slide, the Now button.
    fn show(&self) {
        let (view, moving) = self.model.with(|m| (m.view(), m.moving()));
        let moving = moving || self.wheeling.with(|w| *w);
        let mut band = self.band.get_untracked();
        if band.follow(view) {
            self.band.set(band);
        }
        let clip = if moving {
            band.base
        } else {
            view.round() as i32
        };
        if self.clip.get_untracked() != clip {
            self.clip.set(clip);
        }
        let slide = -band.offset(view) * self.ppm();
        if self.slid.with(|s| std::mem::replace(s, slide)) != slide {
            self.offset.set(slide);
        }
        let now = self.ui.focus.with_untracked(|f| f.now);
        let away = self.model.with(|m| m.away_from_now(now));
        if self.away.get_untracked() != away {
            self.away.set(away);
        }
    }

    /// The pan came to rest: the focus model takes its window.
    fn settle(&self) {
        if self.model.with(|m| m.moving()) {
            return;
        }
        self.ui.guide_panning.set(false);
        let window = self.model.with(|m| m.window());
        let view = crate::lists::view(self.ui);
        self.ui.focus.update(|f| f.pan_to(window, &view));
        self.show();
    }

    /// The platform found a horizontal drag: it pans from here.
    fn grabbed(&self) {
        self.ui.touched();
        self.ui.guide_panning.set(true);
        self.fling.with(|g| *g += 1);
        self.model.with(Timeline::grab);
        self.show();
    }

    fn dragged(&self, gesture: &PanGesture) {
        crate::browse_playback::activity(self.ui);
        let (dx, dy, ppm) = (gesture.delta.x.0, gesture.delta.y.0, self.ppm());
        if self.model.with(|m| m.drag(dx, dy, ppm)) {
            self.show();
        }
    }

    fn released(&self, gesture: &PanGesture) {
        crate::browse_playback::activity(self.ui);
        let ppm = self.ppm();
        if !self.model.with(|m| m.release(gesture.vx, ppm)) {
            self.settle();
            return;
        }
        // Ask for the days it lands in now (both edges of the screen it
        // stops at): they read while the fling runs, so its cells are
        // there when it arrives. The days it passes are asked for as the
        // rows show them.
        let rest = self.model.with(|m| m.rest()).round() as i32;
        let schedule = crate::schedule();
        for minute in [
            rest - MARGIN,
            rest + self.ui.focus.with_untracked(|f| f.visible) + MARGIN,
        ] {
            crate::data::pages::want(&*schedule, minute);
        }
        let pan = *self;
        let generation = self.fling.with(|g| *g);
        spawn(async move {
            let mut last: Option<Instant> = None;
            loop {
                let at = next_frame().await;
                if pan.fling.with(|g| *g) != generation {
                    return;
                }
                let dt = last.map_or(FRAME_S, |l| (at - l).as_secs_f32().min(0.1));
                last = Some(at);
                let flinging = pan.model.with(|m| m.step(dt, pan.ppm()));
                pan.show();
                if !flinging {
                    pan.settle();
                    return;
                }
            }
        });
    }

    /// A wheel or touchpad scrolled `dx` dp sideways (positive: later).
    pub(super) fn wheeled(&self, dx: f32) {
        let ppm = self.ppm();
        if self.model.with(|m| m.dragging()) {
            return;
        }
        self.ui.guide_panning.set(true);
        crate::browse_playback::activity(self.ui);
        self.wheeling.with(|w| *w = true);
        self.fling.with(|g| *g += 1);
        if self.model.with(|m| m.wheel(dx, ppm)) {
            self.show();
        }
    }

    /// The wheel's scroll rested: as a pan does.
    pub(super) fn settle_wheel(&self) {
        if self.wheeling.with(|w| std::mem::replace(w, false)) {
            self.settle();
        }
    }

    fn cancelled(&self) {
        self.model.with(Timeline::cancel);
        self.settle();
    }

    /// Straight to now's window (the Now button): the focused channel's
    /// live programme, as Home.
    pub(crate) fn to_now(self) {
        self.ui.touched();
        self.ui.guide_panning.set(false);
        self.fling.with(|g| *g += 1);
        self.model.with(Timeline::cancel);
        self.ui.focus.update(|f| f.jump_to_now());
    }
}

/// The pan's touch handling on `area` (the grid around the channel list).
pub(crate) fn panning<E: Interactive>(area: E, pan: Pan) -> E {
    let claim = TouchClaim::on_pan(PanActivation::horizontal(px(SLOP), px(SLOP)));
    area.claim_touches(claim)
        .on_responder_grant(move |_: &ResponderEvent| pan.grabbed())
        .on_responder_move(move |e: &ResponderEvent| pan.dragged(&e.gesture))
        .on_responder_release(move |e: &ResponderEvent| pan.released(&e.gesture))
        .on_responder_terminate(move |_: &ResponderEvent| pan.cancelled())
}
