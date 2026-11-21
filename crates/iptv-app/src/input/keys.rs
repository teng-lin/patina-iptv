//! NSH keystroke names and the remote keys they stand for.

use iptv_nav::{Phase, RemoteKey};
use nsh_core::{KeyEvent, element::SharedString};
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

pub(super) const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// The most physical keys tracked down at once (missed releases age out).
const MAX_HELD: usize = 8;

/// The remote keys down now, per physical key: a key pressed while
/// another is held does not lose the other's release.
#[derive(Default)]
pub(super) struct RawKeys {
    held: Vec<(SharedString, RemoteKey)>,
}

impl RawKeys {
    pub(super) fn key(
        &mut self,
        event: &KeyEvent,
        phase: Phase,
        android: bool,
    ) -> Option<RemoteKey> {
        if !android {
            return remote(event, false);
        }
        // The logical key and modifiers can change during a press. Resolve
        // the release against the accepted key before the press timer fires.
        let id = if event.code.is_empty() {
            &event.key
        } else {
            &event.code
        };
        let at = self.held.iter().position(|(held, _)| held == id);
        if phase == Phase::Up {
            return at.map(|at| self.held.remove(at).1);
        }
        if phase == Phase::Repeat
            && let Some(at) = at
        {
            return Some(self.held[at].1);
        }
        if let Some(at) = at {
            self.held.remove(at);
        }
        let key = remote(event, true)?;
        if self.held.len() == MAX_HELD {
            self.held.remove(0);
        }
        self.held.push((id.clone(), key));
        Some(key)
    }
}

fn android_keyboard_key(event: &KeyEvent) -> Option<RemoteKey> {
    let logical = event.key.as_str();
    let name = match logical {
        " " => "space",
        // Match NSH's Android shortcut fallback on layouts without an
        // ASCII character. Leave transported keys to the TV decoder.
        key if key == "Unidentified" || (key.chars().count() == 1 && !key.is_ascii()) => {
            match event.code.as_str() {
                "Minus" => "-",
                "Equal" => "=",
                "BracketLeft" => "[",
                "BracketRight" => "]",
                "Slash" => "/",
                code => code
                    .strip_prefix("Key")
                    .or_else(|| code.strip_prefix("Digit"))
                    .or_else(|| code.strip_prefix("Numpad"))
                    .filter(|suffix| suffix.len() == 1)?,
            }
        }
        key => key,
    };
    DIGITS
        .iter()
        .position(|digit| *digit == name)
        .map(|digit| RemoteKey::Digit(digit as u8))
        .or_else(|| {
            KEYS.iter()
                .find(|(binding, _)| binding.eq_ignore_ascii_case(name))
                .map(|(_, key)| *key)
        })
}

/// Decode hardware remote keys and Android's keyboard stand-ins. Desktop
/// keyboard shortcuts are handled by the window's named bindings.
fn remote(event: &KeyEvent, android: bool) -> Option<RemoteKey> {
    let key = event.key.as_str();
    if event.modifiers.shift || key == "Escape" {
        return None;
    }
    if android && !event.modifiers.ctrl && !event.modifiers.alt && !event.modifiers.cmd {
        if let Some(key) = android_keyboard_key(event) {
            return Some(key);
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

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_nav::{Press, PressDetector};

    fn event(key: &str, code: &str) -> KeyEvent {
        let mut event = KeyEvent::default();
        event.key = key.to_owned().into();
        event.code = code.to_owned().into();
        event
    }

    #[test]
    fn android_keyboard_falls_back_to_the_physical_key_on_non_ascii_layouts() {
        assert_eq!(remote(&event("ь", "KeyM"), true), Some(RemoteKey::Menu));
        assert_eq!(
            remote(&event("١", "Digit1"), true),
            Some(RemoteKey::Digit(1))
        );
        assert_eq!(remote(&event("x", "KeyM"), true), None);
        assert_eq!(
            remote(&event("Unidentified", "F21"), true),
            Some(RemoteKey::Search)
        );
    }

    #[test]
    fn android_key_up_with_new_modifiers_finishes_the_original_press() {
        let mut keys = RawKeys::default();
        let mut detector = PressDetector::new(400);
        let down = event("ArrowRight", "ArrowRight");
        let key = keys
            .key(&down, Phase::Down, true)
            .expect("Right is accepted");
        assert_eq!(detector.feed(key, Phase::Down, 0, true), None);

        let mut up = down.clone();
        up.modifiers.shift = true;
        assert_eq!(
            keys.key(&event("Shift", "ShiftLeft"), Phase::Up, true),
            None
        );
        let key = keys
            .key(&up, Phase::Up, true)
            .expect("release still matches Right");
        assert_eq!(
            detector.feed(key, Phase::Up, 100, true),
            Some((RemoteKey::Right, Press::Short))
        );
        assert_eq!(detector.poll(400), None);
    }

    #[test]
    fn a_key_pressed_while_another_is_held_keeps_the_others_release() {
        let mut keys = RawKeys::default();
        let back = event("GoBack", "");
        let right = event("ArrowRight", "ArrowRight");
        assert_eq!(keys.key(&back, Phase::Down, true), Some(RemoteKey::Back));
        assert_eq!(keys.key(&right, Phase::Down, true), Some(RemoteKey::Right));
        assert_eq!(keys.key(&right, Phase::Up, true), Some(RemoteKey::Right));
        assert_eq!(keys.key(&back, Phase::Repeat, true), Some(RemoteKey::Back));
        assert_eq!(keys.key(&back, Phase::Up, true), Some(RemoteKey::Back));
        assert_eq!(keys.key(&back, Phase::Up, true), None, "released once");
    }
}
