//! The type scale cannot drift: text sizes come from `fonts.rs` and the
//! secondary screens' kit (`ui_kit.rs`, `list_rows.rs`), scaled with the
//! window (`scale.rs`). A production file that sets a literal size
//! (`text_size(px(15.0))`, `text_size(move || px(15.0))`) fails here.
//!
//! [`LATER`] lists the files left for a later pass. It only shrinks: move
//! a file onto the kit, then take it off the list (a listed file without
//! literals fails too, so the list stays true).

use std::path::{Path, PathBuf};

/// Files that still set literal sizes, relative to `src/`.
const LATER: &[&str] = &[
    // The play bar: another workstream is changing it; it gets its own
    // pass.
    "playback/play_bar/progress.rs",
    // The guide's header (the list button, the debug overlay) and the
    // grid's empty state: the guide-navigation workstream's files.
    "guide/header.rs",
    "guide/grid.rs",
];

/// Where the sizes are defined.
const OWNERS: &[&str] = &["fonts.rs", "ui_kit.rs", "guide/list_rows.rs"];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The lines of `code` (before its unit tests) that set a literal size.
fn literal_sizes(code: &str) -> Vec<usize> {
    let code = code.split("#[cfg(test)]").next().unwrap_or("");
    let compact: Vec<(usize, String)> = code
        .lines()
        .enumerate()
        .map(|(n, l)| (n + 1, l.split_whitespace().collect::<String>()))
        .collect();
    let mut hits = Vec::new();
    for (n, line) in &compact {
        for prefix in ["text_size(px(", "text_size(move||px("] {
            let mut rest = line.as_str();
            while let Some(at) = rest.find(prefix) {
                rest = &rest[at + prefix.len()..];
                if rest.starts_with(|c: char| c.is_ascii_digit()) || rest.starts_with("if") {
                    hits.push(*n);
                }
            }
        }
    }
    hits
}

#[test]
fn text_sizes_come_from_the_type_scale() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    let mut drift = Vec::new();
    let mut done = Vec::new();
    for file in files {
        let name = file
            .strip_prefix(&src)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if OWNERS.contains(&name.as_str()) {
            continue;
        }
        let hits = literal_sizes(&std::fs::read_to_string(&file).unwrap());
        match (LATER.contains(&name.as_str()), hits.is_empty()) {
            (false, false) => drift.push(format!("{name}: lines {hits:?}")),
            (true, true) => done.push(name),
            _ => {}
        }
    }
    assert!(
        drift.is_empty(),
        "literal text sizes (use `fonts.rs` and `ui_kit.rs`): {drift:#?}"
    );
    assert!(
        done.is_empty(),
        "on the type scale now: take them off `LATER`: {done:?}"
    );
}

#[test]
fn literals_are_found() {
    assert_eq!(literal_sizes("a\n.text_size(px(15.0))"), vec![2]);
    assert_eq!(literal_sizes(".text_size(move || px(13.0))"), vec![1]);
    assert_eq!(
        literal_sizes(".text_size(move || px(if h { 12.0 } else { 15.0 }))"),
        vec![1]
    );
    assert!(literal_sizes(".text_size(move || size.px(CAPTION))").is_empty());
    assert!(literal_sizes(".text_size(px(TEXT_SIZE))").is_empty());
    assert!(literal_sizes("#[cfg(test)]\n.text_size(px(9.0))").is_empty());
}
