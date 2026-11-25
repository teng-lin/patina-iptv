//! Application boundary ratchets. Each retained shell static has a specific
//! platform, cache or UI purpose. This list may shrink; domain state belongs
//! to the owned Runtime, including the workers that service its commands.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const SHELL_STATICS: &[(&str, &str)] = &[
    ("host/native/android_transport.rs", "RELAYS"), // JNI playback relay handles.
    ("backup.rs", "REQUEST"),                       // Android document picker signal.
    ("backup.rs", "ANSWER"),                        // Android document picker callback.
    ("capture_limits.rs", "REPORTED"),              // The host's report of what can stop a capture.
    ("host/native/keep_awake.rs", "HELD"),          // The OS idle-sleep hold while a capture runs.
    ("host/native/keep_awake.rs", "FAILED"),        // That hold failed, for the start notice.
    ("backup.rs", "PROBE"),                         // Android document picker probe request.
    ("backup.rs", "PICKERS"),                       // The document pickers the host reported.
    ("backup.rs", "PICKERS_FOR_TEST"),              // Synthetic picker injection (headless tests).
    ("content_image.rs", "CACHE"),                  // Artwork cache.
    ("content_image.rs", "PERMITS"),                // Artwork concurrency limit.
    ("content_image.rs", "CLIENT"),                 // Artwork transport cache.
    ("host/native/credential_key.rs", "TEST_KEY"),  // Synthetic test key injection.
    ("host/native/external_player.rs", "HOST"),     // Android chooser JNI class handle.
    ("host/native/external_player.rs", "PLAYBACK"), // External playback outlives the foreground activity.
    ("host/native/external_player.rs", "TEST_HANDLER"), // test-support-only handler probe override.
    ("host/native/credential_key/android.rs", "KEY"), // Android keystore handle.
    ("custom_groups.rs", "GENERATION"),             // Discard stale dialog replies.
    ("data.rs", "APP_OWNER"),                       // UI root owner for detached local tasks.
    ("desktop.rs", "WINDOW"),                       // Native window handle.
    ("desktop.rs", "POINTER"),                      // Pointer position within the window.
    ("desktop.rs", "LAST_MOVE"),                    // Pointer gesture clock.
    ("desktop.rs", "TRAVELED"),                     // Last pointer place, for Keys to Pointer.
    ("desktop.rs", "QUITTING"),                     // Stop and quit in progress (no second ask).
    ("device.rs", "REPORTED"),                      // Reactive handle of the host's device report.
    ("device.rs", "ACCESSIBILITY"), // Reactive handle of the host's accessibility report.
    ("fonts.rs", "FITTED"),         // Font measurement cache.
    ("fonts.rs", "FONTS_IN"),       // Font registration flag.
    ("guide_assign_view.rs", "REQUEST"), // Discard stale dialog replies.
    ("header.rs", "ON"),            // Debug overlay environment flag cache.
    ("i18n.rs", "SYSTEM"),          // System locale cache.
    ("i18n.rs", "WORDING_SETTINGS"), // cfg(test)-only scoped wording preferences.
    ("i18n/pseudo.rs", "ENABLED"),  // test-support-only pseudolocale environment flag.
    ("i18n/pseudo.rs", "FIXED"),    // test-support-only immutable expanded catalog.
    ("input.rs", "REMOTE_PHASES"),  // test-support-only remote key phase switch.
    ("input.rs", "REPEATS"),        // test-support-only keyboard repeat mirror.
    ("runtime_owner.rs", "RUNTIME"), // The shell's single application handle.
    ("runtime_owner.rs", "ROOT_STARTED"), // Guard: the handle is asked for on the application thread only.
    ("lib.rs", "TUI_MODE"), // Host flag: TUI backend bypasses StickyCells trackball wrapper.
    ("lib.rs", "LEAVE"),    // Host hook: Back at the top level asks first, and Yes leaves (Tizen).
    ("lists.rs", "HINT_SHOWN"), // Visible navigation hint revision.
    ("host/native/platform_data.rs", "ANDROID_FILES"), // Android files directory handle.
    ("host/native/platform_data.rs", "TEST_DIRECTORY"), // Test-only explicit isolated storage handle.
    ("host/native/platform_data.rs", "HOST_STOP"), // The root owner's weak handle for off-thread host callbacks.
    ("host/web.rs", "FILES"), // Restored browser files are shared across app owners.
    ("host/web.rs", "HOST_STOP"), // The root owner's weak handle, as natively.
    ("host/native/recording_host/android.rs", "HOST"), // JNI VM/class handle for the recording host.
    ("seek.rs", "PRESSES"),                            // Key gesture state.
    ("seek.rs", "HOLD"),                               // Held-key gesture state.
    ("settings.rs", "REVISION"),                       // Reactive settings subscription handle.
    ("pip.rs", "PIP_SUPPORTED"), // The host's picture-in-picture report, for the settings offer.
    ("settings_view.rs", "PAGES"), // Immutable screen definitions.
    ("settings_view.rs", "LAST"), // Last selected settings tab.
    ("settings_view.rs", "NOTICE"), // Toast signal handle.
    ("host/native/simulator_entitlements.rs", "ENTITLEMENTS"), // Immutable iOS simulator plist section.
    ("startup.rs", "MARKED"),                                  // Startup phase instrumentation.
    ("startup.rs", "FALLBACK"),                                // Startup clock instrumentation.
    ("clock.rs", "SHOWN"),                                     // Visible guide clock origin.
    ("video.rs", "OPENS"),   // Native player open instrumentation.
    ("video.rs", "SOURCED"), // Native player source projection.
];

/// Platform `cfg` outside `src/host/`, the host glue: what is still to
/// move there. It may only shrink; a screen that needs a platform fact asks
/// `crate::host`.
const PLATFORM_CFG_OUTSIDE_HOST: &[&str] = &[
    "backup.rs",                 // Android's document picker beside the desktop file dialog.
    "desktop/player_windows.rs", // macOS window ordering of the desktop player windows.
    "lib.rs",                    // The desktop-only guide walkthrough of the headless tests.
    "main.rs",                   // The desktop binaries' entry points.
    "video.rs",                  // The emulator's mock-server alias; desktop popup routing.
];

/// Whether `code` names a platform in a `cfg`.
fn platform_cfg(code: &str) -> bool {
    ["cfg(", "cfg!(", "cfg_attr("].iter().any(|start| {
        code.match_indices(start).any(|(at, _)| {
            // The condition, to its closing parenthesis (it may span lines).
            let rest = &code[at + start.len()..];
            let mut depth = 1usize;
            let end = rest
                .char_indices()
                .find(|&(_, ch)| {
                    match ch {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    depth == 0
                })
                .map_or(rest.len(), |(end, _)| end);
            let condition = &rest[..end];
            ["target_", "unix", "windows"]
                .iter()
                .any(|platform| condition.contains(platform))
        })
    })
}

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
        // Foreign framework constants declare symbols owned by the OS;
        // only definitions with an initializer own state in this shell.
        let mut brackets = 0usize;
        let terminal = rest
            .char_indices()
            .find_map(|(at, ch)| {
                match ch {
                    '[' => brackets += 1,
                    ']' => brackets = brackets.saturating_sub(1),
                    ';' if brackets == 0 => return Some(at),
                    _ => {}
                }
                None
            })
            .unwrap_or(rest.len());
        let declaration = &rest[..terminal];
        if end != 0 && rest[end..].trim_start().starts_with(':') && declaration.contains('=') {
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

/// The one shell file that names the store adapter: the browser host keeps
/// the store on its memory files (`Ports::store_host`), opening, persisting
/// and publishing it; every query still goes through the runtime.
const STORE_HOST: &str = "host/web/store.rs";

#[test]
fn only_runtime_can_use_the_store_adapter() {
    let (root, files) = app_sources();
    for file in files {
        let source = code(&file);
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for forbidden in ["iptv_store_sqlite", "store_service::"] {
            if forbidden == "iptv_store_sqlite" && name == STORE_HOST {
                continue;
            }
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
fn platform_cfg_lives_in_the_host_modules() {
    let (root, files) = app_sources();
    let mut found = BTreeSet::new();
    for file in files {
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if name == "host.rs" || name.starts_with("host/") || !platform_cfg(&code(&file)) {
            continue;
        }
        assert!(
            PLATFORM_CFG_OUTSIDE_HOST.contains(&name.as_str()),
            "{name} has a platform cfg; move it to src/host and call the host's facade"
        );
        found.insert(name);
    }
    for name in PLATFORM_CFG_OUTSIDE_HOST {
        assert!(
            found.contains(*name),
            "{name} has no platform cfg any more; shrink PLATFORM_CFG_OUTSIDE_HOST"
        );
    }
    assert!(platform_cfg("#[cfg(any(unix, test))]"));
    assert!(platform_cfg("let x = cfg!(target_os = \"ios\");"));
    assert!(platform_cfg(
        "#[cfg(all(\n    feature = \"a\",\n    windows\n))]"
    ));
    assert!(!platform_cfg(
        "#[cfg(feature = \"test-support\")] fn windows() {}"
    ));
}

#[test]
fn only_native_hosts_read_the_standard_clocks() {
    // std's clocks panic in a browser build: screens use NSH's Instant and
    // the host's wall clock (`crate::host::since_epoch`).
    let (root, files) = app_sources();
    for file in files {
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if name == "host/native.rs" || name.starts_with("host/native/") {
            continue;
        }
        let code = code(&file);
        for clock in ["std::time::Instant", "SystemTime", "UNIX_EPOCH"] {
            assert!(
                !code.contains(clock),
                "{name} reads {clock}; use nsh_core::time::Instant or crate::host::since_epoch"
            );
        }
        for (at, _) in code.match_indices("std::time::{") {
            let group = &code[at..at + code[at..].find('}').unwrap_or(0)];
            assert!(!group.contains("Instant"), "{name} imports std's Instant");
        }
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
    assert_eq!(
        statics("static KEY: OnceLock<[u8; 32]> = OnceLock::new();"),
        vec!["KEY"]
    );
    assert!(statics("unsafe extern \"C\" { static HOST_NAME: &str; }").is_empty());
    assert!(statics("fn f() -> &'static str { x } struct A(&'static [u8]);").is_empty());
}
