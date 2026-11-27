//! What the runtime may name. Each list is exact: naming something new fails
//! until it is added here on purpose, and an entry nothing uses any more
//! fails until it is removed, so the lists only change deliberately.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The crates the runtime may depend on: the portable application model,
/// the native services it owns, and what those need. No UI toolkit, shell,
/// player, JNI or platform credential store.
const DEPENDENCIES: &[&str] = &[
    // Native update metadata transport and verification, plus atomic trust-state replacement.
    "futures-util",
    "rustix", // Linux AppImage backup links, permission checks and atomic exchange.
    "tempfile",
    "tough",
    "url",
    "chrono", // Native recurring schedules preserve local wall-clock time.
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
    "pbkdf2", // Native PIN/recovery verification; hashing stays off the UI thread.
    "serde_json",
    "sha2",
    "subtle", // Constant-time verifier comparison.
    "tokio",
    "zeroize", // Short-lived native credential material.
];
/// The modules only a native build has (the `native` feature): the native
/// ports and the recording services. Only these may name the native
/// network adapter or start threads and Tokio runtimes; everything else
/// reaches them through the network and execution ports.
const NATIVE_ONLY: &[&str] = &["native.rs", "native/", "recordings.rs", "recordings/"];

/// Test-only additions.
const DEV_DEPENDENCIES: &[&str] = &["rusqlite", "tempfile"];

/// The runtime modules that may name the store adapter: the service that
/// owns its connections, the modules that write store work items, and the
/// two boundary modules that give its values runtime names (`model`,
/// `error`). Everything else, the orchestration in `data` and the owner's
/// named operations included, reaches the store through these.
const STORE_ADAPTER_USERS: &[&str] = &[
    "assign.rs",
    "guide_repair.rs",
    "guide_repair/prepare.rs",
    "guide_repair/publication.rs",
    "guide_repair/tests.rs",
    "guide_repair/write.rs",
    "backup.rs",
    "backup/error.rs",
    "backup/tests.rs",
    "commands.rs",
    "credential_store.rs",
    "error.rs",
    "favorites.rs",
    "imports.rs",
    "inspection.rs",
    "inspection/probe.rs",
    "library.rs",
    "model.rs",
    "opening.rs",
    "opening/revocation.rs",
    "guide_checks/retry.rs",
    "paging.rs",
    "parental.rs",
    "private_files.rs",
    // The sliced publish where the host keeps the store, and that host's
    // port (opening, persistence, the publish copy).
    "publish.rs",
    "publish/tests/catalog_import_tests.rs", // Encrypted fixture for the cooperative publication driver.
    "store_host.rs",
    "recordings.rs",
    "recordings/completion.rs",
    "recordings/shared.rs", // Owned transport preparation queues durable authority reads.
    "recordings/shared_tests.rs", // Synthetic ownership and authority fixtures.
    "recordings/tests.rs",
    "search.rs",
    "state.rs",
    "store_service.rs",
    "stored.rs",
    "vod.rs",
    "vod/recovery_tests.rs",
];

/// Ratchet the owner shape as PR2 extracts domain state. Includes fields
/// behind `test-support`, matching the source inventory rather than one build.
const RUNTIME_FIELD_LIMIT: usize = 41;

/// Process-lifetime facts and explicitly opted-in native recording services.
/// UI Runtime owners are never discovered globally. The recording registry
/// intentionally survives UI engine disposal and is keyed by protected storage
/// directory; Ports defaults to isolated recording state for other consumers.
const STATICS: &[(&str, &str)] = &[
    // Unique names of temporary backup files and document requests.
    ("backup.rs", "SEQUENCE"),
    // The first restore-activation outcome of each storage root.
    ("backup.rs", "STARTED"),
    // The operating-system lock this process holds on each storage root.
    ("backup/instance.rs", "HELD"),
    // A browser host's clocks (no OS clock there), installed once.
    ("time.rs", "CLOCK"),
    // The standard clock's origin for unit tests of a browser build (`cfg(test)`).
    ("time.rs", "ORIGIN"),
    // Unique unopened directories of unit-test owners (`cfg(test)`).
    ("native.rs", "NEXT"),
    // Startup marks seen by one opening unit test (`cfg(test)`): the mark
    // port is a function pointer.
    ("opening/revocation.rs", "MARKS"),
    // Per-tree filesystem publication gates shared by independent native services.
    ("private_files.rs", "TREES"),
    // Serialize native service startup/restore and retain its independent owner.
    ("recordings/host.rs", "ACTIVATION"),
    ("recordings/host.rs", "HOSTS"),
    // Synthetic callback handoff barrier used only by recording unit tests.
    ("recordings/tests.rs", "PUBLICATION_GATE"),
    // Function-pointer host ports need process-local test observation/gates.
    // This module is declared only under cfg(test), never in a shipped runtime.
    ("recordings/tests.rs", "CALLS"),
    ("recordings/tests.rs", "GATE"),
    ("recordings/tests.rs", "PUBLISHED"),
    // Separate fixtures for the function-pointer notification port, each
    // scoped to the recording test that writes and checks it.
    ("recordings/tests.rs", "STANDBY_NOTICES"),
    ("recordings/tests.rs", "OPEN_FAILURE_NOTICES"),
    ("recordings/tests.rs", "EXPIRED_CAPTURE_NOTICES"),
    ("recordings/tests.rs", "CAPACITY_FAILURE_NOTICES"),
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

fn runtime_fields() -> BTreeSet<String> {
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs")).unwrap();
    let body = source
        .split_once("pub struct Runtime {")
        .expect("Runtime declaration")
        .1;
    let mut depth = 1_i32;
    let mut fields = BTreeSet::new();
    for line in body.lines() {
        if depth == 1 && line.starts_with("    ") && !line.starts_with("        ") {
            let line = line.trim();
            if !line.starts_with("#[")
                && let Some((field, _)) = line.split_once(':')
                && field.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_')
                && !field.is_empty()
            {
                fields.insert(field.to_owned());
            }
        }
        depth += line.matches('{').count() as i32;
        depth -= line.matches('}').count() as i32;
        if depth == 0 {
            break;
        }
    }
    fields
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

#[test]
fn runtime_field_count_only_shrinks() {
    let fields = runtime_fields();
    assert_eq!(
        fields.len(),
        RUNTIME_FIELD_LIMIT,
        "Runtime fields changed; extract them into cohesive per-domain owners and update the ratchet only as PR2 progresses: {fields:?}"
    );
}

#[test]
fn stored_guide_and_combined_state_leave_orchestration_to_data() {
    let sources = runtime_sources();
    for (name, forbidden) in [
        ("data/stored.rs", &["super::", "crate::data"][..]),
        (
            "data/combined.rs",
            &["super::", "crate::data", "library::", "stored::"][..],
        ),
    ] {
        let (_, code) = sources.iter().find(|(path, _)| path == name).unwrap();
        for upward in forbidden {
            assert!(!code.contains(upward), "{name} depends on {upward}");
        }
    }
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
fn runtime_keeps_no_ambient_ui_owner() {
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
                 process-lifetime fact or explicit native service"
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

fn native_only(name: &str) -> bool {
    NATIVE_ONLY.iter().any(|prefix| {
        if prefix.ends_with('/') {
            name.starts_with(prefix)
        } else {
            name == *prefix
        }
    })
}

/// The source files of modules declared `#[cfg(test)] mod name;`.
fn test_modules(sources: &[(String, String)]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (name, code) in sources {
        let parent = match name.as_str() {
            "lib.rs" => String::new(),
            other => format!("{}/", other.trim_end_matches(".rs")),
        };
        let mut lines = code.lines().map(str::trim).peekable();
        while let Some(line) = lines.next() {
            if (line == "#[cfg(test)]" || line.starts_with("#[cfg(all(test,"))
                && let Some(module) = lines
                    .peek()
                    .and_then(|next| next.strip_prefix("mod "))
                    .and_then(|rest| rest.strip_suffix(';'))
            {
                found.insert(format!("{parent}{module}.rs"));
            }
        }
    }
    found
}

/// `code` without its inline unit-test modules (`#[cfg(test)] mod name {`
/// to its closing brace).
fn shipped(code: &str) -> String {
    let mut shipped = Vec::new();
    let mut lines = code.lines().peekable();
    while let Some(line) = lines.next() {
        let attribute = line.trim();
        let test_only = attribute == "#[cfg(test)]" || attribute.starts_with("#[cfg(all(test,");
        if test_only
            && lines
                .peek()
                .is_some_and(|next| next.trim_start().starts_with("mod ") && next.contains('{'))
        {
            let mut depth = 0i64;
            for body in lines.by_ref() {
                depth += body.matches('{').count() as i64 - body.matches('}').count() as i64;
                if depth <= 0 {
                    break;
                }
            }
            continue;
        }
        shipped.push(line);
    }
    shipped.join("\n")
}

#[test]
fn the_native_modules_are_compiled_only_with_the_native_feature() {
    let lib = runtime_sources()
        .into_iter()
        .find(|(name, _)| name == "lib.rs")
        .unwrap()
        .1;
    for declaration in ["mod native;", "pub mod recordings;"] {
        let at = lib.find(declaration).unwrap();
        assert!(
            lib[..at]
                .trim_end()
                .ends_with("#[cfg(feature = \"native\")]"),
            "{declaration} must be gated by the native feature"
        );
    }
}

#[test]
fn only_native_modules_name_the_native_adapter_or_start_threads() {
    const THREADS: &[&str] = &[
        "thread::spawn",
        "thread::Builder",
        "tokio::runtime",
        "new_multi_thread",
        "new_current_thread",
        "Handle::current",
        // The clocks: std's panic in a browser build (`time.rs` holds them).
        "std::time::Instant",
        "SystemTime",
        "UNIX_EPOCH",
        "tokio::time",
    ];
    let sources = runtime_sources();
    let tests = test_modules(&sources);
    for (name, code) in &sources {
        // Unit-test files carry their test fixtures (threads included).
        if native_only(name)
            || tests.contains(name)
            || name.ends_with("tests.rs")
            || name == "time.rs"
        {
            continue;
        }
        let code = shipped(code);
        assert!(
            !identifiers(&code, "iptv_").contains("iptv_net"),
            "{name} names iptv_net outside the native modules; use the network port"
        );
        for thread in THREADS {
            assert!(
                !code.contains(thread),
                "{name} uses {thread} outside the native modules; use the execution port or crate::time"
            );
        }
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
