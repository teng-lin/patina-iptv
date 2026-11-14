//! Application boundary ratchets. Each retained shell static has a specific
//! platform, cache or UI purpose. This list may shrink; domain state belongs
//! to the owned Runtime, including the workers that service its commands.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const SHELL_STATICS: &[(&str, &str)] = &[
    ("android_transport.rs", "RELAYS"), // JNI playback relay handles.
    ("backup.rs", "REQUEST"),           // Android document picker signal.
    ("backup.rs", "ANSWER"),            // Android document picker callback.
    ("content_image.rs", "CACHE"),      // Artwork cache.
    ("content_image.rs", "PERMITS"),    // Artwork concurrency limit.
    ("content_image.rs", "CLIENT"),     // Artwork transport cache.
    ("credential_key.rs", "TEST_KEY"),  // Synthetic test key injection.
    ("credential_key/android.rs", "KEY"), // Android keystore handle.
    ("custom_groups.rs", "GENERATION"), // Discard stale dialog replies.
    ("data.rs", "APP_OWNER"),           // UI root owner for detached local tasks.
    ("desktop.rs", "WINDOW"),           // Native window handle.
    ("desktop.rs", "POINTER"),          // Pointer position within the window.
    ("desktop.rs", "LAST_MOVE"),        // Pointer gesture clock.
    ("fonts.rs", "FITTED"),             // Font measurement cache.
    ("fonts.rs", "FONTS_IN"),           // Font registration flag.
    ("guide_assign_view.rs", "REQUEST"), // Discard stale dialog replies.
    ("header.rs", "ON"),                // Debug overlay environment flag cache.
    ("i18n.rs", "SYSTEM"),              // System locale cache.
    ("i18n.rs", "WORDING_SETTINGS"),    // cfg(test)-only scoped wording preferences.
    ("lib.rs", "RUNTIME"),              // The shell's single application handle.
    ("lib.rs", "ROOT_STARTED"), // Guard: the handle is asked for on the application thread only.
    ("lists.rs", "HINT_SHOWN"), // Visible navigation hint revision.
    ("organization_view.rs", "OPEN_GENERATION"), // Discard stale dialog replies.
    ("platform_data.rs", "ANDROID_FILES"), // Android files directory handle.
    ("platform_data.rs", "HOST_STOP"), // The root owner's weak handle for off-thread host callbacks.
    ("seek.rs", "PRESSES"),            // Key gesture state.
    ("seek.rs", "HOLD"),               // Held-key gesture state.
    ("settings.rs", "REVISION"),       // Reactive settings subscription handle.
    ("settings_view.rs", "PAGES"),     // Immutable screen definitions.
    ("settings_view.rs", "LAST"),      // Last selected settings tab.
    ("settings_view.rs", "NOTICE"),    // Toast signal handle.
    ("settings_view.rs", "LEAVE_ASKED"), // Exit confirmation clock.
    ("startup.rs", "MARKED"),          // Startup phase instrumentation.
    ("startup.rs", "FALLBACK"),        // Startup clock instrumentation.
    ("time.rs", "SHOWN"),              // Visible guide clock origin.
    ("video.rs", "OPENS"),             // Native player open instrumentation.
    ("video.rs", "SOURCED"),           // Native player source projection.
];

fn sources(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

fn code(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Match declarations, including macro bodies and function-local statics;
/// lifetimes such as `&'static str` cannot be declarations.
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

fn app_sources() -> (PathBuf, Vec<PathBuf>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    (root, files)
}

#[test]
fn only_runtime_can_use_the_store_adapter() {
    let (_, files) = app_sources();
    for file in files {
        let source = code(&file);
        for forbidden in ["iptv_store_sqlite", "store_service::"] {
            assert!(
                !source.contains(forbidden),
                "{} uses {forbidden}",
                file.display()
            );
        }
    }
}

#[test]
fn shell_statics_are_only_the_documented_platform_cache_and_ui_handles() {
    let (root, files) = app_sources();
    let mut retained = BTreeSet::new();
    for file in files {
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for item in statics(&code(&file)) {
            assert!(
                SHELL_STATICS.contains(&(name.as_str(), item)),
                "{name}: {item} must belong to Runtime or an explicitly documented UI/platform handle"
            );
            assert!(
                retained.insert((name.clone(), item.to_string())),
                "{name}: duplicate static {item}"
            );
        }
    }
    for &(file, name) in SHELL_STATICS {
        assert!(
            retained.contains(&(file.to_string(), name.to_string())),
            "{file}: {name} was removed; shrink SHELL_STATICS too"
        );
    }
}

#[test]
fn declarations_include_macro_and_local_statics_but_not_lifetimes() {
    assert_eq!(
        statics("thread_local! { static STATE: Cell<u32> = x; }"),
        vec!["STATE"]
    );
    assert_eq!(
        statics("fn f() { static mut local: u32 = 0; }"),
        vec!["local"]
    );
    assert_eq!(statics("pub static\n GLOBAL: usize = 0;"), vec!["GLOBAL"]);
    assert!(statics("fn f() -> &'static str { x } struct A(&'static [u8]);").is_empty());
}
