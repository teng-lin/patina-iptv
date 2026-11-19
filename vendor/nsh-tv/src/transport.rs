//! How the Android TV host hands on the remote keys stock NSH has no name for.
//!
//! Stock NSH's Android backend names a key by its key code
//! (`KEYCODE_DPAD_UP` is `"ArrowUp"`, `KEYCODE_MEDIA_PLAY_PAUSE` is
//! `"MediaPlayPause"`) and reports `"Unidentified"` for the rest, among them
//! a TV remote's channel, guide, info, colour, search and record keys. Its
//! event's `code` comes from the scan code alone (the kernel's evdev code).
//!
//! The extension's host (`dev.nshtv.RemoteKeys`) keeps such a key's key code
//! (Android's own handling and the system see the real key) and replaces only
//! its scan code with the evdev code of a function key no remote sends
//! (F13 to F22). The raw event then arrives as `key: "Unidentified"`,
//! `code: "F13"`…, which [`decode`] turns back into the [`RemoteKey`].
//! [`TABLE`] is the one list both sides follow; a test checks the host's
//! Java copy against it.

use crate::RemoteKey;

/// One transported key: the remote key, Android's key code
/// (`KeyEvent.KEYCODE_*`), the evdev scan code the host puts in its place and
/// the DOM `code` stock NSH reports for that scan code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transported {
    /// The key the app gets.
    pub key: RemoteKey,
    /// Android's key code of the remote's key.
    pub android_key_code: i32,
    /// The evdev scan code the host reports instead of the remote's own.
    pub scan_code: i32,
    /// What stock NSH reports as the raw event's `code` for `scan_code`.
    pub dom_code: &'static str,
}

const fn t(
    key: RemoteKey,
    android_key_code: i32,
    scan_code: i32,
    dom_code: &'static str,
) -> Transported {
    Transported {
        key,
        android_key_code,
        scan_code,
        dom_code,
    }
}

/// Every transported key. Append only: a host and an app built at different
/// times must agree.
pub const TABLE: [Transported; 10] = [
    t(RemoteKey::ChannelUp, 166, 183, "F13"),
    t(RemoteKey::ChannelDown, 167, 184, "F14"),
    t(RemoteKey::Guide, 172, 185, "F15"),
    t(RemoteKey::Info, 165, 186, "F16"),
    t(RemoteKey::Red, 183, 187, "F17"),
    t(RemoteKey::Green, 184, 188, "F18"),
    t(RemoteKey::Yellow, 185, 189, "F19"),
    t(RemoteKey::Blue, 186, 190, "F20"),
    t(RemoteKey::Search, 84, 191, "F21"),
    t(RemoteKey::Record, 130, 192, "F22"),
];

/// The remote key a raw event's `key` / `code` pair stands for, when the
/// host transported it. A real F13 key of a keyboard reports `key: "F13"`
/// and is not taken for one.
pub fn decode(key: &str, code: &str) -> Option<RemoteKey> {
    if key != "Unidentified" {
        return None;
    }
    TABLE.iter().find(|t| t.dom_code == code).map(|t| t.key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_only_unidentified_keys() {
        assert_eq!(decode("Unidentified", "F13"), Some(RemoteKey::ChannelUp));
        assert_eq!(decode("Unidentified", "F20"), Some(RemoteKey::Blue));
        assert_eq!(decode("F13", "F13"), None, "a keyboard's real F13");
        assert_eq!(decode("Unidentified", ""), None);
        assert_eq!(decode("Unidentified", "KeyA"), None);
    }

    #[test]
    fn the_table_is_one_to_one() {
        for (i, a) in TABLE.iter().enumerate() {
            for b in &TABLE[i + 1..] {
                assert_ne!(a.key, b.key);
                assert_ne!(a.android_key_code, b.android_key_code);
                assert_ne!(a.scan_code, b.scan_code);
                assert_ne!(a.dom_code, b.dom_code);
            }
        }
    }

    /// The host's Java table (`RemoteKeys.TRANSPORT`) is this one.
    #[test]
    fn the_android_host_agrees() {
        let java =
            include_str!("../../../hosts/android/app/src/main/java/dev/nshtv/RemoteKeys.java");
        let start = java.find("TRANSPORT = {").expect("RemoteKeys.TRANSPORT");
        let body = &java[start..java[start..].find("};").unwrap() + start];
        let rows: Vec<(i32, i32)> = body
            .lines()
            .filter_map(|line| {
                let line = line.trim().strip_prefix('{')?;
                let line = &line[..line.find('}')?];
                let mut parts = line.split(',').map(|p| p.trim().parse::<i32>().unwrap());
                Some((parts.next()?, parts.next()?))
            })
            .collect();
        let ours: Vec<(i32, i32)> = TABLE
            .iter()
            .map(|t| (t.android_key_code, t.scan_code))
            .collect();
        assert_eq!(rows, ours);
    }
}
