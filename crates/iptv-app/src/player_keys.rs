//! The remote over full-screen video, after the keymap (`iptv_nav`):
//!
//! | Key | Tap | Hold |
//! | --- | --- | --- |
//! | OK on TV | play bar | play bar |
//! | Menu | control strip (again: hide) | – |
//! | Up / Down | channel up / down (`remote.player_up_down`: the channel list) | repeats |
//! | Left | channel list over the video | channel list beside it |
//! | Right | most recent channel | History |
//! | Back | hide the panels, then the guide | stop playback |
//!
//! Over the card and the strip Left/Right move the focus, OK presses it and
//! Up/Down still zap. In a replay the `catchup.seek_keys` seek instead
//! (held: faster, `seek.rs`), the card showing its seek bar.

use iptv_core::tracks::Kind as Track;
use iptv_nav::{Action, Context, Press, RemoteKey, seek_arrows, seek_key, up_down_key};
use iptv_presentation::channel_list::ListMode;
use iptv_presentation::panels::{Control, Pick, QuickAction};
use iptv_presentation::zap::{ZapOrigin, zap_from_list};

use crate::{Playback, Ui, ViewMode};

/// The player's action for `key` after the settings that change it: the
/// seek keys in a replay, and Up/Down's choice.
pub(crate) fn remap(
    ui: &Ui,
    ctx: Context,
    key: RemoteKey,
    action: Option<Action>,
) -> Option<Action> {
    if !matches!(ctx, Context::Player | Context::Controls) {
        return action;
    }
    let replay = matches!(ui.playback.get_untracked(), Playback::CatchUp { .. })
        || crate::on_demand::active(*ui);
    let action = if replay && ctx == Context::Player {
        seek_key(seek_scheme(), key, action)
    } else {
        action
    };
    up_down_key(
        crate::settings::choice(iptv_settings::keys::PLAYER_UP_DOWN),
        key,
        action,
    )
}

/// Whether `key` seeks now (in a replay, under the panels' Player context):
/// it then has no long press, so holding it repeats the step.
pub(crate) fn seeks(ui: &Ui, ctx: Context, key: RemoteKey) -> bool {
    let replay = matches!(ui.playback.get_untracked(), Playback::CatchUp { .. })
        || crate::on_demand::active(*ui);
    replay && ctx == Context::Player && seek_arrows(seek_scheme()).contains(&key)
}

fn seek_scheme() -> &'static str {
    crate::settings::choice(iptv_settings::keys::SEEK_KEYS)
}

/// Seeking in a replay: whether OK plays and pauses (the arrows seek, as
/// in a media player) instead of showing the card.
fn ok_pauses(ui: Ui) -> bool {
    let replay = matches!(ui.playback.get_untracked(), Playback::CatchUp { .. })
        || crate::on_demand::active(ui);
    replay && !seek_arrows(seek_scheme()).is_empty()
}

/// The player's own actions (full screen and its panels); false for any
/// other. `repeat`: the key's auto-repeat.
pub(crate) fn apply(ui: Ui, key: RemoteKey, press: Press, action: Action, repeat: bool) -> bool {
    let fullscreen = ui.mode.get() == ViewMode::Fullscreen;
    let buttons = fullscreen && ui.panels.with(|p| p.captures_input());
    let seek = |step: i32| {
        crate::seek::key(ui, step, repeat);
    };
    match action {
        Action::Record => crate::recordings_view::current(ui),
        Action::MoveLeft if buttons => ui.update_panels(|p, now| p.move_focus(-1, now)),
        Action::MoveRight if buttons => ui.update_panels(|p, now| p.move_focus(1, now)),
        Action::MoveUp | Action::MoveDown if buttons => {}
        Action::Select if buttons => {
            let mut pick = Pick::Nothing;
            ui.update_panels(|p, now| pick = p.select(now));
            match pick {
                Pick::Quick(action) => press_quick(ui, action),
                Pick::Control(control) => press_control(ui, control),
                Pick::Hidden | Pick::Nothing => {}
            }
        }
        // The legacy cards remain available through a custom binding.
        Action::ShowPanels if crate::player::failed(&ui) => crate::player::retry(ui),
        Action::ShowPanels if crate::seek::pending(ui) => crate::seek::commit_now(ui),
        Action::ShowPanels if ok_pauses(ui) => {
            crate::player::toggle_pause(ui);
            ui.update_panels(|p, now| p.show_info(now));
        }
        Action::ShowPanels => ui.update_panels(|p, now| p.show_card(now)),
        Action::ShowControls if !fullscreen => return false,
        Action::ShowControls if nsh_core::prelude::idiom() == nsh_core::platform::Idiom::Tv => {
            if key == RemoteKey::Ok && crate::player::failed(&ui) {
                crate::player::retry(ui);
            } else if key == RemoteKey::Ok && crate::seek::pending(ui) {
                crate::seek::commit_now(ui);
            } else {
                crate::play_bar::remote(ui);
            }
        }
        Action::ShowControls if key == RemoteKey::Menu => {
            ui.update_panels(|p, now| p.toggle_controls(now))
        }
        Action::ShowControls => ui.update_panels(|p, now| p.show_controls(now)),
        Action::ShowInfo if fullscreen => ui.update_panels(|p, now| p.show_info(now)),
        Action::ShowHistory => crate::chooser::open(ui, crate::chooser::Kind::History),
        Action::PlayPause => play_pause(ui, key, press),
        // Catch-up seeking (`seek.rs`); nothing to seek in a live stream.
        Action::SeekBack => seek(-crate::seek::steps().rewind),
        Action::SeekForward => seek(crate::seek::steps().forward),
        Action::TimeBarBack => seek(-crate::seek::steps().timebar),
        Action::TimeBarForward => seek(crate::seek::steps().timebar),
        Action::ChannelUp => zap(ui, -1),
        Action::ChannelDown => zap(ui, 1),
        Action::PreviousChannel => crate::player::previous_channel(ui),
        Action::ShowChannels => crate::channels_view::open(ui, ListMode::Overlay),
        Action::ShowChannelsPreview => crate::channels_view::open(ui, ListMode::Preview),
        Action::Stop => stop_playback(ui),
        Action::GoLive => crate::player::go_live(ui),
        _ => return false,
    }
    true
}

/// Android media keys and every other host share explicit play/pause semantics.
/// Only the toggle key (and the play-bar button) reverses the current state.
pub(crate) fn play_pause(ui: Ui, key: RemoteKey, press: Press) {
    // Keep an owner's explicit Play-or-pause remapping a toggle.
    if crate::remote_editor::media_toggle_override(crate::input::context(&ui), key, press) {
        crate::player::toggle_pause(ui);
        return;
    }
    match key {
        RemoteKey::Play => crate::player::set_paused(ui, false),
        RemoteKey::Pause => crate::player::set_paused(ui, true),
        _ => crate::player::toggle_pause(ui),
    }
}

/// Long Back (or Stop): nothing plays (multiview's tiles neither); back to
/// the guide.
pub(crate) fn stop_playback(ui: Ui) {
    crate::runtime_owner::runtime().next_cast_intent();
    crate::runtime_owner::runtime().cancel_pending_native_cast();
    if ui.cast.remote.get_untracked() {
        crate::casting::stop(ui);
        return;
    }
    crate::external_player::stop(ui);
    crate::interruption::viewer_acted(ui);
    log::info!("iptv player: stopped by the viewer");
    stopped(ui);
}

/// Nothing plays, as after the viewer's Stop: also when an episode ends
/// away from its player (#187).
pub(crate) fn stopped(ui: Ui) {
    stop_to_guide(ui, crate::player::stop);
}

pub(crate) fn stopped_for_external(ui: Ui, target: &iptv_core::parental::Target) {
    stop_to_guide(ui, |ui| crate::player::stop_for_handoff(ui, target));
}

fn stop_to_guide(ui: Ui, stop: impl FnOnce(Ui)) {
    crate::multiview::close(ui);
    ui.playing.set(None);
    stop(ui);
    ui.mode.apply(crate::mode::Action::ReturnToBrowse);
}

/// Channel up/down: switches playback and keeps the focus on it. Wraps
/// within the list on screen (from the playing channel's row, else the
/// focused one); a channel the list does not have zaps within its group.
pub(crate) fn zap(ui: Ui, step: isize) {
    let Some(to) = zap_target(ui, step) else {
        return;
    };
    // Hands playback to the viewer once the channel plays (`Ui::play`).
    crate::browse_playback::zap(ui, to);
}

/// The channel [`zap`] by `step` goes to, if any: also what a swipe over
/// the video shows before it is released (`touch_zap.rs`).
pub(crate) fn zap_target(ui: Ui, step: isize) -> Option<usize> {
    let list = ui.list.get();
    let playing = if ui.mode.get_untracked() != ViewMode::Fullscreen {
        crate::lists::focused_channel(ui)
    } else {
        ui.pending_zap
            .get_untracked()
            .and_then(|t| crate::favorites::channel_of(&t.key))
            .or(ui.playing.get())
    };
    let from = ZapOrigin::from_channel(
        playing,
        |channel| list.row_of(channel),
        || ui.focus.with(|f| f.channel),
    );
    let catalog = crate::runtime_owner::schedule();
    zap_from_list(&*catalog, &list, from, step, |channel| {
        catalog.channel_block(channel) == iptv_core::organization::Block::None
    })
}

/// OK (or a tap) on an action card under the info card.
pub fn press_quick(ui: Ui, action: QuickAction) {
    ui.update_panels(|p, now| p.touch(now));
    match action {
        QuickAction::Guide => {
            if let Some(ch) = ui.playing.get() {
                crate::lists::show_channel(ui, ch);
            }
            ui.mode.apply(crate::mode::Action::ReturnToBrowse);
        }
        QuickAction::History => crate::chooser::open(ui, crate::chooser::Kind::History),
        QuickAction::Channels => crate::channels_view::open(ui, ListMode::Overlay),
        QuickAction::Favorite => press_control(ui, Control::Favorite),
        QuickAction::Multiview => press_control(ui, Control::Multiview),
        QuickAction::Controls => ui.update_panels(|p, now| p.show_controls(now)),
    }
}

/// OK (or a tap) on a control-strip button.
pub fn press_control(ui: Ui, control: Control) {
    use crate::chooser::Kind;
    ui.update_panels(|p, now| p.touch(now));
    if !crate::player_panels::enabled(ui, control) {
        log::info!("iptv control {control:?} not available");
        // Multiview drawn disabled says why (a TV at the account's limit).
        if control == Control::Multiview
            && let Some(reason) =
                nsh_core::prelude::untracked(|| crate::multiview::availability(ui)).reason()
        {
            crate::settings_view::notify(crate::capabilities::reason(reason));
        }
        return;
    }
    match control {
        Control::Recordings => crate::recordings_view::open(ui),
        Control::Record => crate::recordings_view::current(ui),
        Control::Search => crate::search_view::open(ui),
        Control::Multiview => crate::multiview::open(ui),
        Control::ChannelList => crate::channels_view::open(ui, ListMode::Overlay),
        Control::Scaling => crate::chooser::open(ui, Kind::Picture),
        Control::SleepTimer => crate::chooser::open(ui, Kind::Sleep),
        Control::VideoTracks => crate::chooser::open(ui, Kind::Track(Track::Video)),
        Control::AudioTracks => crate::chooser::open(ui, Kind::Track(Track::Audio)),
        Control::Captions => crate::chooser::open(ui, Kind::Track(Track::Text)),
        Control::AudioOffset => crate::chooser::open(ui, Kind::AudioDelay),
        Control::PictureInPicture => crate::pip::enter(ui),
        Control::Favorite => {
            if let Some(ch) = ui.playing.get() {
                crate::favorites::toggle(ui, ch);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_keys_choose_the_time_bar_keys() {
        // The player's Left/Right: the channel list and the most recent channel.
        let (list, recent) = (Some(Action::ShowChannels), Some(Action::PreviousChannel));
        let (up, down) = (Some(Action::ChannelUp), Some(Action::ChannelDown));
        let back = Some(Action::TimeBarBack);
        let forward = Some(Action::TimeBarForward);
        // Left/Right (default): instead of those; Up/Down still zap.
        assert_eq!(seek_key("left_right", RemoteKey::Left, list), back);
        assert_eq!(seek_key("left_right", RemoteKey::Right, recent), forward);
        assert_eq!(seek_key("left_right", RemoteKey::Up, up), up);
        // Down/Up: instead of zapping; Left/Right keep their meaning.
        assert_eq!(seek_key("down_up", RemoteKey::Down, down), back);
        assert_eq!(seek_key("down_up", RemoteKey::Up, up), forward);
        assert_eq!(seek_key("down_up", RemoteKey::Left, list), list);
        // Page keys zap whatever the choice.
        assert_eq!(seek_key("down_up", RemoteKey::PageUp, up), up);
        // Transport keys only: the arrows keep their meaning.
        for (key, action) in [
            (RemoteKey::Left, list),
            (RemoteKey::Right, recent),
            (RemoteKey::Up, up),
            (RemoteKey::Down, down),
        ] {
            assert_eq!(seek_key("transport", key, action), action);
        }
    }

    #[test]
    fn a_held_seek_arrow_seeks_instead_of_its_long_press() {
        // Long Left (the list beside the video) and Long Right (History)
        // give way to the seek in a replay: the arrows are the seek keys.
        let preview = Some(Action::ShowChannelsPreview);
        let history = Some(Action::ShowHistory);
        assert_eq!(
            seek_key("left_right", RemoteKey::Left, preview),
            Some(Action::TimeBarBack)
        );
        assert_eq!(
            seek_key("left_right", RemoteKey::Right, history),
            Some(Action::TimeBarForward)
        );
        assert_eq!(
            seek_arrows("left_right"),
            [RemoteKey::Left, RemoteKey::Right]
        );
        assert_eq!(seek_arrows("down_up"), [RemoteKey::Down, RemoteKey::Up]);
        assert!(seek_arrows("transport").is_empty());
    }

    #[test]
    fn up_down_switch_channels_or_open_the_channel_list() {
        let (up, down) = (Some(Action::ChannelUp), Some(Action::ChannelDown));
        let list = Some(Action::ShowChannels);
        assert_eq!(up_down_key("switch_channels", RemoteKey::Up, up), up);
        assert_eq!(up_down_key("switch_channels", RemoteKey::Down, down), down);
        assert_eq!(up_down_key("channel_list", RemoteKey::Up, up), list);
        assert_eq!(up_down_key("channel_list", RemoteKey::Down, down), list);
        // Channel+/− and the page keys zap whatever the choice.
        assert_eq!(up_down_key("channel_list", RemoteKey::ChannelUp, up), up);
        assert_eq!(up_down_key("channel_list", RemoteKey::PageDown, down), down);
        // A seek (Down/Up in a replay) stays a seek.
        let seek = Some(Action::TimeBarBack);
        assert_eq!(up_down_key("channel_list", RemoteKey::Down, seek), seek);
    }
}
