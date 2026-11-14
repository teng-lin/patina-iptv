//! What the runtime may name. Each list is exact: naming something new fails
//! until it is added here on purpose, and an entry nothing uses any more
//! fails until it is removed, so the lists only change deliberately.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The crates the runtime may depend on: the portable application model,
/// the native services it owns, and what those need. No UI toolkit, shell,
/// player, JNI or platform credential store.
const DEPENDENCIES: &[&str] = &[
    "futures-channel",
    "getrandom",
    "iptv-core",
    "iptv-epg",
    "iptv-guide",
    "iptv-import",
    "iptv-library",
    "iptv-net",
    "iptv-providers",
    "iptv-search",
    "iptv-settings",
    "iptv-store-sqlite",
    "libc",
    "log",
    "serde_json",
    "tokio",
];
/// Test-only additions.
const DEV_DEPENDENCIES: &[&str] = &["rusqlite", "tempfile"];

/// The runtime modules that may name the store adapter: the service that
/// owns its connections, the modules that write store work items, and the
/// two boundary modules that give its values runtime names (`model`,
/// `error`). Everything else, the orchestration in `data` and the owner's
/// named operations included, reaches the store through these.
const STORE_ADAPTER_USERS: &[&str] = &[
    "assign.rs",
    "backup.rs",
    "backup/error.rs",
    "backup/tests.rs",
    "commands.rs",
    "credential_store.rs",
    "error.rs",
    "favorites.rs",
    "imports.rs",
    "library.rs",
    "model.rs",
    "opening.rs",
    "paging.rs",
    "private_files.rs",
    "search.rs",
    "state.rs",
    "store_service.rs",
    "stored.rs",
];

/// Process-lifetime facts that are not owner state. An owner is never found
/// through one of these: every module is handed its owner.
const STATICS: &[(&str, &str)] = &[
    // Unique names of temporary backup files and document requests.
    ("backup.rs", "SEQUENCE"),
    // The first restore-activation outcome of each storage root.
    ("backup.rs", "STARTED"),
    // The operating-system lock this process holds on each storage root.
    ("backup/instance.rs", "HELD"),
    // Unique unopened directories of unit-test owners (`cfg(test)`).
    ("ports.rs", "NEXT"),
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

/// Every runtime source with its path under `src` and its code (comments
/// removed).
fn runtime_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    files
        .into_iter()
        .map(|file| {
            let name = file
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let code = std::fs::read_to_string(&file)
                .unwrap()
                .lines()
                .map(|line| line.split("//").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n");
            (name, code)
        })
        .collect()
}

/// The dependency names of each manifest section.
fn manifest() -> (BTreeSet<String>, BTreeSet<String>) {
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let (mut normal, mut dev) = (BTreeSet::new(), BTreeSet::new());
    let mut section = String::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[') {
            section = name.trim_end_matches(']').to_owned();
            continue;
        }
        let Some(name) = line
            .split(['=', '.', ' '])
            .next()
            .filter(|name| !name.is_empty() && !name.starts_with('#'))
        else {
            continue;
        };
        if section == "dev-dependencies" {
            dev.insert(name.to_owned());
        } else if section.ends_with("dependencies") {
            normal.insert(name.to_owned());
        }
    }
    (normal, dev)
}

fn exact(found: &BTreeSet<String>, listed: &[&str], what: &str) {
    let listed: BTreeSet<String> = listed.iter().map(|name| (*name).to_owned()).collect();
    let unlisted: Vec<_> = found.difference(&listed).collect();
    assert!(
        unlisted.is_empty(),
        "{what}: {unlisted:?} not listed; add on purpose or do without"
    );
    let unused: Vec<_> = listed.difference(found).collect();
    assert!(
        unused.is_empty(),
        "{what}: {unused:?} listed but no longer used; remove from the list"
    );
}

/// The identifiers in `code` that start with `prefix`.
fn identifiers(code: &str, prefix: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let word = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    for (at, _) in code.match_indices(prefix) {
        if code[..at].chars().next_back().is_some_and(word) {
            continue;
        }
        let end = code[at..]
            .find(|ch| !word(ch))
            .map_or(code.len(), |n| at + n);
        found.insert(code[at..end].to_owned());
    }
    found
}

#[test]
fn runtime_depends_only_on_its_listed_crates() {
    let (normal, dev) = manifest();
    exact(&normal, DEPENDENCIES, "runtime dependency");
    exact(&dev, DEV_DEPENDENCIES, "runtime test dependency");
}

#[test]
fn runtime_sources_name_only_listed_workspace_crates_and_no_toolkit() {
    let allowed: BTreeSet<String> = DEPENDENCIES
        .iter()
        .map(|name| name.replace('-', "_"))
        .collect();
    for (name, code) in runtime_sources() {
        for crate_name in identifiers(&code, "iptv_") {
            assert!(
                allowed.contains(&crate_name),
                "{name} names {crate_name}, which the runtime may not depend on"
            );
        }
        let toolkit = identifiers(&code, "nsh_");
        assert!(
            toolkit.is_empty(),
            "{name} names the UI toolkit: {toolkit:?}"
        );
        assert!(!code.contains("crate::Ui"), "{name} names the shell's Ui");
    }
}

#[test]
fn the_store_adapter_is_named_only_by_the_listed_runtime_modules() {
    let users: BTreeSet<String> = runtime_sources()
        .into_iter()
        .filter(|(_, code)| code.contains("iptv_store_sqlite"))
        .map(|(name, _)| name)
        .collect();
    exact(&users, STORE_ADAPTER_USERS, "store adapter user");
}

/// Declarations, including macro bodies and function-local statics;
/// lifetimes such as `&'static str` and `+ 'static` are not declarations.
fn statics(code: &str) -> Vec<&str> {
    let mut result = Vec::new();
    for (at, _) in code.match_indices("static") {
        if at > 0 {
            let before = code.as_bytes()[at - 1];
            if before == b'\'' || before.is_ascii_alphanumeric() || before == b'_' {
                continue;
            }
        }
        let rest = &code[at + "static".len()..];
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let rest = rest.strip_prefix("mut ").unwrap_or(rest).trim_start();
        let end = rest
            .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .unwrap_or(rest.len());
        if end != 0 && rest[end..].trim_start().starts_with(':') {
            result.push(&rest[..end]);
        }
    }
    result
}

#[test]
fn runtime_keeps_no_ambient_owner() {
    let mut found = BTreeSet::new();
    for (name, code) in runtime_sources() {
        assert!(
            !code.contains("thread_local!"),
            "{name}: an owner is passed in, never kept in a thread-local"
        );
        for item in statics(&code) {
            assert!(
                STATICS.contains(&(name.as_str(), item)),
                "{name}: static {item} must belong to a Runtime, or be listed as a \
                 process-lifetime fact that is not owner state"
            );
            found.insert((name.clone(), item.to_owned()));
        }
    }
    for &(file, item) in STATICS {
        assert!(
            found.contains(&(file.to_owned(), item.to_owned())),
            "{file}: {item} was removed; shrink STATICS too"
        );
    }
}

#[test]
fn declarations_include_local_statics_but_not_lifetimes() {
    assert_eq!(statics("fn f() { static mut local: u32 = 0; }"), ["local"]);
    assert_eq!(statics("pub static\n GLOBAL: usize = 0;"), ["GLOBAL"]);
    assert!(statics("fn f() -> &'static str { x } fn g(_: impl Fn() + 'static) {}").is_empty());
    assert_eq!(
        identifiers("use iptv_core::x; my_iptv_core::y; iptv_net::z", "iptv_"),
        ["iptv_core".to_owned(), "iptv_net".to_owned()].into()
    );
}
