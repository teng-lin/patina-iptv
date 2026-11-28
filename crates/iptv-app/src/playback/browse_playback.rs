//! Adapter for the core's cancelable live-preview policy.
use crate::{Playback, Ui, ViewMode};
use iptv_core::capabilities::{FormFactor, Host};
use iptv_core::guide::Airing;
use iptv_nav::Context;
use iptv_presentation::browse_playback::{BrowsePlayback, Effect, Policy, Surface, Target};
use iptv_presentation::channel_list::ListPane;
use iptv_settings::{Settings, keys};
use nsh_core::prelude::*;
use std::time::Duration;

fn current(ui: Ui) -> Option<Target> {
    ui.guide_data();
    let context = crate::wiring::navigation::input_context(&ui);
    if let Some(target) = ui.playback.pending_zap.get()
        && matches!(
            context,
            Context::Player | Context::Controls | Context::ChannelList
        )
        && target.source == ui.app.library.shown.get().map(|s| s.0)
    {
        return Some(target);
    }
    let (ch, surface) = match context {
        Context::ChannelList if !crate::playback::multiview::picking(ui.playback) => {
            let c = ui.playback.channels.get()?;
            if c.pane != ListPane::Channels {
                return None;
            }
            (ui.guide.list.with(|l| l.channel(c.row))?, Surface::List)
        }
        Context::Guide
            if ui.chrome.mode.get() != ViewMode::Fullscreen
                && ui.guide.focus.with(|f| f.pane == crate::guide::Pane::Guide) =>
        {
            let v = crate::guide::lists::view(ui.guide);
            let live = ui
                .guide
                .focus
                .with(|f| f.program(&v).airing(f.now) == Airing::Live);
            let ch = crate::guide::lists::focused_channel(ui.guide)?;
            // A pan from the live program moves the focus along the row
            // without leaving the channel: a waiting choice stays, and
            // panning back starts no preview again.
            if !live {
                let key = crate::wiring::runtime_owner::schedule().channel_key(ch)?;
                let held = ui.guide.focus.with(|f| {
                    ui.guide
                        .pan_anchor
                        .with(|a| a.as_ref().is_some_and(|a| a.holds(&key, f)))
                });
                if !held {
                    return None;
                }
            }
            (
                ch,
                if ui.chrome.mode.get() == ViewMode::Overlay {
                    Surface::Overlay
                } else {
                    Surface::Guide
                },
            )
        }
        _ => return None,
    };
    Some(Target {
        key: crate::wiring::runtime_owner::schedule().channel_key(ch)?,
        source: ui.app.library.shown.get().map(|s| s.0),
        surface,
    })
}
/// Handhelds need connected keys before guide autoplay and its delay
/// apply. Desktops and TVs keep them regardless of the input report.
pub(crate) fn autoplay_supported(host: Host) -> bool {
    !crate::wiring::device::handheld(host.form_factor)
        || crate::kit::input::keys_for(host).is_some()
}

/// Whether choosing a channel in the guide over the video or in the
/// channel list keeps it open (`guide.after_choosing`) on a device of
/// `form_factor`. A phone or a tablet always goes full screen. The guide's
/// preview layout does not ask: there the first choice tunes the preview,
/// and choosing the playing channel goes full screen (`browse_playback`).
fn stays_after_choosing(s: &Settings, form_factor: FormFactor) -> bool {
    !crate::wiring::device::handheld(form_factor)
        && s.choice(keys::GUIDE_AFTER_CHOOSING, &iptv_settings::Target::Global) == "stay"
}

/// `guide.fullscreen_timeout` on a device of `form_factor`, in ms: a
/// TV's only; elsewhere 0 (never).
fn fullscreen_timeout_ms(s: &Settings, form_factor: FormFactor) -> u64 {
    if form_factor == FormFactor::Tv {
        u64::try_from(s.duration_ms(keys::FULLSCREEN_TIMEOUT, &iptv_settings::Target::Global))
            .unwrap_or(0)
    } else {
        0
    }
}

fn policy(app: crate::SharedUi, surface: Option<Surface>) -> Policy {
    let s = crate::wiring::settings_wiring::read(app);
    let at = &iptv_settings::Target::Global;
    // The form factor only: a resized window changes nothing here.
    let host = crate::wiring::device::host();
    let form_factor = host.form_factor;
    Policy {
        // Off while its row is not offered (a phone or a tablet with no
        // keyboard or D-pad, #280), whatever is stored.
        autoplay: surface != Some(Surface::Player)
            && s.bool(keys::AUTOPLAY, at)
            && autoplay_supported(host),
        delay_ms: s.duration_ms(keys::SWITCH_DELAY, at).max(0) as u64,
        fullscreen_ms: if surface == Some(Surface::Player) {
            0
        } else {
            fullscreen_timeout_ms(&s, form_factor)
        },
        // The preview layout decides on every device (d77033af): the first
        // choice tunes the preview, choosing the playing channel again goes
        // full screen. The guide over the video and the channel list
        // follow `guide.after_choosing`.
        stay: match surface {
            Some(Surface::Guide | Surface::Player) => true,
            _ => stays_after_choosing(&s, form_factor),
        },
    }
}
/// Holds guide autoplay on the focused channel until the viewer moves to
/// another one: the main window closed while recordings run (nothing may
/// start playing in the hidden window, nor when it shows again), or a
/// right-click or long press moved the focus to open a menu (`menu_view`).
pub fn hold(ui: Ui) {
    let focused = untracked(|| current(ui)).map(|t| t.key);
    ui.guide.autoplay_held.set(Some(focused));
    ui.guide.browse_playback.set(Default::default());
}

/// `policy`, with autoplay off while it is held on `target`; moving to
/// another target releases the hold.
fn policy_for(ui: Ui, target: Option<&Target>) -> Policy {
    let mut p = policy(ui.app, target.map(|t| t.surface));
    let key = target.map(|t| t.key.clone());
    match ui.guide.autoplay_held.get() {
        Some(held) if held == key || key.is_none() => p.autoplay = false,
        Some(_) => untracked(|| ui.guide.autoplay_held.set(None)),
        None => {}
    }
    p
}
fn sync(ui: Ui) {
    let target = current(ui);
    let p = policy_for(ui, target.as_ref());
    let now = ui.now_ms();
    ui.guide.browse_playback.update(|b| b.focus(target, p, now));
}
/// A touch or mouse drag in the guide or a list crossed the drag
/// threshold: a preview waiting to start is canceled; a tap's choice
/// still plays (`BrowsePlayback::dragged`).
pub fn dragged(ui: Ui) {
    ui.guide.browse_playback.update(|b| b.dragged());
    activity(ui);
}
/// The touches on `list` (the guide's rows, the channel list): a finger
/// that crosses the drag threshold is a drag ([`dragged`]) even where the
/// list cannot scroll (at its top, dragging down), whose platform view may
/// then report no scroll drag at all.
pub fn watch_drags<E: Interactive>(list: E, ui: Ui) -> E {
    let watch = stored(crate::guide::timeline::Threshold::default());
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
    ui.guide.browse_playback.update(|b| b.activity(ui.now_ms()));
    arm(ui);
}
pub fn select(ui: Ui) {
    // A choice hands playback to the viewer once it plays (`Ui::play`):
    // one a guard refuses changes nothing (`interruption.rs`).
    sync(ui);
    ui.guide.browse_playback.update(|b| b.select(ui.now_ms()));
    run(ui);
}
fn run(ui: Ui) {
    // Revalidate at callback time, before using a queued identity.
    sync(ui);
    let mut effect = None;
    ui.guide
        .browse_playback
        .update(|b| effect = b.tick(ui.now_ms()));
    match effect {
        Some(Effect::Tune {
            target,
            fullscreen,
            chosen,
        }) => {
            if let Some(channel) = crate::guide::favorites::channel_of(&target.key)
                && crate::playback::player::allow_guide_channel(channel)
            {
                if chosen && crate::playback::external_player::maybe_open(ui, channel, None) {
                    ui.playback.pending_zap.set(None);
                    return;
                }
                if !playing_live(ui.playback, channel) {
                    let state = ui.guide.browse_playback.get_untracked();
                    ui.play(channel, Playback::Live);
                    ui.guide.browse_playback.set(state);
                }
                if target.surface == Surface::Player {
                    ui.playback.pending_zap.set(None);
                    crate::guide::lists::show_channel(ui, channel);
                    let list = ui.guide.list.get_untracked();
                    ui.playback
                        .channels
                        .update(|c| c.iter_mut().for_each(|c| c.follow(channel, &list)));
                    // A touch's zap always shows its banner (#285).
                    let touched = ui.playback.touch_zapped.get_untracked();
                    if touched {
                        ui.playback.touch_zapped.set(false);
                    }
                    if ui.chrome.mode.get_untracked() == ViewMode::Fullscreen
                        && (crate::wiring::settings::flag(keys::INFO_ON_SWITCH)
                            || (touched && !untracked(|| crate::playback::play_bar::shows(ui))))
                    {
                        ui.show_playing();
                    }
                } else if fullscreen {
                    full(ui);
                } else if playing_live(ui.playback, channel) {
                    ui.guide
                        .browse_playback
                        .update(|state| state.playing(&target.key, ui.now_ms()));
                }
            } else if target.surface == Surface::Player {
                ui.playback.pending_zap.set(None);
            }
        }
        Some(Effect::Fullscreen) => {
            if let Some(channel) =
                current(ui).and_then(|t| crate::guide::favorites::channel_of(&t.key))
                && playing_live(ui.playback, channel)
                && crate::wiring::runtime_owner::schedule().channel_block(channel)
                    == iptv_core::organization::Block::None
            {
                full(ui);
            }
        }
        None => {}
    }
    arm(ui);
}
fn playing_live(ui: crate::PlaybackUi, channel: usize) -> bool {
    ui.playing.get_untracked() == Some(channel) && ui.playback.get_untracked() == Playback::Live
}

fn full(ui: Ui) {
    if crate::screens::channels_view::is_open(ui.playback) {
        crate::screens::channels_view::close(ui.playback);
    }
    ui.chrome
        .mode
        .apply(crate::kit::mode::Action::EnterFullscreen);
    ui.show_playing();
}
fn arm(ui: Ui) {
    if let Some(at) = ui
        .guide
        .browse_playback
        .with_untracked(BrowsePlayback::deadline)
    {
        drop(set_timeout(
            Duration::from_millis(at.saturating_sub(ui.now_ms())),
            move || {
                // Input may have extended this deadline; old timers do nothing.
                if ui
                    .guide
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
        let focus = ui.guide.focus.get();
        ui.guide.list.with(|_| ());
        let channel = untracked(|| crate::guide::lists::focused_channel(ui.guide))
            .and_then(|ch| crate::wiring::runtime_owner::schedule().channel_key(ch));
        let stale = ui.guide.pan_anchor.with_untracked(|a| {
            a.as_ref()
                .is_some_and(|a| channel.as_deref().is_none_or(|c| !a.holds(c, &focus)))
        });
        if stale {
            ui.guide.pan_anchor.set(None);
        }
    });
    effect(move || {
        let target = current(ui);
        let p = policy_for(ui, target.as_ref());
        let playing = ui
            .playback
            .playing
            .get()
            .and_then(|c| crate::wiring::runtime_owner::schedule().channel_key(c));
        let live = ui.playback.playback.get() == Playback::Live;
        untracked(|| {
            ui.guide.browse_playback.update(|b| {
                b.focus(target, p, ui.now_ms());
                if live && let Some(key) = playing {
                    b.playing(&key, ui.now_ms());
                }
            });
            arm(ui);
        });
    });
}

/// The channel a full-screen zap waits to tune (`guide.switch_delay`):
/// the banner and the play bar name it at once.
pub(crate) fn pending_channel(ui: Ui) -> Option<usize> {
    if ui.chrome.mode.get() != ViewMode::Fullscreen {
        return None;
    }
    ui.playback.pending_zap.with(|t| {
        t.as_ref()
            .filter(|t| t.surface == Surface::Player)
            .and_then(|t| crate::guide::favorites::channel_of(&t.key))
    })
}

/// Debounced full-screen/list zapping; repeated zaps start from the pending
/// channel rather than reopening the currently playing one repeatedly.
pub fn zap(ui: Ui, channel: usize) {
    zap_by(ui, channel, false);
}

/// [`zap`]; `touch`: a touch's zap (a swipe, a play bar button, a screen
/// reader's action), whose banner shows when it tunes whatever
/// `player.info_on_switch` says (`touch_zap.rs`).
pub fn zap_by(ui: Ui, channel: usize, touch: bool) {
    if ui.chrome.mode.get_untracked() != ViewMode::Fullscreen {
        crate::guide::lists::show_channel(ui, channel);
        select(ui);
        return;
    }
    if let Some(key) = crate::wiring::runtime_owner::schedule().channel_key(channel) {
        ui.playback.touch_zapped.set(touch);
        ui.playback.pending_zap.set(Some(Target {
            key,
            source: ui.app.library.shown.get_untracked().map(|s| s.0),
            surface: Surface::Player,
        }));
        select(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_settings::Value;

    #[test]
    fn the_stay_choice_and_the_full_screen_timeout_follow_the_device() {
        let mut s = Settings::new();
        s.set("guide.after_choosing", Value::choice("stay"))
            .unwrap();
        s.set("guide.fullscreen_timeout", Value::secs(5)).unwrap();
        for (form, stays, timeout) in [
            (FormFactor::Phone, false, 0),
            (FormFactor::Tablet, false, 0),
            (FormFactor::Desktop, true, 0),
            (FormFactor::Tv, true, 5_000),
        ] {
            assert_eq!(stays_after_choosing(&s, form), stays, "{form:?}");
            assert_eq!(fullscreen_timeout_ms(&s, form), timeout, "{form:?}");
        }
        // The default goes full screen everywhere.
        let s = Settings::new();
        assert!(!stays_after_choosing(&s, FormFactor::Tv));
        assert!(!stays_after_choosing(&s, FormFactor::Desktop));
    }
}
