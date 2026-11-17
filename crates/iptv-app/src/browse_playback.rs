//! Adapter for the core's cancelable live-preview policy.
use crate::{Playback, Ui, ViewMode};
use iptv_core::browse_playback::{BrowsePlayback, Effect, Policy, Surface, Target};
use iptv_core::{channel_list::ListPane, guide::Airing};
use iptv_nav::Context;
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
        && target.source == ui.library.shown.get().map(|s| s.0)
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
                && ui.focus.with(|f| f.pane == crate::guide::Pane::Guide)
                && !ui.guide_panning.get() =>
        {
            let v = crate::lists::view(ui);
            if ui.focus.with(|f| f.program(&v).airing(f.now)) != Airing::Live {
                return None;
            }
            (
                crate::lists::focused_channel(ui)?,
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
        source: ui.library.shown.get().map(|s| s.0),
        surface,
    })
}
fn policy(ui: Ui, surface: Option<Surface>) -> Policy {
    let s = crate::settings::read(ui);
    let at = &iptv_settings::Target::Global;
    let stay = match surface {
        Some(Surface::Guide) => keys::GUIDE_STAY_ON_GUIDE,
        Some(Surface::Overlay) => keys::GUIDE_STAY_ON_GUIDE_OVERLAY,
        _ => keys::GUIDE_STAY_ON_LIST,
    };
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
        stay: surface == Some(Surface::Player) || s.bool(stay, at),
    }
}
fn sync(ui: Ui) {
    let target = current(ui);
    let p = policy(ui, target.as_ref().map(|t| t.surface));
    let now = ui.now_ms();
    ui.browse_playback.update(|b| b.focus(target, p, now));
}
pub fn activity(ui: Ui) {
    ui.browse_playback.update(|b| b.activity(ui.now_ms()));
    arm(ui);
}
pub fn select(ui: Ui) {
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
            if let Some(channel) = crate::favorites::channel_of(&target.key) {
                if ui.playing.get_untracked() != Some(channel)
                    || ui.playback.get_untracked() != Playback::Live
                {
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
                }
            }
        }
        Some(Effect::Fullscreen) => full(ui),
        None => {}
    }
    arm(ui);
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
    effect(move || {
        let target = current(ui);
        let p = policy(ui, target.as_ref().map(|t| t.surface));
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
            source: ui.library.shown.get_untracked().map(|s| s.0),
            surface: Surface::Player,
        }));
        select(ui);
    }
}
