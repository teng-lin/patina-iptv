//! Adapter for the core's cancelable live-preview policy.
use crate::{Playback, Ui, ViewMode};
use iptv_core::guide::Airing;
use iptv_nav::Context;
use iptv_presentation::browse_playback::{BrowsePlayback, Effect, Policy, Surface, Target};
use iptv_presentation::channel_list::ListPane;
use iptv_settings::keys;
use nsh_core::prelude::*;
use std::time::Duration;

fn current(ui: Ui) -> Option<Target> {
    ui.guide_data();
    let context = crate::input::context(&ui);
    if let Some(target) = ui.pending_zap.get()
        && matches!(
            context,
            Context::Player | Context::Controls | Context::ChannelList
        )
        && target.source == ui.app.library.shown.get().map(|s| s.0)
    {
        return Some(target);
    }
    let (ch, surface) = match context {
        Context::ChannelList if !crate::multiview::picking(ui) => {
            let c = ui.channels.get()?;
            if c.pane != ListPane::Channels {
                return None;
            }
            (ui.list.with(|l| l.channel(c.row))?, Surface::List)
        }
        Context::Guide
            if ui.mode.get() != ViewMode::Fullscreen
                && ui.focus.with(|f| f.pane == crate::guide::Pane::Guide) =>
        {
            let v = crate::lists::view(ui);
            let live = ui
                .focus
                .with(|f| f.program(&v).airing(f.now) == Airing::Live);
            let ch = crate::lists::focused_channel(ui)?;
            // A pan from the live program moves the focus along the row
            // without leaving the channel: a waiting choice stays, and
            // panning back starts no preview again.
            if !live {
                let key = crate::schedule().channel_key(ch)?;
                let held = ui.focus.with(|f| {
                    ui.pan_anchor
                        .with(|a| a.as_ref().is_some_and(|a| a.holds(&key, f)))
                });
                if !held {
                    return None;
                }
            }
            (
                ch,
                if ui.mode.get() == ViewMode::Overlay {
                    Surface::Overlay
                } else {
                    Surface::Guide
                },
            )
        }
        _ => return None,
    };
    Some(Target {
        key: crate::schedule().channel_key(ch)?,
        source: ui.app.library.shown.get().map(|s| s.0),
        surface,
    })
}
fn policy(ui: Ui, surface: Option<Surface>) -> Policy {
    let s = crate::settings::read(ui);
    let at = &iptv_settings::Target::Global;
    Policy {
        autoplay: !crate::desktop::recorder_only_startup()
            && surface != Some(Surface::Player)
            && s.bool(keys::AUTOPLAY, at),
        delay_ms: s.duration_ms(keys::SWITCH_DELAY, at).max(0) as u64,
        fullscreen_ms: if surface == Some(Surface::Player) {
            0
        } else {
            s.duration_ms(keys::FULLSCREEN_TIMEOUT, at).max(0) as u64
        },
        stay: match surface {
            Some(Surface::Guide | Surface::Player) => true,
            Some(Surface::Overlay) => s.bool(keys::GUIDE_STAY_ON_GUIDE, at),
            _ => s.bool(keys::GUIDE_STAY_ON_LIST, at),
        },
    }
}
/// Holds guide autoplay on the focused channel until the viewer moves to
/// another one: the main window closed while recordings run (nothing may
/// start playing in the hidden window, nor when it shows again), or a
/// right-click or long press moved the focus to open a menu (`menu_view`).
pub fn hold(ui: Ui) {
    let focused = untracked(|| current(ui)).map(|t| t.key);
    ui.autoplay_held.set(Some(focused));
    ui.browse_playback.set(Default::default());
}

/// `policy`, with autoplay off while it is held on `target`; moving to
/// another target releases the hold.
fn policy_for(ui: Ui, target: Option<&Target>) -> Policy {
    let mut p = policy(ui, target.map(|t| t.surface));
    let key = target.map(|t| t.key.clone());
    match ui.autoplay_held.get() {
        Some(held) if held == key || key.is_none() => p.autoplay = false,
        Some(_) => untracked(|| ui.autoplay_held.set(None)),
        None => {}
    }
    p
}
fn sync(ui: Ui) {
    let target = current(ui);
    let p = policy_for(ui, target.as_ref());
    let now = ui.now_ms();
    ui.browse_playback.update(|b| b.focus(target, p, now));
}
/// A touch or mouse drag in the guide or a list crossed the drag
/// threshold: a preview waiting to start is canceled; a tap's choice
/// still plays (`BrowsePlayback::dragged`).
pub fn dragged(ui: Ui) {
    ui.browse_playback.update(|b| b.dragged());
    activity(ui);
}
/// The touches on `list` (the guide's rows, the channel list): a finger
/// that crosses the drag threshold is a drag ([`dragged`]) even where the
/// list cannot scroll (at its top, dragging down), whose platform view may
/// then report no scroll drag at all.
pub fn watch_drags<E: Interactive>(list: E, ui: Ui) -> E {
    let watch = stored(crate::timeline::Threshold::default());
    list.on_touch(move |e: &nsh_core::TouchEvent| {
        // The first finger down is followed by its id; others are ignored.
        for p in e.points.iter().filter(|p| p.changed) {
            let (id, x, y) = (p.id, p.page.x.0, p.page.y.0);
            let drag = match e.phase {
                nsh_core::TouchPhase::Start => {
                    watch.with(|w| w.down(id, x, y));
                    false
                }
                nsh_core::TouchPhase::Move => watch.with(|w| w.moved(id, x, y)),
                // A scroll view around the list took a moving finger over
                // (Android intercepts a drag past its own slop, at the
                // list's end too): a drag as well. A still finger's cancel
                // (a dialog, the system) is none.
                nsh_core::TouchPhase::Cancel => watch.with(|w| w.canceled(id, x, y)),
                nsh_core::TouchPhase::End => {
                    watch.with(|w| w.up(id));
                    false
                }
            };
            if drag {
                dragged(ui);
            }
        }
    })
}
pub fn activity(ui: Ui) {
    ui.browse_playback.update(|b| b.activity(ui.now_ms()));
    arm(ui);
}
pub fn select(ui: Ui) {
    // A choice hands playback to the viewer once it plays (`Ui::play`):
    // one a guard refuses changes nothing (`interruption.rs`).
    sync(ui);
    ui.browse_playback.update(|b| b.select(ui.now_ms()));
    run(ui);
}
fn run(ui: Ui) {
    // Revalidate at callback time, before using a queued identity.
    sync(ui);
    let mut effect = None;
    ui.browse_playback.update(|b| effect = b.tick(ui.now_ms()));
    match effect {
        Some(Effect::Tune { target, fullscreen }) => {
            if let Some(channel) = crate::favorites::channel_of(&target.key)
                && crate::player::allow_guide_channel(channel)
            {
                if !playing_live(ui, channel) {
                    let state = ui.browse_playback.get_untracked();
                    ui.play(channel, Playback::Live);
                    ui.browse_playback.set(state);
                }
                if target.surface == Surface::Player {
                    ui.pending_zap.set(None);
                    crate::lists::show_channel(ui, channel);
                    let list = ui.list.get_untracked();
                    ui.channels
                        .update(|c| c.iter_mut().for_each(|c| c.follow(channel, &list)));
                    if ui.mode.get_untracked() == ViewMode::Fullscreen
                        && crate::settings::flag(keys::INFO_ON_SWITCH)
                    {
                        ui.show_playing();
                    }
                } else if fullscreen {
                    full(ui);
                } else if playing_live(ui, channel) {
                    ui.browse_playback
                        .update(|state| state.playing(&target.key, ui.now_ms()));
                }
            } else if target.surface == Surface::Player {
                ui.pending_zap.set(None);
            }
        }
        Some(Effect::Fullscreen) => {
            if let Some(channel) = current(ui).and_then(|t| crate::favorites::channel_of(&t.key))
                && playing_live(ui, channel)
                && crate::schedule().channel_block(channel) == iptv_core::organization::Block::None
            {
                full(ui);
            }
        }
        None => {}
    }
    arm(ui);
}
fn playing_live(ui: Ui, channel: usize) -> bool {
    ui.playing.get_untracked() == Some(channel) && ui.playback.get_untracked() == Playback::Live
}

fn full(ui: Ui) {
    if crate::channels_view::is_open(ui) {
        crate::channels_view::close(ui);
    }
    ui.mode.set(ViewMode::Fullscreen);
    ui.show_playing();
}
fn arm(ui: Ui) {
    if let Some(at) = ui.browse_playback.with_untracked(BrowsePlayback::deadline) {
        drop(set_timeout(
            Duration::from_millis(at.saturating_sub(ui.now_ms())),
            move || {
                // Input may have extended this deadline; old timers do nothing.
                if ui
                    .browse_playback
                    .with_untracked(|b| b.deadline().is_some_and(|at| ui.now_ms() >= at))
                {
                    run(ui);
                }
            },
        ));
    }
}
pub fn bind(ui: Ui) {
    // A pan's anchor ends once the focus moves anywhere it does not hold
    // (keys, a tap, another list, a zap): coming back later is no pan.
    effect(move || {
        let focus = ui.focus.get();
        ui.list.with(|_| ());
        let channel = untracked(|| crate::lists::focused_channel(ui))
            .and_then(|ch| crate::schedule().channel_key(ch));
        let stale = ui.pan_anchor.with_untracked(|a| {
            a.as_ref()
                .is_some_and(|a| channel.as_deref().is_none_or(|c| !a.holds(c, &focus)))
        });
        if stale {
            ui.pan_anchor.set(None);
        }
    });
    effect(move || {
        let target = current(ui);
        let p = policy_for(ui, target.as_ref());
        let playing = ui
            .playing
            .get()
            .and_then(|c| crate::schedule().channel_key(c));
        let live = ui.playback.get() == Playback::Live;
        untracked(|| {
            ui.browse_playback.update(|b| {
                b.focus(target, p, ui.now_ms());
                if live && let Some(key) = playing {
                    b.playing(&key, ui.now_ms());
                }
            });
            arm(ui);
        });
    });
}

/// Debounced full-screen/list zapping; repeated zaps start from the pending
/// channel rather than reopening the currently playing one repeatedly.
pub fn zap(ui: Ui, channel: usize) {
    if ui.mode.get_untracked() != ViewMode::Fullscreen {
        crate::lists::show_channel(ui, channel);
        select(ui);
        return;
    }
    if let Some(key) = crate::schedule().channel_key(channel) {
        ui.pending_zap.set(Some(Target {
            key,
            source: ui.app.library.shown.get_untracked().map(|s| s.0),
            surface: Surface::Player,
        }));
        select(ui);
    }
}
