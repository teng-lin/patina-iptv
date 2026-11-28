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
    "semver", // Native GitHub release discovery compares stable version precedence.
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

/// Shipped runtime modules that may name the store adapter. Test fixtures
/// are tracked separately below.
const STORE_ADAPTER_USERS: &[&str] = &[
    "backup.rs",
    "backup/error.rs",
    "credential_store.rs",
    "error.rs",
    "guide_repair.rs",
    "guide_repair/prepare.rs",
    "imports.rs",
    "inspection.rs",
    "inspection/probe.rs",
    "model.rs",
    "opening.rs",
    "private_files.rs",
    "publish.rs",
    "recordings.rs",
    "recordings/shared.rs",
    "state.rs",
    "store_host.rs",
    "store_service/",
    "stored.rs",
    "vod.rs",
];

/// Test-only adapter references do not widen the shipped runtime surface.
const STORE_ADAPTER_TEST_USERS: &[&str] = &[
    "backup/tests.rs",
    "favorites.rs",
    "guide_checks/retry.rs",
    "guide_repair/tests.rs",
    "opening/revocation.rs",
    "parental.rs",
    "publish/tests/catalog_import_tests.rs",
    "recordings/shared_tests.rs",
    "recordings/tests.rs",
    "vod/recovery_tests.rs",
];

/// Ratchet the owner shape as PR2 extracts domain state. Includes fields
/// behind `test-support`, matching the source inventory rather than one build.
const RUNTIME_FIELD_LIMIT: usize = 14;

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
    // Only a scoped Tokio worker-spawn unwind leaves the TUI terminal in place.
    ("native/execution.rs", "STARTING_TOKIO_WORKER"),
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
    assert!(
        STORE_ADAPTER_USERS.len() <= 20,
        "the shipped store adapter boundary exceeded 20 users"
    );
    let sources = runtime_sources();
    let test_modules = test_modules(&sources);
    let users: BTreeSet<String> = sources
        .iter()
        .filter(|(name, code)| {
            !test_modules.contains(name.as_str()) && shipped(code).contains("iptv_store_sqlite")
        })
        .map(|(name, _)| {
            if name.starts_with("store_service/") {
                "store_service/".to_owned()
            } else {
                name.clone()
            }
        })
        .collect();
    exact(&users, STORE_ADAPTER_USERS, "shipped store adapter user");
    for (name, code) in &sources {
        if adapter_reexport(&shipped(code)) {
            assert!(
                matches!(name.as_str(), "model.rs" | "error.rs"),
                "{name} re-exports the SQLite adapter"
            );
        }
    }
    let test_users: BTreeSet<String> = sources
        .iter()
        .filter(|(name, code)| {
            code.contains("iptv_store_sqlite")
                && (test_modules.contains(name) || !shipped(code).contains("iptv_store_sqlite"))
        })
        .map(|(name, _)| name.clone())
        .collect();
    exact(
        &test_users,
        STORE_ADAPTER_TEST_USERS,
        "test-only store adapter user",
    );
}

fn adapter_reexport(code: &str) -> bool {
    let compact: String = code.chars().filter(|ch| !ch.is_whitespace()).collect();
    ["pub", "pub(crate)", "pub(super)", "pub(self)"]
        .into_iter()
        .any(|visibility| {
            [
                "iptv_store_sqlite",
                "::iptv_store_sqlite",
                "{iptv_store_sqlite",
            ]
            .into_iter()
            .any(|adapter| compact.contains(&format!("{visibility}use{adapter}")))
        })
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
        // The panic hook receives no owner. Allow only its scoped
        // boolean, so other thread-local declarations still fail.
        let panic_scope = name == "native/execution.rs"
            && code.contains(
                "thread_local! {\n    static STARTING_TOKIO_WORKER: Cell<bool> = const { Cell::new(false) };\n}",
            );
        assert_eq!(
            code.matches("thread_local!").count(),
            usize::from(panic_scope),
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
            other if other.ends_with("/mod.rs") => other.trim_end_matches("mod.rs").into(),
            other => format!("{}/", other.trim_end_matches(".rs")),
        };
        let mut lines = code.lines().map(str::trim).peekable();
        while let Some(line) = lines.next() {
            if line != "#[cfg(test)]" && !line.starts_with("#[cfg(all(test,") {
                continue;
            }
            let Some(module) = lines.peek().and_then(|next| next.strip_prefix("mod ")) else {
                continue;
            };
            if let Some(module) = module.strip_suffix(';') {
                found.insert(format!("{parent}{module}.rs"));
            } else if let Some(module) = module.strip_suffix(" {") {
                // An external module declared inside a test-only inline module
                // inherits the enclosing module's cfg.
                let mut depth = 0i64;
                for body in lines.by_ref() {
                    if depth == 1
                        && let Some(child) = body
                            .strip_prefix("mod ")
                            .and_then(|rest| rest.strip_suffix(';'))
                    {
                        found.insert(format!("{parent}{module}/{child}.rs"));
                    }
                    depth += body.matches('{').count() as i64 - body.matches('}').count() as i64;
                    if depth <= 0 {
                        break;
                    }
                }
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
        if test_only
            && lines.peek().is_some_and(|next| {
                let next = next.trim_start();
                next.starts_with("use ") || next.starts_with("pub use ")
            })
        {
            lines.next();
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

/// Shipped `store_service/**` modules may name only infrastructure modules,
/// dependency-free data modules, and the leaf `parental::gate` submodule.
/// Exact, shrink-only list.
const STORE_SERVICE_MAY_NAME: &[&str] = &[
    // Crate root re-exports of infrastructure ports and test gates.
    "MaybeSend",
    "Ports",
    "TestGate",
    // Infrastructure modules.
    "credential_store",
    "error",
    "execution",
    "file_writer",
    "guide_checks",
    "network",
    "settings",
    "testing",
    "time",
    "workers",
    // Dependency-free data and leaf gate modules.
    "model",
    "paging",
    "parental::gate",
    "ports",
    "state",
];

fn expand_group(group: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut item = String::new();
    for ch in group.chars() {
        match ch {
            '{' => depth += 1,
            '}' if depth == 0 => break,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                items.push(std::mem::take(&mut item));
                continue;
            }
            _ => {}
        }
        item.push(ch);
    }
    items.push(item);
    items
}

fn first_segment(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    let end = text
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .unwrap_or(text.len());
    (&text[..end], &text[end..])
}

fn module_path(file: &str) -> Vec<String> {
    if file == "lib.rs" {
        return Vec::new();
    }
    let trimmed = file
        .strip_suffix("/mod.rs")
        .or_else(|| file.strip_suffix(".rs"))
        .unwrap_or(file);
    trimmed.split('/').map(str::to_owned).collect()
}

fn inline_mod_name(line: &str) -> Option<&str> {
    let mut rest = line.trim_start();
    if let Some(after) = rest.strip_prefix("pub") {
        let after = after.trim_start();
        if let Some(in_parens) = after.strip_prefix('(') {
            let (_, after_parens) = in_parens.split_once(')')?;
            rest = after_parens.trim_start();
        } else if after.starts_with("mod ") {
            rest = after;
        } else {
            return None;
        }
    }
    let after = rest.strip_prefix("mod ")?;
    let (name, tail) = first_segment(after);
    if !name.is_empty() && tail.trim_start().starts_with('{') {
        Some(name)
    } else {
        None
    }
}

fn push_resolved_segments(prefix: &[String], tail: &str, found: &mut BTreeSet<String>) {
    let mut base: Vec<String> = prefix.to_vec();
    let mut rest = tail.trim_start();
    while let Some(after) = rest.strip_prefix("super::") {
        base.pop();
        rest = after.trim_start();
    }
    let items: Vec<String> = if let Some(group) = rest.strip_prefix('{') {
        expand_group(group)
    } else {
        vec![rest.to_owned()]
    };
    for item in items {
        let (first, after_first) = first_segment(&item);
        if first.is_empty() {
            continue;
        }
        let mut segments = base.clone();
        segments.push(first.to_owned());
        if segments.len() == 1
            && matches!(segments[0].as_str(), "parental" | "cast" | "native")
            && let Some(after) = after_first.trim_start().strip_prefix("::")
        {
            let after = after.trim_start();
            if let Some(group) = after.strip_prefix('{') {
                for sub in expand_group(group) {
                    let (second, _) = first_segment(&sub);
                    if !second.is_empty() {
                        found.insert(format!("{}::{second}", segments[0]));
                    }
                }
            } else {
                let (second, _) = first_segment(after);
                if !second.is_empty() {
                    found.insert(format!("{}::{second}", segments[0]));
                } else {
                    found.insert(segments[0].clone());
                }
            }
            continue;
        }
        if matches!(segments[0].as_str(), "parental" | "cast" | "native") && segments.len() >= 2 {
            found.insert(format!("{}::{}", segments[0], segments[1]));
        } else {
            found.insert(segments[0].clone());
        }
    }
}

/// Crate-rooted module targets named in `code` via `crate::` or `super::`
/// (resolved against `file`'s module path and any enclosing inline `mod { }`
/// blocks). Expands `{a, b::c}` groups and matches `parental::*`, `cast::*`,
/// and `native::*` at two segments.
fn crate_named(code: &str, file: &str) -> BTreeSet<String> {
    let word = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    let mut current = module_path(file);
    let mut mod_depths: Vec<i32> = Vec::new();
    let mut depth = 0_i32;
    let mut found = BTreeSet::new();
    let mut offset = 0usize;
    for raw_line in code.split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        if let Some(name) = inline_mod_name(line) {
            current.push(name.to_owned());
            mod_depths.push(depth);
        }
        for prefix in ["crate::", "super::"] {
            for (at, _) in line.match_indices(prefix) {
                let before = &line[..at];
                if before.chars().next_back().is_some_and(word) || before.ends_with("::") {
                    continue;
                }
                let rest = &code[offset + at + prefix.len()..];
                if prefix == "crate::" {
                    push_resolved_segments(&[], rest, &mut found);
                } else {
                    let parent = if current.is_empty() {
                        &[][..]
                    } else {
                        &current[..current.len() - 1]
                    };
                    push_resolved_segments(parent, rest, &mut found);
                }
            }
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        while mod_depths.last().is_some_and(|&d| depth <= d) {
            mod_depths.pop();
            current.pop();
        }
        offset += raw_line.len();
    }
    found
}

#[test]
fn store_service_names_only_infrastructure() {
    let sources = runtime_sources();
    let tests = test_modules(&sources);
    let mut found = BTreeSet::new();
    for (name, code) in &sources {
        if !name.starts_with("store_service/") || tests.contains(name) {
            continue;
        }
        let code = shipped(code);
        for item in crate_named(&code, name) {
            if item == "store_service" {
                continue;
            }
            assert!(
                STORE_SERVICE_MAY_NAME.contains(&item.as_str()),
                "{name} names crate::{item}; store_service may name only listed infrastructure \
                 and leaf data modules"
            );
            found.insert(item);
        }
    }
    exact(&found, STORE_SERVICE_MAY_NAME, "store_service dependency");
}

/// Bottom-layer modules: they may name each other and external crates, never
/// an orchestration module. Exact, shrink-only list.
const FOUNDATION: &[&str] = &[
    "error",
    "model",
    "state",
    "paging",
    "time",
    "network",
    "execution",
    "workers",
    "ports",
    "events",
    "file_writer",
    "storage",
    "build_config",
];

/// Non-FOUNDATION items that foundation modules and leaf submodules may name:
/// crate-root re-exports, leaf host/threading/test traits, native adapter
/// bridges behind `cfg(feature = "native")`, and leaf submodules. Exact list.
const FOUNDATION_EXTRA_MAY_NAME: &[&str] = &[
    "Error",
    "MaybeSend",
    "TestGate",
    "cast::pairing",
    "cast::port",
    "file_host",
    "native::execution",
    "native::network",
    "parental::gate",
    "store_host",
    "testing",
    "threading",
];

fn foundation_member(name: &str) -> bool {
    if matches!(name, "parental/gate.rs" | "cast/port.rs" | "cast/pairing.rs") {
        return true;
    }
    FOUNDATION.iter().any(|module| {
        name == format!("{module}.rs") || name.starts_with(&format!("{module}/"))
    })
}

#[test]
fn foundation_modules_depend_only_on_foundation() {
    let sources = runtime_sources();
    let tests = test_modules(&sources);
    let mut extra_used = BTreeSet::new();
    for (name, code) in &sources {
        if !foundation_member(name) || tests.contains(name) {
            continue;
        }
        let code = shipped(code);
        for item in crate_named(&code, name) {
            if FOUNDATION.contains(&item.as_str()) {
                continue;
            }
            assert!(
                FOUNDATION_EXTRA_MAY_NAME.contains(&item.as_str()),
                "{name} names crate::{item}; foundation modules may name only FOUNDATION \
                 modules or listed leaf dependencies"
            );
            extra_used.insert(item);
        }
    }
    exact(
        &extra_used,
        FOUNDATION_EXTRA_MAY_NAME,
        "foundation extra dependency",
    );
}

#[test]
fn crate_named_resolves_super_inline_mods_and_two_segment_paths() {
    assert_eq!(
        crate_named(
            "use super::{Services, StoreError};\nuse super::super::imports::Imported;\n\
             use crate::parental::{gate::Snapshot, State};\nuse crate::native::app_updates;",
            "store_service/epg_repo.rs"
        ),
        [
            "imports",
            "native::app_updates",
            "parental::State",
            "parental::gate",
            "store_service",
        ]
        .map(str::to_owned)
        .into()
    );
    assert_eq!(
        crate_named(
            "mod host {\n    fn clock() -> super::Clock {}\n}\nuse super::Error;",
            "time.rs"
        ),
        ["Error", "time"].map(str::to_owned).into()
    );
    assert_eq!(
        crate_named(
            "use super::{\n    pairing::PairingPrompt,\n};",
            "cast/port.rs"
        ),
        ["cast::pairing"].map(str::to_owned).into()
    );
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
