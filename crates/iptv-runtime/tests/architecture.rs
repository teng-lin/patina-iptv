use std::path::Path;

fn check_sources(directory: &Path) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            check_sources(&path);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let code = std::fs::read_to_string(&path).unwrap();
            for forbidden in ["nsh_", "iptv_app", "iptv_player_mpv", "crate::Ui"] {
                assert!(
                    !code.contains(forbidden),
                    "{} uses {forbidden}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn runtime_has_no_shell_or_toolkit_dependencies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    check_dependency_allowlist(&manifest);
    check_sources(&root.join("src"));
}

// Explicit capabilities: adding a toolkit or platform service must be a
// conscious boundary change, rather than escaping a list of known bad names.
const DEPENDENCIES: &[&str] = &[
    "iptv-core",
    "iptv-library",
    "iptv-guide",
    "iptv-epg",
    "iptv-providers",
    "iptv-import",
    "iptv-net",
    "iptv-store-sqlite",
    "iptv-search",
    "iptv-settings",
    "getrandom",
    "futures-channel",
    "tokio",
    "log",
    "serde_json",
    "libc",
];

fn check_dependency_allowlist(manifest: &str) {
    let mut dependencies = false;
    let mut development = false;
    for line in manifest.lines() {
        let line = line.split('#').next().unwrap().trim();
        if line.starts_with('[') {
            assert!(
                !line.contains("build-dependencies"),
                "runtime has no build dependencies"
            );
            development = line == "[dev-dependencies]" || line.ends_with(".dev-dependencies]");
            dependencies =
                development || line == "[dependencies]" || line.ends_with(".dependencies]");
            // Dependency subtables could hide package aliases; keep the
            // dependency declarations explicit inline entries in this crate.
            assert!(
                !line.contains("dependencies."),
                "use inline dependency entries: {line}"
            );
        } else if dependencies && !line.is_empty() {
            let (key, value) = line.split_once('=').expect("inline dependency declaration");
            let name = key.trim().strip_suffix(".workspace").unwrap_or(key.trim());
            assert!(
                DEPENDENCIES.contains(&name)
                    || (development && ["rusqlite", "tempfile"].contains(&name)),
                "runtime cannot depend on {name}"
            );
            assert!(
                !value.contains("package"),
                "dependency aliases require boundary review: {line}"
            );
        }
    }
}

#[test]
fn dependency_gate_rejects_an_unlisted_toolkit_and_a_disguised_alias() {
    for declaration in [
        "[dependencies]\nnsh-core.workspace = true",
        "[dev-dependencies]\nnsh-core.workspace = true",
        "[target.'cfg(windows)'.dependencies]\nwindows = \"0.62\"",
        "[dependencies]\nlog = { package = \"nsh-core\", version = \"0.4\" }",
    ] {
        assert!(std::panic::catch_unwind(|| check_dependency_allowlist(declaration)).is_err());
    }
    check_dependency_allowlist("[dependencies]\nlog.workspace = true");
}
