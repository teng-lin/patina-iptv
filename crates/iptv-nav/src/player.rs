//! Settings that affect how playback keys resolve.

use crate::{Action, RemoteKey};

/// The arrows that step along a replay's time bar under `scheme`.
pub fn seek_arrows(scheme: &str) -> &'static [RemoteKey] {
    match scheme {
        "left_right" => &[RemoteKey::Left, RemoteKey::Right],
        "down_up" => &[RemoteKey::Down, RemoteKey::Up],
        _ => &[],
    }
}

/// The player's action for `key` during catch-up under the `seek_keys`
/// choice `scheme`: Left/Right (the default, instead of the channel list
/// and the most recent channel) or Down/Up (instead of zapping); with
/// `transport` only the media keys seek (Rewind/Fast-forward always do).
pub fn seek_key(scheme: &str, key: RemoteKey, action: Option<Action>) -> Option<Action> {
    match (scheme, key) {
        ("left_right", RemoteKey::Left) | ("down_up", RemoteKey::Down) => Some(Action::TimeBarBack),
        ("left_right", RemoteKey::Right) | ("down_up", RemoteKey::Up) => {
            Some(Action::TimeBarForward)
        }
        _ => action,
    }
}

/// Up/Down over the video under `remote.player_up_down`: zap (the
/// default) or open the channel list. Channel+/− and Page keys always zap.
pub fn up_down_key(choice: &str, key: RemoteKey, action: Option<Action>) -> Option<Action> {
    match (choice, key, action) {
        (
            "channel_list",
            RemoteKey::Up | RemoteKey::Down,
            Some(Action::ChannelUp | Action::ChannelDown),
        ) => Some(Action::ShowChannels),
        _ => action,
    }
}
