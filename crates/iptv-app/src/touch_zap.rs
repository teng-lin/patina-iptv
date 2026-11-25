//! Changing channel by touch over full-screen live video (#285): a
//! vertical swipe on the video, the play bar's channel buttons and the
//! video's accessibility actions. All three zap as Ch± does
//! ([`crate::browse_playback::zap_by`]): the banner names the new channel
//! at once and the stream tunes after `guide.switch_delay`, so a burst of
//! swipes opens only the last channel's stream. A touch's banner shows
//! whatever `player.info_on_switch` says.
//!
//! The swipe is heard on the full-screen tap catcher above the video
//! (`ui_layers`' "video" view), never on the player's own view. Whether
//! touches are a swipe, which way and whether it commits is
//! `iptv_presentation::touch_zap`'s; this module feeds it the first
//! finger's touches and acts on its outcomes. Replays, on-demand titles,
//! multiview and anything open over the video take no swipe;
//! `playback.swipe_to_zap` turns the swipe off, not the buttons.

use iptv_presentation::touch_zap::{Direction, Outcome, Swipe};
use iptv_settings::keys;
use nsh_core::prelude::*;

use crate::fonts::OSD_TEXT;
use crate::theme::RADIUS_LARGE;
use crate::{Playback, Ui, ViewMode, channel_name, tint};

const WHITE: u32 = 0xFFFFFF;
const PANEL_ALPHA: u8 = 0xE0;
/// The preview banner's height, about: it is centered on the finger.
const PREVIEW_HEIGHT: f32 = 76.0;

/// Shared by the video and its UI ancestor, so a second finger on a
/// sibling control cancels the video's gesture too.
#[derive(Clone, Copy)]
pub(crate) struct Gesture {
    swipe: Stored<Swipe>,
    pub swiped: Stored<bool>,
}

impl Gesture {
    pub(crate) fn new() -> Self {
        Self {
            swipe: stored(Swipe::default()),
            swiped: stored(false),
        }
    }

    /// Observe starts anywhere in the UI without admitting a new swipe.
    /// Keep the original pointer ignored until it lifts: resetting here
    /// could let the second finger start a new gesture on the video.
    pub(crate) fn observe(self, ui: Ui, event: &nsh_core::TouchEvent) {
        for point in event
            .points
            .iter()
            .filter(|point| point.changed && point.pointer_type == nsh_core::PointerType::Touch)
        {
            if event.phase != nsh_core::TouchPhase::Start {
                if matches!(
                    event.phase,
                    nsh_core::TouchPhase::End | nsh_core::TouchPhase::Cancel
                ) {
                    // The video's handler already processed its own lift.
                    // A sibling's lift reaches only this ancestor, and must
                    // also release the portable gesture's cancellation lock.
                    let outcome = self.swipe.with(|swipe| swipe.cancel(point.id));
                    apply(ui, outcome, point.page.y.0);
                }
                continue;
            }
            let outcome = self.swipe.with(|swipe| {
                if swipe.pointer().is_some_and(|id| id != point.id) {
                    let size = window();
                    swipe.down(
                        point.id,
                        (point.page.x.0, point.page.y.0),
                        event.timestamp,
                        (size.width.0, size.height.0),
                    )
                } else {
                    Outcome::None
                }
            });
            if outcome != Outcome::None {
                self.swiped.with(|swiped| *swiped = true);
            }
            apply(ui, outcome, point.page.y.0);
        }
    }
}

/// The banner a swipe under way drags: the channel it would change to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Preview {
    pub channel: usize,
    /// The finger's height in the window (dp).
    pub y: f32,
    /// Released now, the swipe would change channel.
    pub committing: bool,
}

/// Whether full-screen video plays a live channel that a channel change
/// applies to: not a replay, a recording or an on-demand title, and not
/// multiview's tiles.
pub(crate) fn live(ui: Ui) -> bool {
    ui.mode.get() == ViewMode::Fullscreen
        && ui.playing.get().is_some()
        && changes_channel(ui)
        && !crate::multiview::is_open(ui)
}

/// Whether what plays is a live channel, which Ch± changes (not a replay
/// or an on-demand title): the video then has the channel actions.
pub(crate) fn changes_channel(ui: Ui) -> bool {
    ui.playback.get() == Playback::Live && !crate::on_demand::active(ui)
}

/// Whether a swipe on the video may change channel now: live video with
/// nothing open over it, the setting on.
fn takes_swipes(ui: Ui) -> bool {
    live(ui)
        && !ui.any_overlay_open()
        && crate::pip::ui_shown(ui)
        && crate::settings::flag(keys::SWIPE_TO_ZAP)
}

/// Changes channel the way `direction` goes, as Ch± does, from a touch:
/// the banner names the new channel at once (the play bar's own line does
/// while the bar shows).
pub(crate) fn zap(ui: Ui, direction: Direction) {
    if !untracked(|| live(ui)) {
        log::info!("iptv touch zap: {direction:?}: no live channel");
        return;
    }
    let Some(to) = untracked(|| crate::player_keys::zap_target(ui, direction.step())) else {
        log::info!("iptv touch zap: {direction:?}: no channel");
        return;
    };
    log::info!("iptv touch zap: {direction:?} to {to}");
    // The play bar stays usable over its popups (a chooser, the program's
    // details), which are about the channel it leaves: they go, so the
    // player owns input again and the pending zap tunes.
    if ui.chooser.with_untracked(Option::is_some) {
        crate::chooser::close(ui);
    }
    if ui.description.with_untracked(Option::is_some) {
        ui.description.set(None);
    }
    crate::browse_playback::zap_by(ui, to, true);
    if untracked(|| crate::play_bar::shows(ui)) {
        crate::play_bar::used(ui);
    } else {
        ui.show_playing();
    }
}

/// A touch used the video or the bar: as [`Ui::touched`], except that a
/// zap waiting for its switch delay stays, so the next one starts from
/// it (a burst of swipes or presses skips channels).
fn touched(ui: Ui) {
    crate::browse_playback::activity(ui);
    ui.input.set(ui.last_press.get_untracked());
}

/// The touches on `video` (the full-screen tap catcher): the first
/// finger's swipe. `swiped` holds from a swipe's first move past the
/// threshold until the next finger goes down or the view's next pointer
/// press, which a platform may still report when the finger lifts inside
/// the view (Android's does, the view filling the screen): that press is
/// then no tap. A keyboard's or a screen reader's press always is.
pub(crate) fn watch<E: Interactive>(video: E, ui: Ui, gesture: Gesture) -> E {
    let Gesture { swipe, swiped, .. } = gesture;
    // An overlay can take touch delivery before the video sees End/Cancel.
    // Clear the gesture as soon as video stops admitting it.
    effect(move || {
        if !takes_swipes(ui) {
            untracked(|| {
                swipe.with(Swipe::reset);
                if ui.swipe.get_untracked().is_some() {
                    ui.swipe.set(None);
                }
            });
        }
    });
    // Made again mid-swipe (live video turned into a replay): its banner
    // goes with it.
    cleanup(move || {
        swipe.with(Swipe::reset);
        swiped.with(|swiped| *swiped = false);
        if untracked(|| ui.swipe.try_get()).flatten().is_some() {
            ui.swipe.set(None);
        }
    });
    video.on_touch(move |e: &nsh_core::TouchEvent| {
        for p in e.points.iter().filter(|p| p.changed) {
            let (id, at, t) = (p.id, (p.page.x.0, p.page.y.0), e.timestamp);
            let outcome = match e.phase {
                nsh_core::TouchPhase::Start => {
                    let finger = p.pointer_type == nsh_core::PointerType::Touch;
                    // Native reports include unchanged contacts too. Unlike
                    // cached IDs, this snapshot cannot retain a lift lost
                    // when a modal interrupts delivery.
                    let alone = e.points.iter().all(|point| {
                        point.pointer_type != nsh_core::PointerType::Touch || point.id == id
                    });
                    let admitted = finger && alone && untracked(|| takes_swipes(ui));
                    if admitted {
                        // A one-contact Start is authoritative, even when its
                        // ID is reused. Retire any interrupted sequence and its
                        // preview before admitting the new touch.
                        apply(ui, swipe.with(Swipe::reset), at.1);
                    }
                    swipe.with(|s| {
                        if !s.swiping() {
                            swiped.with(|w| *w = false);
                        }
                        // Another finger down still cancels a swipe under
                        // way (a pinch is none).
                        if admitted || s.swiping() {
                            let size = window();
                            s.down(id, at, t, (size.width.0, size.height.0))
                        } else {
                            s.reset()
                        }
                    })
                }
                // Something opened over the video, or it stopped being live,
                // mid-swipe: no change, even at the lift.
                nsh_core::TouchPhase::Move | nsh_core::TouchPhase::End
                    if !untracked(|| takes_swipes(ui)) =>
                {
                    swipe.with(Swipe::reset)
                }
                nsh_core::TouchPhase::Move => swipe.with(|s| s.moved(id, at, t)),
                nsh_core::TouchPhase::End => swipe.with(|s| s.up(id, at, t)),
                nsh_core::TouchPhase::Cancel => swipe.with(|s| s.cancel(id)),
            };
            // A lift can be the swipe's first report past the threshold
            // (no move between): it swiped as much as a followed one.
            if outcome != Outcome::None {
                swiped.with(|w| *w = true);
            }
            apply(ui, outcome, at.1);
        }
    })
}

fn apply(ui: Ui, outcome: Outcome, y: f32) {
    match outcome {
        Outcome::None => {}
        Outcome::Follow {
            direction,
            committing,
            ..
        } => {
            let preview =
                untracked(|| crate::player_keys::zap_target(ui, direction.step())).map(|channel| {
                    Preview {
                        channel,
                        y,
                        committing,
                    }
                });
            if ui.swipe.get_untracked() != preview {
                ui.swipe.set(preview);
            }
        }
        Outcome::Commit(direction) => {
            log::info!("iptv swipe: {direction:?}");
            ui.swipe.set(None);
            touched(ui);
            haptic(Haptic::Impact(HapticImpact::Light));
            zap(ui, direction);
        }
        Outcome::Cancel => {
            log::info!("iptv swipe: cancelled");
            ui.swipe.set(None);
        }
    }
}

/// The accessibility actions of the full-screen video: next and previous
/// channel, for a screen reader, which takes swipes for itself. On an
/// iPad the video is adjustable too, so VoiceOver's own swipe up and down
/// changes channel (Android reads `increment` and `decrement` as scroll
/// actions, which TalkBack's menu need not list: there they stay off).
pub(crate) fn actions() -> Vec<nsh_core::AccessibilityActionDef> {
    let mut actions = vec![
        nsh_core::AccessibilityActionDef {
            name: "next_channel".into(),
            label: Some(crate::i18n::tr("Next channel").into()),
        },
        nsh_core::AccessibilityActionDef {
            name: "previous_channel".into(),
            label: Some(crate::i18n::tr("Previous channel").into()),
        },
    ];
    if crate::IOS {
        for name in ["increment", "decrement"] {
            actions.push(nsh_core::AccessibilityActionDef {
                name: name.into(),
                label: None,
            });
        }
    }
    actions
}

/// A screen reader's action on the video ([`actions`]).
pub(crate) fn act(ui: Ui, action: &str) {
    match action {
        "next_channel" | "increment" => zap(ui, Direction::Next),
        "previous_channel" | "decrement" => zap(ui, Direction::Previous),
        _ => {}
    }
}

/// A channel button beside the channel's name in the play bar's top line.
pub(crate) fn button(ui: Ui, direction: Direction) -> impl IntoElement {
    let (id, icon) = match direction {
        Direction::Next => (
            "play-bar-next-channel",
            crate::ui_kit::icons::Icon::ChannelNext,
        ),
        Direction::Previous => (
            "play-bar-previous-channel",
            crate::ui_kit::icons::Icon::ChannelPrevious,
        ),
    };
    let label = move || match direction {
        Direction::Next => crate::i18n::tr("Next channel"),
        Direction::Previous => crate::i18n::tr("Previous channel"),
    };
    view()
        .test_id(id)
        .role(AccessibilityRole::Button)
        .accessible(true)
        .label(label)
        .flex_shrink_0()
        .w(px(28.0))
        .h(px(28.0))
        .rounded(px(14.0))
        .items_center()
        .justify_center()
        // A finger's target: 44 dp around the 28 dp circle.
        .hit_slop(EdgeInsets::all(px(8.0)))
        .hover(|s| s.bg(tint(WHITE, crate::theme::RESTING_ALPHA * 2)))
        .on_press(move || {
            touched(ui);
            zap(ui, direction);
        })
        .child(
            svg(move || crate::ui_kit::icons::src(icon))
                .size(px(18.0))
                .text_color(rgb(WHITE)),
        )
}

/// The banner a swipe drags over the video, centered on the finger: the
/// channel's number and name and its program on air. Faint until the
/// swipe has gone far enough to change channel.
pub(crate) fn layer(ui: Ui) -> impl IntoElement {
    // Only while the video takes swipes: one held as the video leaves full
    // screen (or a dialog opens over it) shows nothing until it ends.
    let preview = computed(move || ui.swipe.get().filter(|_| takes_swipes(ui)));
    when(
        move || preview.with(Option::is_some),
        move || {
            let channel = move || preview.get().map_or(0, |p| p.channel);
            let top = move || {
                let y = preview.get().map_or(0.0, |p| p.y);
                let (high, low) = (
                    safe_area().top.0 + 8.0,
                    window().height.0 - safe_area().bottom.0 - PREVIEW_HEIGHT - 8.0,
                );
                px((y - PREVIEW_HEIGHT / 2.0).clamp(high, low.max(high)))
            };
            view()
                .test_id("swipe-preview")
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .top(top)
                .items_center()
                .pointer_events(PointerEvents::None)
                .accessibility_elements_hidden(|| true)
                .child(
                    view()
                        .max_w(px(560.0))
                        .mx(px(24.0))
                        .gap(px(4.0))
                        .px(px(20.0))
                        .py(px(12.0))
                        .rounded(px(RADIUS_LARGE))
                        .bg(move || tint(ui.app.theme.with(|t| t.background.panel), PANEL_ALPHA))
                        .opacity(move || {
                            if preview.get().is_some_and(|p| p.committing) {
                                1.0_f32
                            } else {
                                0.7
                            }
                        })
                        .child(
                            text(move || {
                                let channel = channel();
                                format!(
                                    "{}  {}",
                                    crate::lists::number(ui, channel),
                                    channel_name(channel)
                                )
                            })
                            .test_id("swipe-preview-channel")
                            .number_of_lines(1)
                            .text_size(px(OSD_TEXT))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(WHITE)),
                        )
                        .child(
                            text(move || {
                                ui.guide_data();
                                let channel = channel();
                                let schedule = crate::schedule();
                                let now = ui.focus.with(|f| f.now);
                                crate::listing_labels::listing(
                                    schedule.listing(channel, schedule.program_at(channel, now)),
                                )
                            })
                            .number_of_lines(1)
                            .text_size(px(OSD_TEXT))
                            .text_color(rgb(WHITE)),
                        ),
                )
        },
    )
}
