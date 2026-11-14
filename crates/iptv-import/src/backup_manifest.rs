//! Portable validation shared by restore and its fuzz/property tests.
//! Parsing never touches files; callers stage only these validated entries.
use serde_json::Value;
use std::collections::BTreeSet;

pub const MAX_MANIFEST: u64 = 4 * 1024 * 1024;
pub const MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const MAX_FILES: usize = 20_000;
#[derive(Debug, PartialEq, Eq)]
pub struct Manifest {
    pub root: String,
    pub files: Vec<Entry>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub length: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub struct EntryLimit;
pub fn entry_limit(name: &str, bytes: u64) -> Result<(), EntryLimit> {
    let max = match name {
        "settings.json" | "search-history.json" => 1 << 20,
        n if matches!(n, "last_channel" | "last_channel_combined")
            || n.starts_with("last_channel-") =>
        {
            64 << 10
        }
        _ => MAX_BYTES,
    };
    if bytes > max { Err(EntryLimit) } else { Ok(()) }
}
/// Whether Windows would store `component` (one name between slashes)
/// under that name. It does not for a name ending in a dot or a space
/// (both are dropped, so `list.` and `list` are the same file), and not for
/// a device name: `NUL`, `nul.txt` and `Com1.tar.gz` all open the device
/// whatever the directory, as the part before the first dot decides. A
/// backup is restored on any platform, so these are refused everywhere.
/// Whether `component` is a file name every supported system can hold:
/// none of Windows' device names (`CON`, `NUL`, `COM1`…, with or without an
/// extension) and no trailing dot or space.
pub fn portable_component(component: &str) -> bool {
    if component.ends_with(['.', ' ']) {
        return false;
    }
    let stem = component.split('.').next().unwrap_or(component);
    let stem = stem.trim_end_matches(' ').to_ascii_uppercase();
    let numbered = |prefix: &str| {
        // Port 0 and the superscript digits are reserved like 1 to 9.
        stem.strip_prefix(prefix).is_some_and(|n| {
            matches!(
                n,
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    };
    !(matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || numbered("COM")
        || numbered("LPT"))
}

pub fn allowed(name: &str) -> bool {
    !name.contains(['\\', ':', '\0'])
        && name
            .split('/')
            .all(|c| !c.is_empty() && c != "." && c != ".." && portable_component(c))
        && (matches!(
            name,
            "iptv.sqlite"
                | "settings.json"
                | "search-history.json"
                | "last_channel"
                | "last_channel_combined"
        ) || name
            .strip_prefix("last_channel-")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            || name.starts_with("playlists/"))
}

/// Normalize an absolute storage path from any supported host without consulting
/// the local filesystem. Windows device namespaces other than verbatim drive and
/// UNC paths are not storage roots. Archive entry names still use `allowed`.
pub fn portable_absolute_path(value: &str) -> Option<String> {
    fn drive_absolute(path: &str) -> bool {
        path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            && path.as_bytes().get(1) == Some(&b':')
            && matches!(path.as_bytes().get(2), Some(b'/' | b'\\'))
    }
    fn unc_component(part: &str) -> bool {
        !part.is_empty()
            && !matches!(part, "." | "..")
            && !part
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    }
    if value.contains('\0') {
        return None;
    }
    let path = if let Some(rest) = value.strip_prefix(r"\\?\") {
        if let Some(unc) = rest.strip_prefix(r"UNC\") {
            format!("//{}", unc.replace('\\', "/"))
        } else if drive_absolute(rest) {
            rest.replace('\\', "/")
        } else {
            return None;
        }
    } else {
        // A single leading backslash is relative to the current Windows drive.
        if value.starts_with('\\') && !value.starts_with(r"\\") {
            return None;
        }
        value.replace('\\', "/")
    };
    let absolute = if let Some(unc) = path.strip_prefix("//") {
        let mut parts = unc.split('/');
        parts.next().is_some_and(unc_component) && parts.next().is_some_and(unc_component)
    } else {
        path.starts_with('/') || drive_absolute(&path)
    };
    absolute.then_some(path)
}

pub fn parse(bytes: &[u8]) -> Result<Manifest, &'static str> {
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err("Backup header is too large");
    }
    let manifest: Value = serde_json::from_slice(bytes).map_err(|_| "Backup header is invalid")?;
    if manifest["format"] != "patina-whole-app" || manifest["version"] != 1 {
        return Err("Backup format is not supported by this version");
    }
    let root = manifest["root"]
        .as_str()
        .ok_or("Backup storage path is missing")?;
    if portable_absolute_path(root).is_none() {
        return Err("Backup storage path is invalid");
    }
    let files = manifest["files"]
        .as_array()
        .ok_or("Backup file list is missing")?;
    if files.len() > MAX_FILES {
        return Err("Backup has too many files");
    }
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    let mut out = Vec::new();
    for entry in files {
        let name = entry["name"]
            .as_str()
            .ok_or("Backup file name is invalid")?;
        if !allowed(name) || !seen.insert(name) {
            return Err("Backup contains an unsafe or duplicate file path");
        }
        let length = entry["length"]
            .as_u64()
            .ok_or("Backup file size is invalid")?;
        entry_limit(name, length).map_err(|_| "Backup structured entry is too large")?;
        total = total.checked_add(length).ok_or("Backup is too large")?;
        if total > MAX_BYTES {
            return Err("Backup is too large");
        }
        out.push(Entry {
            name: name.into(),
            length,
        });
    }
    if !seen.contains("iptv.sqlite") || !seen.contains("settings.json") {
        return Err("Backup is missing the library or settings");
    }
    Ok(Manifest {
        root: root.into(),
        files: out,
    })
}

#[cfg(test)]
mod windows_tests {
    use super::*;
    #[test]
    fn backup_roots_accept_absolute_storage_paths_but_not_device_or_relative_paths() {
        for (root, valid) in [
            (r"C:\Users\Synthetic\Patina", true),
            ("C:/Patina", true),
            (r"\\synthetic-server\share\Patina", true),
            ("//synthetic-server/share/Patina", true),
            (r"\\?\C:\Users\Synthetic\Patina", true),
            (r"\\?\UNC\synthetic-server\share\Patina", true),
            (r"\\synthetic-server\share", true),
            ("C:Patina", false),
            ("Patina", false),
            (r"\Patina", false),
            (r"\\synthetic-server", false),
            (r"\\synthetic-server\", false),
            (r"\\?\UNC\synthetic-server", false),
            (r"\\?\UNC\\share\Patina", false),
            (r"\\?\C:Patina", false),
            (r"\\?\Patina", false),
            (r"\\?\GLOBALROOT\Device\Synthetic", false),
            (r"\\.\C:\Patina", false),
            ("//./C:/Patina", false),
            ("//?/C:/Patina", false),
            ("C:/Patina\0", false),
            ("/home/synthetic", true),
        ] {
            let bytes = serde_json::to_vec(&serde_json::json!({
                "format": "patina-whole-app", "version": 1, "root": root,
                "files": [{"name": "iptv.sqlite", "length": 0}, {"name": "settings.json", "length": 0}]
            })).unwrap();
            assert_eq!(parse(&bytes).is_ok(), valid, "{root}");
        }
    }

    #[test]
    fn windows_device_names_and_trailing_dots_or_spaces_are_not_archive_entries() {
        let manifest = |name: &str| {
            serde_json::to_vec(&serde_json::json!({
                "format": "patina-whole-app", "version": 1, "root": "/home/synthetic",
                "files": [
                    {"name": "iptv.sqlite", "length": 0},
                    {"name": "settings.json", "length": 0},
                    {"name": name, "length": 0}
                ]
            }))
            .unwrap()
        };
        for name in [
            "playlists/CON",
            "playlists/con",
            "playlists/Prn",
            "playlists/AUX",
            "playlists/nul",
            "playlists/NUL.m3u",
            "playlists/aux.tar.gz",
            "playlists/con .m3u",
            "playlists/COM1",
            "playlists/com9.m3u",
            "playlists/Com5.txt",
            "playlists/COM0",
            "playlists/COM\u{b9}",
            "playlists/LPT1",
            "playlists/lpt9.m3u",
            "playlists/LPT\u{b3}.m3u",
            "playlists/CONIN$",
            "playlists/conout$.m3u",
            "playlists/0123456789abcdef/aux.m3u",
            "playlists/nul/list.m3u",
            "playlists/list.m3u.",
            "playlists/list.m3u ",
            "playlists/list.",
            "playlists/folder./list.m3u",
            "playlists/folder /list.m3u",
            "playlists/...",
        ] {
            assert!(!allowed(name), "{name}");
            assert_eq!(
                parse(&manifest(name)),
                Err("Backup contains an unsafe or duplicate file path"),
                "{name}"
            );
        }
        // Names that only resemble them are ordinary files.
        for name in [
            "iptv.sqlite",
            "settings.json",
            "search-history.json",
            "last_channel",
            "last_channel_combined",
            "last_channel-12",
            "playlists/0123456789abcdef/list.m3u",
            "playlists/console.m3u",
            "playlists/nullable.m3u",
            "playlists/auxiliary",
            "playlists/com10.m3u",
            "playlists/com.m3u",
            "playlists/lpt.m3u",
            "playlists/lpt12",
            "playlists/my.con",
            "playlists/list.nul.m3u",
            "playlists/_con.m3u",
            "playlists/a b.m3u",
            "playlists/.hidden.m3u",
        ] {
            assert!(allowed(name), "{name}");
            // The manifest above already lists the two required files.
            if !matches!(name, "iptv.sqlite" | "settings.json") {
                assert!(parse(&manifest(name)).is_ok(), "{name}");
            }
        }
    }

    #[test]
    fn unc_storage_roots_do_not_allow_absolute_or_traversing_archive_entries() {
        for name in [
            r"\\synthetic-server\share\outside.m3u",
            r"\\?\C:\outside.m3u",
            "C:/outside.m3u",
            "/outside.m3u",
            "playlists/../outside.m3u",
            r"playlists\outside.m3u",
            "playlists/drive:stream",
        ] {
            let bytes = serde_json::to_vec(&serde_json::json!({
                "format": "patina-whole-app", "version": 1,
                "root": r"\\synthetic-server\share\Patina",
                "files": [
                    {"name": "iptv.sqlite", "length": 0},
                    {"name": "settings.json", "length": 0},
                    {"name": name, "length": 0}
                ]
            }))
            .unwrap();
            assert_eq!(
                parse(&bytes),
                Err("Backup contains an unsafe or duplicate file path"),
                "{name}"
            );
        }
    }
}
