//! Migration inventory for names that currently cross the intended layers.
//! The temporary scan tracks explicit names and imports. Cargo checks the
//! actual boundary when presentation becomes a separate crate.

use std::{collections::BTreeSet, fs, path::Path};

const LOWER: &[&str] = &[
    "iptv-core",
    "iptv-epg",
    "iptv-guide",
    "iptv-import",
    "iptv-library",
    "iptv-net",
    "iptv-player-mpv",
    "iptv-providers",
    "iptv-runtime",
    "iptv-search",
    "iptv-settings",
    "iptv-sqlcipher-init",
    "iptv-store-sqlite",
];

// Fully presentation modules (or modules waiting to split), and the
// presentation names exported by files that also contain domain values.
const NAMES: &[(&str, &[&str])] = &[
    ("iptv_core::browse", &[]),
    ("iptv_core::browse_playback", &[]),
    (
        "iptv_core::capabilities",
        &["pip", "multiview", "Availability", "Reason"],
    ),
    ("iptv_core::channel_list", &[]),
    ("iptv_core::channel_number", &[]),
    ("iptv_core::channel_picker", &[]),
    ("iptv_core::clock", &[]),
    ("iptv_core::editor", &[]),
    ("iptv_core::favorites", &["History"]),
    (
        "iptv_core::guide",
        &[
            "Focus",
            "Move",
            "Pane",
            "PreviewOrigin",
            "ProgramSelection",
            "home_window",
            "moves",
            "page_row",
            "view_range",
            "visible_programs",
            "STEP",
        ],
    ),
    ("iptv_core::heard", &[]),
    ("iptv_core::i18n", &[]),
    ("iptv_core::interruption", &["Interruptions", "Response"]),
    ("iptv_core::layout", &[]),
    ("iptv_core::library_recovery", &[]),
    (
        "iptv_core::lists",
        &[
            "ALL",
            "FAVORITES",
            "RECENT",
            "FIRST_GROUP",
            "Entry",
            "ListId",
            "ChannelList",
            "ListView",
            "entry_count",
            "entry_name",
            "resume_entry",
        ],
    ),
    ("iptv_core::membership", &["Membership"]),
    ("iptv_core::menu", &[]),
    ("iptv_core::on_demand", &["Seek", "Timeline"]),
    (
        "iptv_core::multiview",
        &[
            "Dialog",
            "Key",
            "Layout",
            "Limits",
            "Multiview",
            "Outcome",
            "Pick",
            "Rect",
            "THUMB",
            "Tile",
            "TileAction",
            "VIDEO",
            "WARNING_CHOICES",
            "WarningChoice",
        ],
    ),
    (
        "iptv_core::organization",
        &[
            "Action",
            "Editor",
            "Mode",
            "ACTIONS",
            "GROUP_SORTS",
            "LEAVE_CHOICES",
        ],
    ),
    ("iptv_core::panels", &[]),
    ("iptv_core::parental", &["Dialog", "Phase", "Submission"]),
    ("iptv_core::pip", &[]),
    ("iptv_core::pip_window", &[]),
    ("iptv_core::play_bar", &[]),
    (
        "iptv_core::playback",
        &[
            "ChannelHistory",
            "Health",
            "ZapOrigin",
            "group_range",
            "zap_from_list",
            "zap_target",
            "RECENT_MAX",
        ],
    ),
    ("iptv_core::playback_windows", &[]),
    ("iptv_core::player_info", &[]),
    ("iptv_core::player_options", &[]),
    (
        "iptv_core::recording",
        &["Screen", "Draft", "EntryField", "Entry"],
    ),
    (
        "iptv_core::reorder",
        &[
            "Reorder",
            "Key",
            "Outcome",
            "Exit",
            "Hint",
            "HintAction",
            "HintKey",
            "LeaveChoice",
            "SubMode",
        ],
    ),
    ("iptv_core::seek_hold", &[]),
    ("iptv_core::sound", &[]),
    ("iptv_core::timeline", &[]),
    (
        "iptv_core::tracks",
        &[
            "Choice",
            "audio_layout",
            "audio_layout_in",
            "bitrate_label",
            "codec_name",
            "entries",
            "entries_in",
            "fps_label",
            "language_name",
            "language_name_in",
            "resolution_class",
            "summary",
            "summary_in",
        ],
    ),
    (
        "iptv_core::vod",
        &[
            "Screen",
            "Control",
            "Operation",
            "Retried",
            "Destination",
            "Handoff",
            "library",
        ],
    ),
    ("iptv_core::wizard", &[]),
    ("iptv_library::details", &[]),
    ("iptv_library::epg_sources", &[]),
    ("iptv_library::guide_assign", &[]),
    ("iptv_library::guide_inspect", &[]),
    ("iptv_library::guide_repair_screen", &[]),
    ("iptv_library::playlists", &[]),
    ("iptv_search::selection", &[]),
    ("iptv_search::state", &[]),
    ("iptv_settings::screen", &[]),
];

fn rust_files(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("source directory entry").path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn flatten_imports(code: &str) -> String {
    fn expand(path: &str, prefix: &str, paths: &mut String) {
        if let Some((base, group)) = path.split_once('{') {
            let prefix = format!("{prefix}{}", base.trim());
            let Some(group) = group.trim_end().strip_suffix('}') else {
                return;
            };
            let mut depth = 0;
            let mut start = 0;
            for (i, byte) in group.bytes().enumerate() {
                match byte {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    b',' if depth == 0 => {
                        expand(&group[start..i], &prefix, paths);
                        start = i + 1;
                    }
                    _ => {}
                }
            }
            expand(&group[start..], &prefix, paths);
        } else if let Some(item) = path.split_whitespace().next() {
            paths.push_str(&format!(" {prefix}{item}"));
        }
    }

    let mut paths = String::new();
    for statement in code.split(';') {
        if let Some((_, path)) = statement.rsplit_once("use ") {
            expand(path, "", &mut paths);
        }
    }
    paths
}

fn names_in(code: &str, path: &str, local: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let source = format!("{code} {}", flatten_imports(code));
    let identifier = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for (module, items) in NAMES {
        let (owner, leaf) = module.split_once("::").expect("qualified module");
        for (start, _) in source.match_indices(module) {
            let before = &source[..start];
            let following = &source[start + module.len()..];
            if before.ends_with(identifier) || following.starts_with(identifier) {
                continue;
            }
            if items.is_empty() {
                names.insert((*module).to_owned());
            } else if let Some(suffix) = following.strip_prefix("::") {
                let item = suffix.split(|c| !identifier(c)).next().unwrap_or_default();
                if items.contains(&item) {
                    names.insert(format!("{module}::{item}"));
                }
            }
        }
        // A lower crate's own state holder is still a departure.
        if items.is_empty() && owner == local {
            let declaration = format!("mod {leaf};");
            let internal = format!("crate::{leaf}::");
            if code.contains(&declaration) || code.contains(&internal) {
                names.insert((*module).to_owned());
            }
        }
    }
    if path == "iptv-runtime/src/commands.rs" && code.contains("editor: Editor") {
        names.insert("edit_organization takes Editor".into());
    }
    if path == "iptv-runtime/src/operations.rs" && code.contains("fn favorites_history") {
        names.insert("favorites_history returns History".into());
    }
    names
}

fn core_reexports(source: &str) -> impl Iterator<Item = (&str, &str)> {
    source.lines().filter_map(|line| {
        let (kind, target) = if let Some(target) = line.strip_prefix("pub use crate::") {
            ("shipped", target)
        } else {
            ("test", line.strip_prefix("pub(crate) use crate::")?)
        };
        let module = target.split("::").next()?;
        (module.ends_with("_state") || matches!(module, "playback_health" | "track_labels" | "zap"))
            .then_some((kind, module))
    })
}

#[test]
fn lower_layers_name_only_the_inventoried_presentation_items() {
    // The scanner uses the source tree, so it is irrelevant to the wasm
    // execution environment. The same crates are compiled there separately.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory");
    let mut actual = BTreeSet::new();
    for &name in LOWER {
        let mut files = Vec::new();
        rust_files(&root.join(name).join("src"), &mut files);
        rust_files(&root.join(name).join("tests"), &mut files);
        for file in files {
            let path = file
                .strip_prefix(root)
                .expect("path under crates")
                .to_string_lossy();
            let path = path.replace('\\', "/");
            let source = fs::read_to_string(&file).expect("read inventoried source");
            if name == "iptv-core" {
                for (kind, module) in core_reexports(&source) {
                    actual.insert(format!("{kind} {path} re-exports {module}"));
                }
                // Schedule's remaining language arguments are the final
                // dependencies of the domain on localization during the split.
                if path == "iptv-core/src/guide.rs" && source.contains("crate::i18n::Language") {
                    actual.insert(format!("shipped {path} iptv_core::i18n::Language"));
                }
                continue;
            }
            let filename = file
                .file_stem()
                .expect("source file name")
                .to_string_lossy();
            let test_file =
                path.contains("/tests/") || filename == "tests" || filename.ends_with("_tests");
            let code: String = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .map(|line| format!("{line}\n"))
                .collect();
            let (production, tests) = if test_file {
                ("", code.as_str())
            } else {
                code.split_once("#[cfg(test)]\nmod tests {")
                    .unwrap_or((&code, ""))
            };
            for (kind, code) in [("shipped", production), ("test", tests)] {
                let code = code.split_whitespace().collect::<Vec<_>>().join(" ");
                for entry in names_in(&code, &path, &name.replace('-', "_")) {
                    actual.insert(format!("{kind} {path} {entry}"));
                }
            }
        }
    }

    let expected: BTreeSet<_> = include_str!("layer_inventory.txt")
        .lines()
        .map(str::to_owned)
        .collect();
    let added: Vec<_> = actual.difference(&expected).collect();
    let removed: Vec<_> = expected.difference(&actual).collect();
    assert!(
        added.is_empty() && removed.is_empty(),
        "new entries:\n{}\nstale entries:\n{}",
        added
            .iter()
            .map(|line| line.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        removed
            .iter()
            .map(|line| line.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}
