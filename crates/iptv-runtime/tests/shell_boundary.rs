//! What the shell may name from the runtime. The list is exact: a new name
//! fails until it is added here on purpose, and an entry the shell no longer
//! uses fails until it is removed.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The runtime's items the shell names, by their first path segment. The
/// shell holds one `Runtime` handle and passes it to every call; it names no
/// store service, worker pool, executor, file writer or owner lookup.
const SHELL_MAY_NAME: &[&str] = &[
    // The owner, how it is built, and what crosses its boundary.
    "BrowserState",
    "Error",
    "HostHandle",
    "Ports",
    "Runtime",
    "RuntimeEvent",
    // Backup progress, errors and the operation in progress.
    "backup",
    // Named library operations, each given the owner.
    "commands",
    // Import, library, combined-browser and guide orchestration.
    "data",
    // Guide-source list edits (pure).
    "guides",
    // Value types of the commands and events.
    "model",
    // A stored source's playback context before its refresh (pure).
    "playback",
    // Effective setting values and the save error.
    "settings",
    // Private-directory protection, before any owner exists.
    "storage",
    // The guide status and the lines of the guide sources screen.
    "stored",
];

fn sources(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn shell_sources() -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../iptv-app/src"),
        &mut files,
    );
    files
        .into_iter()
        .map(|file| {
            let code = std::fs::read_to_string(&file)
                .unwrap()
                .lines()
                .map(|line| line.split("//").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n");
            (file, code)
        })
        .collect()
}

/// The first path segments named after the shell's `runtime::` alias (and
/// the crate's own name), with `{a, b::c}` groups expanded. Another crate's
/// module of the same name (`tokio::runtime::`) is not the alias.
fn named(code: &str) -> BTreeSet<String> {
    let word = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    let mut found = BTreeSet::new();
    for alias in ["iptv_runtime::", "runtime::"] {
        for (at, _) in code.match_indices(alias) {
            let before = &code[..at];
            if before.chars().next_back().is_some_and(word) {
                continue;
            }
            if before.ends_with("::") && !before.ends_with("crate::") {
                continue;
            }
            let rest = &code[at + alias.len()..];
            let first = |text: &str| {
                let text = text.trim_start();
                let end = text.find(|ch| !word(ch)).unwrap_or(text.len());
                text[..end].to_owned()
            };
            if let Some(group) = rest.strip_prefix('{') {
                let mut depth = 0usize;
                let mut item = String::new();
                for ch in group.chars() {
                    match ch {
                        '{' => depth += 1,
                        '}' if depth == 0 => break,
                        '}' => depth -= 1,
                        ',' if depth == 0 => {
                            found.insert(first(&item));
                            item.clear();
                            continue;
                        }
                        _ => {}
                    }
                    item.push(ch);
                }
                found.insert(first(&item));
            } else {
                found.insert(first(rest));
            }
        }
    }
    found.remove("");
    found
}

#[test]
fn the_shell_names_only_the_listed_runtime_items() {
    let mut found = BTreeSet::new();
    for (file, code) in shell_sources() {
        for item in named(&code) {
            assert!(
                SHELL_MAY_NAME.contains(&item.as_str()),
                "{} names runtime::{item}; pass the owner to a listed operation, \
                 or list the item on purpose",
                file.display()
            );
            found.insert(item);
        }
    }
    for item in SHELL_MAY_NAME {
        assert!(
            found.contains(*item),
            "runtime::{item} is no longer named by the shell; remove it from the list"
        );
    }
}

#[test]
fn the_shell_names_no_store_type_and_does_not_depend_on_the_adapter() {
    for (file, code) in shell_sources() {
        for forbidden in ["iptv_store_sqlite", "store_service"] {
            assert!(
                !code.contains(forbidden),
                "{} names {forbidden}",
                file.display()
            );
        }
    }
    // Only its tests may open a library directly.
    let manifest = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../iptv-app/Cargo.toml"),
    )
    .unwrap();
    let mut section = "";
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
        } else if line.starts_with("iptv-store-sqlite") {
            assert_eq!(
                section, "[dev-dependencies]",
                "the shell depends on the store adapter outside its tests"
            );
        }
    }
}

#[test]
fn shell_only_handles_runtime_events_at_its_presentation_entry_point() {
    for (file, code) in shell_sources() {
        if file.file_name().unwrap() != "runtime_events.rs" {
            assert!(
                !code.contains("RuntimeEvent::"),
                "{} produces or handles runtime events outside the presentation entry point",
                file.display()
            );
        }
    }
}

#[test]
fn path_segments_are_read_from_plain_grouped_and_nested_uses() {
    let code = "use crate::runtime::{Error, model::DatabaseKey};\n\
                crate::runtime::data::start(&crate::runtime());\n\
                use iptv_runtime as runtime; let x: runtime::Runtime;\n\
                tokio::runtime::Builder::new_current_thread();\n\
                pub use crate::runtime::data::{assign, combined::{self, owner}};";
    assert_eq!(
        named(code),
        ["Error", "Runtime", "data", "model"]
            .map(str::to_owned)
            .into()
    );
}
