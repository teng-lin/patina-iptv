//! Application boundary ratchets. Each retained shell static has a specific
//! platform, cache or UI purpose. This list may shrink; domain state belongs
//! to the owned Runtime, including the workers that service its commands.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const UI_TOP_LEVEL_FIELDS: usize = 7;

#[test]
fn ui_only_holds_domain_handles() {
    let (root, _) = app_sources();
    let source = code(&root.join("ui.rs"));
    let (_, definition) = source.split_once("pub struct Ui {").expect("Ui definition");
    let (fields, _) = definition.split_once("\n}").expect("end of Ui definition");
    let fields: BTreeSet<_> = fields
        .lines()
        .filter_map(|line| {
            let (field, kind) = line.split_once(':')?;
            Some((
                field.split_whitespace().last()?,
                kind.trim().trim_end_matches(','),
            ))
        })
        .collect();
    assert_eq!(fields.len(), UI_TOP_LEVEL_FIELDS);
    assert_eq!(
        fields,
        BTreeSet::from([
            ("app", "SharedUi"),
            ("owner", "Owner"),
            ("guide", "GuideUi"),
            ("playback", "PlaybackUi"),
            ("cast", "casting::CastUi"),
            ("screens", "ScreensUi"),
            ("chrome", "ChromeUi"),
        ])
    );
}

const SHELL_STATICS: &[(&str, &str)] = &[
    // NSH native views use these IDs and weak references to attach to a runtime-owned sender.
    ("host/native/apple_airplay/transport.rs", "NEXT_ID"),
    ("host/native/apple_airplay/transport.rs", "VIEWS"),
    // The JNI class handle and non-owning callback IDs of Android's Cast foreground service.
    ("host/native/casting/android.rs", "HOST"),
    ("host/native/casting/android.rs", "LEASES"),
    ("host/native/casting/android.rs", "NEXT_LEASE"),
    ("host/native/android_transport.rs", "RELAYS"), // JNI playback relay handles.
    ("host/backup_documents.rs", "REQUEST"),        // Android document picker signal.
    ("host/backup_documents.rs", "ANSWER"),         // Android document picker callback.
    ("screens/capture_limits.rs", "REPORTED"),      // The host's report of what can stop a capture.
    ("host/native/keep_awake.rs", "HELD"),          // The OS idle-sleep hold while a capture runs.
    ("host/native/keep_awake.rs", "FAILED"),        // That hold failed, for the start notice.
    ("host/backup_documents.rs", "PROBE"),          // Android document picker probe request.
    ("host/backup_documents.rs", "CREATE_TEXT"), // Android non-stub text document creator probe signal.
    ("host/backup_documents.rs", "SHARE"),       // Android non-stub text share probe signal.
    ("library_ui/backup.rs", "PICKERS"),         // The document pickers the host reported.
    ("library_ui/backup.rs", "PICKERS_FOR_TEST"), // Synthetic picker injection (headless tests).
    ("kit/content_image.rs", "CACHE"),           // Artwork cache.
    ("kit/content_image.rs", "PERMITS"),         // Artwork concurrency limit.
    ("kit/content_image.rs", "CLIENT"),          // Artwork transport cache.
    ("host/native/credential_key.rs", "TEST_KEY"), // Synthetic test key injection.
    ("host/updates/android.rs", "HOST"),         // Direct APK installer JNI class handle.
    ("host/native/external_player.rs", "HOST"),  // Android chooser JNI class handle.
    ("host/native/external_player.rs", "PLAYBACK"), // External playback outlives the foreground activity.
    ("host/native/external_player.rs", "TEST_HANDLER"), // test-support-only handler probe override.
    ("host/native/credential_key/android.rs", "KEY"), // Android keystore handle.
    ("wiring/tasks.rs", "APP_OWNER"),               // UI root owner for detached local tasks.
    ("shell/desktop.rs", "WINDOW"),                 // Native window handle.
    ("shell/desktop.rs", "POINTER"),                // Pointer position within the window.
    ("shell/desktop.rs", "LAST_MOVE"),              // Pointer gesture clock.
    ("shell/desktop.rs", "TRAVELED"),               // Last pointer place, for Keys to Pointer.
    ("shell/desktop.rs", "QUITTING"),               // Stop and quit in progress (no second ask).
    ("wiring/device.rs", "REPORTED"),               // Reactive handle of the host's device report.
    ("wiring/device.rs", "ACCESSIBILITY"), // Reactive handle of the host's accessibility report.
    ("kit/fonts.rs", "FITTED"),            // Font measurement cache.
    ("kit/fonts.rs", "FONTS_IN"),          // Font registration flag.
    ("guide/header.rs", "ON"),             // Debug overlay environment flag cache.
    ("kit/i18n.rs", "SYSTEM"),             // System locale cache.
    ("kit/i18n.rs", "WORDING_SETTINGS"),   // cfg(test)-only scoped wording preferences.
    ("kit/i18n/pseudo.rs", "ENABLED"),     // test-support-only pseudolocale environment flag.
    ("kit/i18n/pseudo.rs", "FIXED"),       // test-support-only immutable expanded catalog.
    ("kit/input.rs", "REMOTE_PHASES"),     // test-support-only remote key phase switch.
    ("shell/input.rs", "REPEATS"),         // test-support-only keyboard repeat mirror.
    ("wiring/runtime_owner.rs", "RUNTIME"), // The shell's single application handle.
    ("wiring/runtime_owner.rs", "ROOT_STARTED"), // Guard: the handle is asked for on the application thread only.
    ("host.rs", "TUI_MODE"), // Host flag: TUI backend bypasses StickyCells trackball wrapper.
    ("shell/back.rs", "LEAVE"), // Host hook: Back at the top level asks first, and Yes leaves (Tizen).
    ("guide/lists.rs", "HINT_SHOWN"), // Visible navigation hint revision.
    ("host/native/platform_data.rs", "ANDROID_FILES"), // Android files directory handle.
    ("host/native/platform_data.rs", "TEST_DIRECTORY"), // Test-only explicit isolated storage handle.
    ("host/native/platform_data.rs", "HOST_STOP"), // The root owner's weak handle for off-thread host callbacks.
    ("host/web.rs", "FILES"), // Restored browser files are shared across app owners.
    ("host/web.rs", "HOST_STOP"), // The root owner's weak handle, as natively.
    ("host/native/recording_host/android.rs", "HOST"), // JNI VM/class handle for the recording host.
    ("playback/seek.rs", "PRESSES"),                   // Key gesture state.
    ("playback/seek.rs", "HOLD"),                      // Held-key gesture state.
    ("wiring/settings.rs", "REVISION"),                // Reactive settings subscription handle.
    ("playback/pip.rs", "PIP_SUPPORTED"), // The host's picture-in-picture report, for the settings offer.
    ("screens/settings_view.rs", "PAGES"), // Immutable screen definitions.
    ("screens/settings_view.rs", "LAST"), // Last selected settings tab.
    ("kit/toast.rs", "NOTICE"),           // Toast signal handle.
    ("host/native/simulator_entitlements.rs", "ENTITLEMENTS"), // Immutable iOS simulator plist section.
    ("kit/startup.rs", "MARKED"),                              // Startup phase instrumentation.
    ("kit/startup.rs", "FALLBACK"),                            // Startup clock instrumentation.
    ("guide/clock.rs", "SHOWN"),                               // Visible guide clock origin.
    ("playback/video.rs", "OPENS"), // Native player open instrumentation.
    ("playback/video.rs", "SOURCED"), // Native player source projection.
    // Synthetic receiver for headless Cast journeys.
    ("host/native.rs", "TEST_CAST"),
    ("host/native.rs", "TEST_NATIVE_CAST"), // Synthetic native sender port in isolated test processes.
    // Process-wide forwarding diagnostic logger, panic hook and rotating log handles.
    ("host/diagnostics.rs", "LOGGER"),
    ("host/native/diagnostics.rs", "ORIGIN"),
    ("host/native/diagnostics.rs", "PANIC_HOOK"),
    ("host/native/diagnostics.rs", "PANIC_DIR"),
    ("host/native/diagnostics.rs", "ACTIVATED"),
    ("host/native/diagnostics.rs", "TEMP_SEQ"),
    ("host/native/diagnostics.rs", "ENV_LOGGER"),
    ("host/native/diagnostics.rs", "IN_PANIC"),
    ("host/native/diagnostics.rs", "ANDROID_INIT"),
];

/// Platform `cfg` outside `src/host/`, the host glue: what is still to
/// move there. It may only shrink; a screen that needs a platform fact asks
/// `crate::host`.
const PLATFORM_CFG_OUTSIDE_HOST: &[&str] = &["main.rs"];

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
        let name = file_name(&root, &file);
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
        let name = file_name(&root, &file);
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
        let name = file_name(&root, &file);
        if host_file(&name) || !platform_cfg(&code(&file)) {
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
        let name = file_name(&root, &file);
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
const VIEW_TREES: [&str; 7] = [
    "guide",
    "kit",
    "library_ui",
    "playback",
    "screens",
    "shell",
    "wiring",
];
const TREE_SEAMS: [&str; 2] = ["screens", "library_ui"];

/// Existing lateral dependencies. Remove rows as callers move to shared
/// routes and view helpers.
const CROSS_SCREEN_IMPORTS: &[(&str, &str)] = &[
    ("library_ui/custom_groups.rs", "screens::parental_view"),
    ("library_ui/epg_sources_view.rs", "screens::parental_view"),
    ("library_ui/epg_sources_view.rs", "screens::settings_view"),
    ("library_ui/guide_repair_view.rs", "screens::parental_view"),
    ("library_ui/organization_view.rs", "screens::channels_view"),
    ("library_ui/organization_view.rs", "screens::parental_view"),
    ("library_ui/organization_view.rs", "screens::settings_view"),
    ("library_ui/playlists_view.rs", "screens::settings_view"),
    ("library_ui/wizard_view.rs", "screens::settings_view"),
    ("screens/settings_offer.rs", "library_ui::backup"),
    ("screens/settings_view.rs", "library_ui::backup"),
    (
        "screens/settings_view/view.rs",
        "library_ui::playlists_view",
    ),
    ("screens/vod_view/empty.rs", "library_ui::playlists_view"),
    ("screens/vod_view/empty.rs", "library_ui::wizard_view"),
];

fn host_file(name: &str) -> bool {
    name == "host.rs" || name.starts_with("host/")
}

fn file_name(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

/// Test harnesses can instantiate the application to exercise real host
/// adapters. Skip those modules when checking production dependencies.
fn shipped(source: &str) -> String {
    let mut lines = source.lines().peekable();
    let mut result = String::new();
    while let Some(line) = lines.next() {
        let attribute = line.trim();
        if attribute.starts_with("#[cfg(") {
            let mut cfg = attribute.to_owned();
            while !cfg.ends_with(']') {
                let Some(continuation) = lines.next() else {
                    break;
                };
                cfg.push_str(continuation.trim());
            }
            let test = cfg == "#[cfg(test)]" || cfg.starts_with("#[cfg(all(test,");
            if test
                && lines
                    .peek()
                    .is_some_and(|next| next.trim_start().starts_with("mod "))
            {
                let mut depth = 0isize;
                for line in lines.by_ref() {
                    depth +=
                        line.matches('{').count() as isize - line.matches('}').count() as isize;
                    if depth <= 0 {
                        break;
                    }
                }
                continue;
            }
            result.push_str(&cfg);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    result
}

fn is_word(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn tokens(mut source: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        source = source.trim_start();
        let first = source.chars().next()?;
        let length = if is_word(first) {
            source
                .find(|character| !is_word(character))
                .unwrap_or(source.len())
        } else if source.starts_with("::") {
            2
        } else {
            first.len_utf8()
        };
        let (token, rest) = source.split_at(length);
        source = rest;
        Some(token)
    })
}

/// Expand use trees as well as ordinary paths so aliases, groups and globs
/// cannot hide an import. Qualified calls have the same boundary as imports.
fn paths_from<'a>(source: &'a str, root: &str) -> BTreeSet<Vec<&'a str>> {
    let tokens: Vec<_> = tokens(source).collect();
    let mut paths = BTreeSet::new();
    for (at, pair) in tokens.windows(2).enumerate() {
        if pair == [root, "::"] {
            expand_path(&tokens, &mut (at + 2), &mut Vec::new(), &mut paths);
        }
    }
    paths
}

fn expand_path<'a>(
    tokens: &[&'a str],
    at: &mut usize,
    prefix: &mut Vec<&'a str>,
    paths: &mut BTreeSet<Vec<&'a str>>,
) {
    let Some(&component) = tokens.get(*at) else {
        return;
    };
    *at += 1;
    if component == "{" {
        while tokens.get(*at).is_some_and(|token| *token != "}") {
            expand_path(tokens, at, prefix, paths);
            if tokens.get(*at) == Some(&"as") {
                *at += 2;
            }
            if tokens.get(*at) != Some(&",") {
                break;
            }
            *at += 1;
        }
        if tokens.get(*at) == Some(&"}") {
            *at += 1;
        }
    } else if component == "*" || component.chars().next().is_some_and(is_word) {
        prefix.push(component);
        if tokens.get(*at) == Some(&"::") {
            *at += 1;
            expand_path(tokens, at, prefix, paths);
        } else {
            paths.insert(prefix.clone());
        }
        prefix.pop();
    }
}

fn is_module_in(root: &Path, tree: &str, name: &str) -> bool {
    if !name.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
        return false;
    }
    let module = root.join(tree).join(name);
    module.with_extension("rs").is_file() || module.join("mod.rs").is_file()
}

fn is_view_dependency(root: &Path, name: &str) -> bool {
    matches!(
        name,
        "*" | "self"
            | "App"
            | "Ui"
            | "SharedUi"
            | "GuideUi"
            | "PlaybackUi"
            | "ChromeUi"
            | "ScreensUi"
            | "ViewMode"
            | "Playback"
            | "debug_overlay"
            | "schedule"
    ) || VIEW_TREES
        .into_iter()
        .any(|tree| name == tree || is_module_in(root, tree, name))
}

/// Root private imports are visible to descendants. Check short crate paths
/// and relative paths that climb from the current file to the crate root.
fn rooted_paths<'a>(source: &'a str, file: &str) -> BTreeSet<Vec<&'a str>> {
    let mut absolute = paths_from(source, "crate");
    let depth = file
        .strip_suffix("/mod.rs")
        .unwrap_or(file)
        .split('/')
        .count();
    for path in paths_from(source, "super") {
        let parents = path.iter().take_while(|part| **part == "super").count() + 1;
        if parents >= depth && parents <= path.len() {
            absolute.insert(path[parents - 1..].to_vec());
        }
    }
    absolute
}

#[test]
fn host_has_no_view_or_wiring_dependency() {
    let (root, files) = app_sources();
    for file in files {
        let name = file_name(&root, &file);
        if !host_file(&name) || name.ends_with("/tests.rs") {
            continue;
        }
        let source = shipped(&code(&file));
        for path in rooted_paths(&source, &name) {
            assert!(
                path != ["runtime"] && !is_view_dependency(&root, path[0]),
                "{name} depends on application path {}",
                path.join("::")
            );
        }
    }
}

#[test]
fn native_adapters_and_package_update_operations_live_in_the_host() {
    let (root, files) = app_sources();
    for file in files {
        let name = file_name(&root, &file);
        if host_file(&name) || name == "main.rs" || name.ends_with("/tests.rs") {
            continue;
        }
        let source = shipped(&code(&file));
        for word in source.split(|character| !is_word(character)) {
            assert!(
                !word.starts_with("nsh_platform_")
                    && !matches!(
                        word,
                        "jni"
                            | "objc2"
                            | "block2"
                            | "dbus"
                            | "dbus_secret_service"
                            | "keyring"
                            | "iptv_cast"
                            | "iptv_net"
                            | "iptv_web"
                            | "iptv_store_sqlite"
                    ),
                "{name} names host adapter {word}"
            );
        }
        for scope in ["iptv_runtime", "runtime"] {
            assert!(
                !paths_from(&source, scope)
                    .iter()
                    .any(|path| path.first() == Some(&"app_updates")),
                "{name} must use the host facade for package update operations"
            );
        }
    }
}

#[test]
fn direct_screen_imports_follow_the_shrink_only_list() {
    let (root, files) = app_sources();
    let mut found = BTreeSet::new();
    for file in files {
        let name = file_name(&root, &file);
        let Some((tree, _)) = name.split_once('/') else {
            continue;
        };
        let Some(other) = TREE_SEAMS
            .into_iter()
            .find(|candidate| *candidate != tree && TREE_SEAMS.contains(&tree))
        else {
            continue;
        };
        let source = code(&file);
        for path in rooted_paths(&source, &name) {
            let first = path[0];
            let module = if first == other {
                path.get(1).copied().unwrap_or("*")
            } else if is_module_in(&root, other, first) {
                first
            } else {
                continue;
            };
            assert!(
                !matches!(module, "*" | "self"),
                "{name} imports all of {other}"
            );
            found.insert((name.clone(), format!("{other}::{module}")));
        }
    }
    let expected = CROSS_SCREEN_IMPORTS
        .iter()
        .map(|(file, module)| (file.to_string(), module.to_string()))
        .collect();
    assert_eq!(found, expected);
}

#[test]
fn grouped_and_aliased_paths_expose_their_dependencies() {
    let source = "
        use crate::{host::{self, Path}, screens::{settings_view as settings, *}, Ui};
        use super::super::wiring::{data::catalog, runtime::Runtime};
        let status = crate :: playback :: video :: has_source(0);
        #[cfg(test)]
        mod tests { use crate::App; }
    ";
    let source = shipped(source);
    assert_eq!(
        paths_from(&source, "crate"),
        BTreeSet::from([
            vec!["host", "self"],
            vec!["host", "Path"],
            vec!["screens", "settings_view"],
            vec!["screens", "*"],
            vec!["Ui"],
            vec!["playback", "video", "has_source"],
        ])
    );
    assert!(paths_from(&source, "super").contains(&vec!["super", "wiring", "data", "catalog"]));
}

/// Value types from `iptv_import` that UI code may name outside `src/host/`.
/// Operations (such as `stalker_link`) belong in `iptv-runtime`.
const IMPORT_VALUES_OUTSIDE_HOST: &[&str] = &["CancelToken"];

/// Known intentional exception: artwork downloads use `Network::client` directly.
const IMPORT_FETCH_EXCEPTIONS: &[&str] = &["kit/content_image.rs"];

#[test]
fn domain_import_operations_stay_outside_views() {
    let (root, files) = app_sources();
    for file in files {
        let name = file_name(&root, &file);
        if host_file(&name) || name.ends_with("/tests.rs") {
            continue;
        }
        let source = shipped(&code(&file));
        let token_list: Vec<_> = tokens(&source).collect();
        for (at, &token) in token_list.iter().enumerate() {
            if token == "iptv_import" && token_list.get(at + 1) != Some(&"::") {
                panic!(
                    "{name} aliases or imports bare `iptv_import`; move provider I/O operations into iptv-runtime"
                );
            }
        }
        for path in paths_from(&source, "iptv_import") {
            let item = path.first().copied().unwrap_or("");
            if IMPORT_VALUES_OUTSIDE_HOST.contains(&item) {
                continue;
            }
            if item == "Fetch" && IMPORT_FETCH_EXCEPTIONS.contains(&name.as_str()) {
                continue;
            }
            panic!(
                "{name} names iptv_import::{}; move provider I/O operations into iptv-runtime",
                path.join("::")
            );
        }
    }
}

#[test]
fn host_cannot_reach_views_through_root_imports() {
    let (root, _) = app_sources();
    for (file, source) in [
        ("host.rs", "use super::{App as Root};"),
        ("host/bridge.rs", "use super::super::i18n::tr;"),
        ("host/bridge.rs", "use crate::{settings_view as screen};"),
        ("host/bridge.rs", "let _ = crate::runtime();"),
    ] {
        assert!(
            rooted_paths(source, file)
                .iter()
                .any(|path| path == &["runtime"] || is_view_dependency(&root, path[0]))
        );
    }
    assert!(is_module_in(&root, "screens", "parental_view"));
}

/// Shrink-only count of shipped functions in `crates/iptv-app/src/**` taking
/// the whole `Ui` (`: Ui` or `: &Ui`). Lower this as more functions move to
/// narrower `Ui` sub-handles.
const UI_PARAMETER_LIMIT: usize = 870;

fn ui_parameter_functions(source: &str) -> usize {
    let tokens: Vec<_> = tokens(source).collect();
    let mut count = 0usize;
    let mut i = 0usize;
    while i < tokens.len() {
        if tokens[i] != "fn" {
            i += 1;
            continue;
        }
        // Require a function name identifier (excludes `fn(Ui)` pointer types).
        let Some(&name) = tokens.get(i + 1) else {
            break;
        };
        if !name.chars().next().is_some_and(is_word) {
            i += 1;
            continue;
        }
        i += 2;
        if tokens.get(i) == Some(&"<") {
            let mut angle = 0usize;
            while i < tokens.len() {
                match tokens[i] {
                    "<" => angle += 1,
                    ">" if tokens.get(i.saturating_sub(1)) != Some(&"-") => {
                        angle -= 1;
                        if angle == 0 {
                            i += 1;
                            break;
                        }
                    }
                    ";" | "{" => break,
                    _ => {}
                }
                i += 1;
            }
        }
        if tokens.get(i) != Some(&"(") {
            continue;
        }
        let mut paren = 0usize;
        let mut angle = 0usize;
        let mut takes_ui = false;
        while i < tokens.len() {
            match tokens[i] {
                "(" => paren += 1,
                ")" => {
                    paren -= 1;
                    if paren == 0 {
                        i += 1;
                        break;
                    }
                }
                "<" if paren == 1 => angle += 1,
                ">" if paren == 1 => angle = angle.saturating_sub(1),
                ":" if paren == 1 && angle == 0 => {
                    let mut j = i + 1;
                    if tokens.get(j) == Some(&"&") {
                        j += 1;
                        if tokens.get(j) == Some(&"'") {
                            j += 2;
                        }
                        if tokens.get(j) == Some(&"mut") {
                            j += 1;
                        }
                    }
                    while matches!(tokens.get(j), Some(&"crate" | &"super"))
                        && tokens.get(j + 1) == Some(&"::")
                    {
                        j += 2;
                    }
                    if tokens.get(j) == Some(&"Ui")
                        && matches!(tokens.get(j + 1), Some(&"," | &")"))
                    {
                        takes_ui = true;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        if takes_ui {
            count += 1;
        }
    }
    count
}

#[test]
fn ui_parameter_count_only_shrinks() {
    let (root, files) = app_sources();
    let actual: usize = files
        .iter()
        .filter(|file| !file_name(&root, file).ends_with("/tests.rs"))
        .map(|file| ui_parameter_functions(&shipped(&code(file))))
        .sum();
    assert_eq!(
        actual, UI_PARAMETER_LIMIT,
        "shipped functions taking whole `Ui` changed (actual={actual}, limit={UI_PARAMETER_LIMIT}); \
         narrow parameters to `Ui` sub-handles and lower `UI_PARAMETER_LIMIT` as they shrink"
    );
}

const FEATURE_TREES: [&str; 4] = ["guide", "playback", "library_ui", "screens"];

/// Ambient service facades in `wiring/` that `kit/**` building blocks may read
/// (reactive settings, device reports, and the runtime handle). Dispatch and
/// feature glue (`wiring::navigation`, `wiring::data`, `wiring::runtime_events`,
/// `wiring::tasks`, …) stay forbidden from `kit/**`.
const KIT_WIRING_SERVICES: &[&str] = &["device", "runtime_owner", "settings"];

#[test]
fn kit_has_no_feature_shell_or_dispatch_wiring_dependency() {
    let (root, files) = app_sources();
    for file in files {
        let name = file_name(&root, &file);
        if !name.starts_with("kit/") || name.ends_with("/tests.rs") {
            continue;
        }
        let source = shipped(&code(&file));
        for path in rooted_paths(&source, &name) {
            let first = path[0];
            for forbidden in FEATURE_TREES.into_iter().chain(["shell"]) {
                assert!(
                    first != forbidden && !is_module_in(&root, forbidden, first),
                    "{name} depends on {forbidden} via {}",
                    path.join("::")
                );
            }
            if first == "wiring" {
                let submodule = path.get(1).copied().unwrap_or("*");
                assert!(
                    KIT_WIRING_SERVICES.contains(&submodule),
                    "{name} depends on wiring::{submodule} (only {KIT_WIRING_SERVICES:?} are allowed in kit/)"
                );
            } else {
                assert!(
                    !is_module_in(&root, "wiring", first),
                    "{name} depends on wiring via root alias {}",
                    path.join("::")
                );
            }
        }
    }
    // Keep the ambient wiring facades themselves free of feature trees and shell
    // so `kit/` cannot transitively depend on them.
    for service in KIT_WIRING_SERVICES {
        let name = format!("wiring/{service}.rs");
        let source = shipped(&code(&root.join(&name)));
        for path in rooted_paths(&source, &name) {
            let first = path[0];
            for forbidden in FEATURE_TREES.into_iter().chain(["shell"]) {
                assert!(
                    first != forbidden && !is_module_in(&root, forbidden, first),
                    "{name} (used by kit/) depends on {forbidden} via {}",
                    path.join("::")
                );
            }
        }
    }
}

#[test]
fn feature_trees_never_name_shell() {
    let (root, files) = app_sources();
    for file in files {
        let name = file_name(&root, &file);
        let Some((tree, _)) = name.split_once('/') else {
            continue;
        };
        if !FEATURE_TREES.contains(&tree) || name.ends_with("/tests.rs") {
            continue;
        }
        let source = shipped(&code(&file));
        for path in rooted_paths(&source, &name) {
            let first = path[0];
            assert!(
                first != "shell" && !is_module_in(&root, "shell", first),
                "{name} names shell via {}; route through wiring::navigation instead",
                path.join("::")
            );
        }
    }
}

#[test]
fn ui_parameter_counter_handles_refs_paths_generics_and_tests() {
    let source = "
        fn takes_value(ui: Ui) {}
        fn takes_ref(ui: &Ui, other: usize) {}
        fn takes_crate_path<T: Fn(Ui)>(ui: crate::Ui, cb: T) {}
        fn takes_sub_handle(screens: ScreensUi) {}
        fn takes_fn_ptr(cb: fn(Ui)) {}
        #[cfg(test)]
        mod tests {
            fn test_helper(ui: Ui) {}
        }
    ";
    assert_eq!(ui_parameter_functions(&shipped(source)), 3);
}
