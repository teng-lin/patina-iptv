//! NSH keystroke names and the remote keys they stand for.

use iptv_nav::RemoteKey;
use nsh_core::KeyEvent;
use nsh_tv::RemoteKey as TvKey;

/// Named keyboard bindings, including the keyboard stand-ins for a remote.
pub(super) const KEYS: &[(&str, RemoteKey)] = &[
    ("up", RemoteKey::Up),
    ("down", RemoteKey::Down),
    ("left", RemoteKey::Left),
    ("right", RemoteKey::Right),
    ("enter", RemoteKey::Ok),
    ("pageup", RemoteKey::PageUp),
    ("pagedown", RemoteKey::PageDown),
    // Keyboard stand-ins.
    ("m", RemoteKey::Menu),
    ("g", RemoteKey::Guide),
    ("i", RemoteKey::Info),
    ("=", RemoteKey::ChannelUp),
    ("-", RemoteKey::ChannelDown),
    ("space", RemoteKey::PlayPause),
    ("p", RemoteKey::Previous),
    ("n", RemoteKey::Next),
    ("[", RemoteKey::Rewind),
    ("]", RemoteKey::FastForward),
    // A desktop keyboard's own (Ctrl + arrows: Cmd on macOS, where Ctrl
    // switches spaces).
    ("home", RemoteKey::Home),
    ("tab", RemoteKey::Tab),
    ("shift+tab", RemoteKey::ShiftTab),
    ("shift+left", RemoteKey::ShiftLeft),
    ("shift+right", RemoteKey::ShiftRight),
    ("mod+left", RemoteKey::CtrlLeft),
    ("mod+right", RemoteKey::CtrlRight),
    ("/", RemoteKey::Search),
    ("mod+f", RemoteKey::Search),
];

/// Decode hardware remote keys and Android's keyboard stand-ins. Desktop
/// keyboard shortcuts are handled by the window's named bindings.
pub(super) fn remote(event: &KeyEvent) -> Option<RemoteKey> {
    let key = event.key.as_str();
    if event.modifiers.shift || key == "Escape" {
        return None;
    }
    if super::LONG_PRESS && !event.modifiers.ctrl && !event.modifiers.alt && !event.modifiers.cmd {
        let name = if key == " " { "space" } else { key };
        if let Some((_, key)) = KEYS
            .iter()
            .find(|(binding, _)| binding.eq_ignore_ascii_case(name))
        {
            return Some(*key);
        }
    } else if matches!(
        key,
        "ArrowUp"
            | "ArrowDown"
            | "ArrowLeft"
            | "ArrowRight"
            | "Enter"
            | "PageUp"
            | "PageDown"
            | "0"
            | "1"
            | "2"
            | "3"
            | "4"
            | "5"
            | "6"
            | "7"
            | "8"
            | "9"
            | "F1"
            | "F2"
            | "F3"
            | "F4"
    ) {
        return None;
    }
    Some(match TvKey::from_event(event)? {
        TvKey::Up => RemoteKey::Up,
        TvKey::Down => RemoteKey::Down,
        TvKey::Left => RemoteKey::Left,
        TvKey::Right => RemoteKey::Right,
        TvKey::Ok => RemoteKey::Ok,
        TvKey::Back => RemoteKey::Back,
        TvKey::Menu => RemoteKey::Menu,
        TvKey::Guide => RemoteKey::Guide,
        TvKey::Info => RemoteKey::Info,
        TvKey::ChannelUp => RemoteKey::ChannelUp,
        TvKey::ChannelDown => RemoteKey::ChannelDown,
        TvKey::PlayPause => RemoteKey::PlayPause,
        TvKey::Play => RemoteKey::Play,
        TvKey::Pause => RemoteKey::Pause,
        TvKey::Stop => RemoteKey::Stop,
        TvKey::Rewind => RemoteKey::Rewind,
        TvKey::FastForward => RemoteKey::FastForward,
        TvKey::Record => RemoteKey::Record,
        TvKey::Next => RemoteKey::Next,
        TvKey::Previous => RemoteKey::Previous,
        TvKey::Red => RemoteKey::Red,
        TvKey::Green => RemoteKey::Green,
        TvKey::Yellow => RemoteKey::Yellow,
        TvKey::Blue => RemoteKey::Blue,
        TvKey::Search => RemoteKey::Search,
        TvKey::Digit(digit) => RemoteKey::Digit(digit),
        _ => return None,
    })
}
