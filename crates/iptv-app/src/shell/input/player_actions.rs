//! Transport, auxiliary windows, player controls, and the guide preview.
use super::{Action, Move, Pane, Press, RemoteKey, Ui, ViewMode};

pub(super) fn transport(ui: Ui, key: RemoteKey, press: Press, action: Action) -> bool {
    // Transport keys control the owned receiver even while its destination
    // picker or another overlay has focus.
    if ui.playback.player_surface == 0
        && ui.cast.remote.get_untracked()
        && action == Action::PlayPause
    {
        crate::playback::player_keys::play_pause(ui, key, press);
        return true;
    }
    if action == Action::Stop
        && ui.playback.player_surface == 0
        && (ui.cast.remote.get_untracked() || ui.cast.busy.get_untracked())
    {
        crate::playback::player_keys::stop_playback(ui);
        return true;
    }
    if action == Action::Stop
        && ui.playback.player_surface == 0
        && crate::playback::external_player::is_playing(ui.cast)
    {
        crate::playback::player_keys::stop_playback(ui);
        return true;
    }
    false
}

pub(super) fn auxiliary(ui: Ui, action: Action) -> bool {
    if ui.playback.player_surface != 0 {
        match action {
            // The guide is in the main window; dismissing this player lets
            // its owner stop the correct surface. Search has no local screen.
            Action::Stop | Action::ShowGuide => {
                ui.chrome
                    .mode
                    .apply(crate::kit::mode::Action::ReturnToBrowse);
                return true;
            }
            Action::OpenSearch | Action::Record => return true,
            _ => {}
        }
    }
    false
}

pub(super) fn preview(ui: Ui, key: RemoteKey, press: Press, action: Action, repeat: bool) -> bool {
    let mv = |m: Move| crate::guide::lists::navigate(ui, m);
    // The preview owns focus, not the guide row left underneath it. Keep
    // program/list commands from acting on a different channel.
    if ui.chrome.mode.get() == ViewMode::Guide && ui.guide.focus.with(|f| f.pane == Pane::Preview) {
        match action {
            Action::MoveUp => mv(Move::Up),
            Action::MoveDown => mv(Move::Down),
            Action::MoveLeft => mv(Move::Left),
            Action::MoveRight => mv(Move::Right),
            Action::Select | Action::ReturnToPlayer => crate::guide::lists::ok(ui),
            Action::ShowMenu | Action::ShowControls
                if ui.playback.playing.get().is_some()
                    || crate::playback::on_demand::active(ui.playback) =>
            {
                ui.chrome
                    .mode
                    .apply(crate::kit::mode::Action::EnterFullscreen);
                crate::playback::play_bar::remote(ui);
            }
            Action::Back => {
                crate::shell::back::go_back(ui);
            }
            Action::PlayPause | Action::Stop => {
                crate::playback::player_keys::apply(ui, key, press, action, repeat);
            }
            _ => {}
        }
        return true;
    }
    false
}

pub(super) fn controls(ui: Ui, key: RemoteKey, press: Press, action: Action, repeat: bool) -> bool {
    crate::playback::play_bar::key(ui, key, action, repeat)
        || crate::playback::player_keys::apply(ui, key, press, action, repeat)
}
