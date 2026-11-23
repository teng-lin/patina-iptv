//! The keys of a TV remote, independent of the platform.

use nsh_core::KeyEvent;

use crate::transport;

/// A key of a TV remote (or a keyboard key standing in for one while an app
/// is developed on a desktop).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RemoteKey {
    /// D-pad up.
    Up,
    /// D-pad down.
    Down,
    /// D-pad left.
    Left,
    /// D-pad right.
    Right,
    /// OK / select: the D-pad's centre, a gamepad's A (the host makes it the
    /// centre), Enter.
    Ok,
    /// Back (Escape on a desktop keyboard).
    Back,
    /// Menu (Android's `KEYCODE_MENU`).
    Menu,
    /// Guide.
    Guide,
    /// Info.
    Info,
    /// Channel up (Page Up on a keyboard).
    ChannelUp,
    /// Channel down (Page Down on a keyboard).
    ChannelDown,
    /// Play / pause.
    PlayPause,
    /// Play.
    Play,
    /// Pause.
    Pause,
    /// Stop.
    Stop,
    /// Rewind.
    Rewind,
    /// Fast forward.
    FastForward,
    /// Record.
    Record,
    /// Next track / chapter.
    Next,
    /// Previous track / chapter.
    Previous,
    /// The red colour key (F1 on a keyboard).
    Red,
    /// The green colour key (F2 on a keyboard).
    Green,
    /// The yellow colour key (F3 on a keyboard).
    Yellow,
    /// The blue colour key (F4 on a keyboard).
    Blue,
    /// Search.
    Search,
    /// A digit, 0 to 9.
    Digit(u8),
}

impl RemoteKey {
    /// The remote key a raw NSH key event is, if any. A chord with Control,
    /// Alt or Meta is the app's shortcut, never a remote key.
    pub fn from_event(event: &KeyEvent) -> Option<RemoteKey> {
        Self::from_parts(
            &event.key,
            &event.code,
            event.modifiers.ctrl || event.modifiers.alt || event.modifiers.cmd,
        )
    }

    fn from_parts(key: &str, code: &str, chord: bool) -> Option<RemoteKey> {
        if chord {
            return None;
        }
        if let Some(key) = transport::decode(key, code) {
            return Some(key);
        }
        Some(match key {
            "ArrowUp" => RemoteKey::Up,
            "ArrowDown" => RemoteKey::Down,
            "ArrowLeft" => RemoteKey::Left,
            "ArrowRight" => RemoteKey::Right,
            "Enter" => RemoteKey::Ok,
            "GoBack" | "BrowserBack" | "Escape" => RemoteKey::Back,
            "ContextMenu" => RemoteKey::Menu,
            "MediaPlayPause" => RemoteKey::PlayPause,
            "MediaPlay" => RemoteKey::Play,
            "MediaPause" => RemoteKey::Pause,
            "MediaStop" => RemoteKey::Stop,
            "MediaRewind" => RemoteKey::Rewind,
            "MediaFastForward" => RemoteKey::FastForward,
            "MediaRecord" => RemoteKey::Record,
            "MediaTrackNext" => RemoteKey::Next,
            "MediaTrackPrevious" => RemoteKey::Previous,
            "BrowserSearch" => RemoteKey::Search,
            // UI Events names, as browser TV platforms deliver them (Samsung
            // Tizen: CH+/CH- 427/428, the colour keys 403-406, Info 457,
            // Guide 458), beside a keyboard's stand-ins.
            "ChannelUp" | "PageUp" => RemoteKey::ChannelUp,
            "ChannelDown" | "PageDown" => RemoteKey::ChannelDown,
            "ColorF0Red" | "F1" => RemoteKey::Red,
            "ColorF1Green" | "F2" => RemoteKey::Green,
            "ColorF2Yellow" | "F3" => RemoteKey::Yellow,
            "ColorF3Blue" | "F4" => RemoteKey::Blue,
            "Info" => RemoteKey::Info,
            "Guide" => RemoteKey::Guide,
            digit if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() => {
                RemoteKey::Digit(digit.as_bytes()[0] - b'0')
            }
            _ => return None,
        })
    }

    /// A short label for hints and diagnostics.
    pub fn label(self) -> String {
        match self {
            RemoteKey::Digit(d) => d.to_string(),
            RemoteKey::ChannelUp => "CH+".into(),
            RemoteKey::ChannelDown => "CH−".into(),
            RemoteKey::PlayPause => "Play/Pause".into(),
            RemoteKey::FastForward => "Fast forward".into(),
            key => format!("{key:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::keymap::Modifiers;

    fn event(key: &str, code: &str) -> KeyEvent {
        let mut event = KeyEvent::default();
        event.key = key.to_owned().into();
        event.code = code.to_owned().into();
        event
    }

    fn key(key: &str, code: &str) -> Option<RemoteKey> {
        RemoteKey::from_event(&event(key, code))
    }

    #[test]
    fn maps_what_stock_android_names() {
        // KEYCODE_DPAD_CENTER (23) and ENTER (66) are both "Enter".
        assert_eq!(key("Enter", ""), Some(RemoteKey::Ok));
        assert_eq!(key("GoBack", ""), Some(RemoteKey::Back));
        assert_eq!(key("ArrowLeft", ""), Some(RemoteKey::Left));
        assert_eq!(key("MediaPlayPause", ""), Some(RemoteKey::PlayPause));
        assert_eq!(key("ContextMenu", ""), Some(RemoteKey::Menu));
        assert_eq!(key("7", ""), Some(RemoteKey::Digit(7)));
    }

    #[test]
    fn maps_what_browser_tvs_name() {
        // UI Events key values (Samsung Tizen's remote, Chromium 69).
        assert_eq!(key("GoBack", ""), Some(RemoteKey::Back));
        assert_eq!(key("ChannelUp", ""), Some(RemoteKey::ChannelUp));
        assert_eq!(key("ChannelDown", ""), Some(RemoteKey::ChannelDown));
        assert_eq!(key("ColorF0Red", ""), Some(RemoteKey::Red));
        assert_eq!(key("ColorF1Green", ""), Some(RemoteKey::Green));
        assert_eq!(key("ColorF2Yellow", ""), Some(RemoteKey::Yellow));
        assert_eq!(key("ColorF3Blue", ""), Some(RemoteKey::Blue));
        assert_eq!(key("Info", ""), Some(RemoteKey::Info));
        assert_eq!(key("Guide", ""), Some(RemoteKey::Guide));
        assert_eq!(key("MediaPlayPause", ""), Some(RemoteKey::PlayPause));
        assert_eq!(key("MediaFastForward", ""), Some(RemoteKey::FastForward));
        assert_eq!(key("MediaStop", ""), Some(RemoteKey::Stop));
        assert_eq!(key("0", "Digit0"), Some(RemoteKey::Digit(0)));
        // A desktop keyboard's stand-ins keep their meaning.
        assert_eq!(key("PageUp", "PageUp"), Some(RemoteKey::ChannelUp));
        assert_eq!(key("F1", "F1"), Some(RemoteKey::Red));
    }

    #[test]
    fn maps_transported_keys() {
        for (code, expected) in [
            ("F13", RemoteKey::ChannelUp),
            ("F14", RemoteKey::ChannelDown),
            ("F15", RemoteKey::Guide),
            ("F16", RemoteKey::Info),
            ("F17", RemoteKey::Red),
            ("F18", RemoteKey::Green),
            ("F19", RemoteKey::Yellow),
            ("F20", RemoteKey::Blue),
            ("F21", RemoteKey::Search),
            ("F22", RemoteKey::Record),
        ] {
            assert_eq!(
                key("Unidentified", code),
                Some(expected),
                "{code} should decode the Android transport",
            );
            assert_eq!(
                key(code, code),
                None,
                "{code} should remain a keyboard function key",
            );
        }
        assert_eq!(
            key("ArrowLeft", "F15"),
            Some(RemoteKey::Left),
            "a known logical key takes priority over a reserved transport code",
        );
    }

    #[test]
    fn ignores_typing_and_chords() {
        assert_eq!(key("a", "KeyA"), None);
        assert_eq!(key("Unidentified", ""), None);
        assert_eq!(key("12", ""), None);
        assert_eq!(key("١", "Digit1"), None);

        for (logical, physical, decoded) in [
            ("ArrowUp", "ArrowUp", RemoteKey::Up),
            ("Unidentified", "F15", RemoteKey::Guide),
        ] {
            let mut event = event(logical, physical);
            for modifiers in [
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
                Modifiers {
                    cmd: true,
                    ..Modifiers::default()
                },
            ] {
                event.modifiers = modifiers;
                assert_eq!(RemoteKey::from_event(&event), None, "{event:?}");
            }
            event.modifiers = Modifiers {
                shift: true,
                ..Modifiers::default()
            };
            assert_eq!(RemoteKey::from_event(&event), Some(decoded), "{event:?}");
        }
    }

    #[test]
    fn remote_hint_labels_cover_digits_channels_and_media_keys() {
        for (logical, physical, label) in [
            ("9", "Digit9", "9"),
            ("Unidentified", "F13", "CH+"),
            ("ChannelDown", "", "CH−"),
            ("MediaPlayPause", "", "Play/Pause"),
            ("MediaFastForward", "", "Fast forward"),
            ("ArrowRight", "ArrowRight", "Right"),
        ] {
            let decoded = key(logical, physical).expect("remote key");
            assert_eq!(decoded.label(), label, "{logical} {physical}");
        }
    }
}
