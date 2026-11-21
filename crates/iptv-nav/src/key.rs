//! Keys a TV remote (or a keyboard standing in for one) can send, and the
//! few a desktop keyboard adds (a remote never sends those).

/// A remote key, independent of platform key codes. Shells map their own
/// events to these; settings persist them by [`RemoteKey::name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RemoteKey {
    Up,
    Down,
    Left,
    Right,
    /// OK / select (D-pad center, Enter).
    Ok,
    Back,
    Menu,
    Guide,
    Info,
    ChannelUp,
    ChannelDown,
    PageUp,
    PageDown,
    PlayPause,
    Play,
    Pause,
    Stop,
    Rewind,
    FastForward,
    Record,
    Next,
    Previous,
    Red,
    Green,
    Yellow,
    Blue,
    /// A remote's Search (Android's `KEYCODE_SEARCH`, which the host hands
    /// on as F12), a keyboard's `/` and Ctrl + F (Cmd + F on macOS).
    Search,
    /// 0–9.
    Digit(u8),
    // A desktop keyboard's own keys and chords (no remote has them).
    Home,
    Tab,
    ShiftTab,
    ShiftLeft,
    ShiftRight,
    /// Ctrl + Left/Right (Cmd on macOS).
    CtrlLeft,
    CtrlRight,
}

const NAMED: [(RemoteKey, &str); 34] = [
    (RemoteKey::Up, "up"),
    (RemoteKey::Down, "down"),
    (RemoteKey::Left, "left"),
    (RemoteKey::Right, "right"),
    (RemoteKey::Ok, "ok"),
    (RemoteKey::Back, "back"),
    (RemoteKey::Menu, "menu"),
    (RemoteKey::Guide, "guide"),
    (RemoteKey::Info, "info"),
    (RemoteKey::ChannelUp, "channel_up"),
    (RemoteKey::ChannelDown, "channel_down"),
    (RemoteKey::PageUp, "page_up"),
    (RemoteKey::PageDown, "page_down"),
    (RemoteKey::PlayPause, "play_pause"),
    (RemoteKey::Play, "play"),
    (RemoteKey::Pause, "pause"),
    (RemoteKey::Stop, "stop"),
    (RemoteKey::Rewind, "rewind"),
    (RemoteKey::FastForward, "fast_forward"),
    (RemoteKey::Record, "record"),
    (RemoteKey::Next, "next"),
    (RemoteKey::Previous, "previous"),
    (RemoteKey::Red, "red"),
    (RemoteKey::Green, "green"),
    (RemoteKey::Yellow, "yellow"),
    (RemoteKey::Blue, "blue"),
    (RemoteKey::Search, "search"),
    (RemoteKey::Home, "home"),
    (RemoteKey::Tab, "tab"),
    (RemoteKey::ShiftTab, "shift_tab"),
    (RemoteKey::ShiftLeft, "shift_left"),
    (RemoteKey::ShiftRight, "shift_right"),
    (RemoteKey::CtrlLeft, "ctrl_left"),
    (RemoteKey::CtrlRight, "ctrl_right"),
];

impl RemoteKey {
    /// The stable name used in settings (`ok`, `channel_up`, `digit_5`).
    pub fn name(self) -> String {
        match self {
            RemoteKey::Digit(d) => format!("digit_{d}"),
            key => NAMED
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, n)| (*n).to_owned())
                .expect("every named key is listed"),
        }
    }

    /// Parses [`RemoteKey::name`]'s output.
    pub fn from_name(name: &str) -> Option<RemoteKey> {
        if let Some(d) = name.strip_prefix("digit_") {
            return d
                .parse::<u8>()
                .ok()
                .filter(|d| *d <= 9)
                .map(RemoteKey::Digit);
        }
        NAMED.iter().find(|(_, n)| *n == name).map(|(k, _)| *k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        let mut keys: Vec<RemoteKey> = NAMED.iter().map(|(k, _)| *k).collect();
        keys.extend((0..=9).map(RemoteKey::Digit));
        for key in keys {
            assert_eq!(RemoteKey::from_name(&key.name()), Some(key), "{key:?}");
        }
        assert_eq!(RemoteKey::from_name("digit_10"), None);
        assert_eq!(RemoteKey::from_name("volume_up"), None);
    }
}
