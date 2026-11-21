//! Synthetic whole-app export, schedule/cancel and next-launch restore.
use super::personalization::step;
use crate::headless::Headless;
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_settings::{Settings, Target, Value, keys};
use iptv_store_sqlite::{ChannelQuery, OrderScope, SourcePolicy};
use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use nsh_testing::DialogAnswer;
use std::time::Duration;

#[test]
fn backup_export_cancel_restore_and_restart_preserve_personalized_library() {
    crate::isolated::initialize_key();
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Info)
        .try_init();
    let phase = std::env::var("IPTV_BACKUP_PHASE").ok();
    if phase.is_none() {
        let directory = tempfile::tempdir().unwrap();
        let name = "backup_restore::backup_export_cancel_restore_and_restart_preserve_personalized_library";
        for language in ["en", "zh-Hant"] {
            let home = directory.path().join(language);
            std::fs::create_dir_all(&home).unwrap();
            for phase in ["exercise", "verify"] {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([name, "--exact", "--nocapture", "--test-threads=1"])
                    .env("IPTV_BACKUP_PHASE", phase)
                    .env("IPTV_BACKUP_LANGUAGE", language)
                    .env("IPTV_RS_TEST_LOCALE", "en-US")
                    .env("HOME", &home)
                    .env("LOCALAPPDATA", &home)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{phase} check failed: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        return;
    }
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let root = home.join(if cfg!(windows) {
        "Patina"
    } else if cfg!(target_os = "macos") {
        "Library/Application Support/iptv-rs"
    } else {
        ".local/share/iptv-rs"
    });
    if phase.as_deref() == Some("verify") {
        let mut app = start_app();
        step(app.wait_for_within("text:★ Channel Two", Duration::from_secs(20)));
        let store = crate::isolated::open_database(root.join("iptv.sqlite")).unwrap();
        let source = store.sources().unwrap()[0].id;
        assert_eq!(
            store.source(source).unwrap().unwrap().source.name,
            "Generated backup source"
        );
        assert_eq!(
            store.custom_groups(source).unwrap()[0].name,
            "Generated saved group"
        );
        assert_eq!(
            store
                .load_channels(source, &ChannelQuery::default())
                .unwrap()[0]
                .name,
            "Channel Two"
        );
        assert!(!root.with_extension("restore-pending").exists());
        let (settings, report) =
            Settings::from_json(&std::fs::read_to_string(root.join("settings.json")).unwrap())
                .unwrap();
        assert!(report.is_clean());
        assert_eq!(
            settings.text(keys::LANGUAGE, &Target::Global),
            language_tag()
        );
        step(app.wait_for(&message("Nothing playing")));
        return;
    }
    std::fs::create_dir_all(root.join("playlists/generated")).unwrap();
    let playlist = root.join("playlists/generated/list.m3u");
    let text = "#EXTM3U\n#EXTINF:-1 group-title=\"Generated\",Channel One\nhttp://example.test/one\n#EXTINF:-1 group-title=\"Generated\",Channel Two\nhttp://example.test/two\n";
    std::fs::write(&playlist, text).unwrap();
    let mut initial = Settings::new();
    initial
        .set("general.language", Value::text(&language_tag()))
        .unwrap();
    std::fs::write(root.join("settings.json"), initial.to_json_string()).unwrap();
    let path = root.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated backup source".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(playlist.to_string_lossy()),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    store
        .set_source_policy(
            source,
            SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(text);
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    let favorite = parsed.channels.items[1].id.clone();
    store.set_favorite(source, &favorite, true).unwrap();
    store
        .move_channel(source, &OrderScope::All, &favorite, 0)
        .unwrap();
    let group = store
        .create_custom_group(source, "Generated saved group")
        .unwrap();
    store.add_to_custom_group(group, &favorite).unwrap();
    drop(store);
    let mut app = start_app();
    step(app.wait_for_within("text:★ Channel Two", Duration::from_secs(20)));
    step(app.press("text:/^☰  /"));
    step(app.press_key("right"));
    step(app.press_key("m"));
    step(app.wait_for(&message("Manage favorites…")));
    step(app.press(&message("Settings >> nth=1")));
    step(app.wait_for(&message("Back up")));
    let archive = home.join("generated.patinabackup");
    step(app.on_pick_save_path(Some(archive.clone())));
    // Native dialogs default to Cancel: canceling export creates no archive.
    step(app.press(&message("Back up")));
    assert!(!archive.exists());
    app.on_dialog(|_| DialogAnswer::Press(word("Choose file…")));
    step(app.press(&message("Back up")));
    step(app.wait_for_within(&message("Backup saved"), Duration::from_secs(20)));
    let bytes = std::fs::read(&archive).unwrap();
    assert!(!bytes.is_empty());
    step(app.on_pick_paths(vec![archive.clone()]));
    app.on_dialog(|_| DialogAnswer::Press(word("Schedule restore")));
    schedule(&mut app);
    step(app.wait_for(&message("Scheduled for next launch")));
    let pending = root.with_extension("restore-pending");
    assert!(pending.exists());
    // The running library remains editable until restart.
    crate::isolated::open_database(&path)
        .unwrap()
        .rename_source(source, "Generated changed after backup")
        .unwrap();
    app.on_dialog(|_| DialogAnswer::Press(word("Cancel restore")));
    step(app.press(&message("Restore a backup")));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for(&message("Scheduled restore canceled")));
    step(app.wait_for_gone(
        &message("Scheduled for next launch"),
        Duration::from_secs(3),
    ));
    assert!(!pending.exists());
    app.on_dialog(|_| DialogAnswer::Press(word("Schedule restore")));
    schedule(&mut app);
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .source(source)
            .unwrap()
            .unwrap()
            .source
            .name,
        "Generated changed after backup"
    );
    drop(app);
    // Recreating an engine in this process must leave staged state alone:
    // worker handles from the previous root may still be alive.
    let mut app = start_app();
    step(app.wait_for_within("text:★ Channel Two", Duration::from_secs(20)));
    assert!(pending.exists());
    assert_eq!(
        crate::isolated::open_database(&path)
            .unwrap()
            .source(source)
            .unwrap()
            .unwrap()
            .source
            .name,
        "Generated changed after backup"
    );
    drop(app);
    let mut changed = Settings::new();
    changed.set("general.language", Value::text("de")).unwrap();
    std::fs::write(root.join("settings.json"), changed.to_json_string()).unwrap();
}

fn language_tag() -> String {
    std::env::var("IPTV_BACKUP_LANGUAGE").unwrap_or_else(|_| "en".into())
}

fn word(source: &str) -> String {
    // The existing restore confirmation composes its heading with a question
    // mark rather than storing a separate catalog key.
    if !iptv_core::i18n::is_cataloged(source)
        && let Some(base) = source.strip_suffix('?')
        && iptv_core::i18n::is_cataloged(base)
    {
        return format!("{}?", word(base));
    }
    iptv_core::i18n::translate(iptv_core::i18n::resolve(&language_tag(), None), source).to_owned()
}

fn message(source: &str) -> String {
    let (source, suffix) = source
        .split_once(" >> ")
        .map_or((source, ""), |(s, suffix)| (s, suffix));
    let selector = format!("text:{}", word(source));
    if suffix.is_empty() {
        selector
    } else {
        format!("{selector} >> {suffix}")
    }
}

fn schedule(app: &mut Headless) {
    step(app.press(&message("Restore a backup")));
    step(app.wait_for(&message("Restore a backup?")));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for_within(
        &message("Restore scheduled. Close and reopen Patina to apply it"),
        Duration::from_secs(20),
    ));
}

fn start_app() -> Headless {
    Headless::start(EngineConfig {
        viewport: Size::new(px(1920.), px(1080.)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap()
}
