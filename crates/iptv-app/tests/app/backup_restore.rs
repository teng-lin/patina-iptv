//! Synthetic whole-app export, schedule/cancel and next-launch restore.
//! The backup keeps the guide's setup and local file, not its downloaded
//! programs: the restored app downloads the guide again at its first start.
use super::personalization::step;
use crate::{accessibility_visibility::Exposure, headless::Headless};
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_settings::{Settings, Target, Value, keys};
use iptv_store_sqlite::{ChannelQuery, GuideSource, OrderScope, SourceId, SourcePolicy, Store};
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
            for phase in ["exercise", "verify", "welcome"] {
                let child_home = if phase == "welcome" {
                    directory.path().join(format!("{language}-welcome"))
                } else {
                    home.clone()
                };
                std::fs::create_dir_all(&child_home).unwrap();
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([name, "--exact", "--nocapture", "--test-threads=1"])
                    .env("IPTV_BACKUP_PHASE", phase)
                    .env("IPTV_BACKUP_LANGUAGE", language)
                    .env("IPTV_RS_TEST_LOCALE", "en-US")
                    .env("IPTV_BACKUP_ARCHIVE", home.join("generated.patinabackup"))
                    .env("HOME", &child_home)
                    .env("LOCALAPPDATA", &child_home)
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
    if phase.as_deref() == Some("welcome") {
        restore_from_welcome(&root);
        return;
    }
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
        assert_eq!(
            store.guide_sources(source).unwrap()[0].name.as_deref(),
            Some("Generated local guide")
        );
        drop(store);
        // The restored library had no programs: these are downloaded again.
        wait_for_guide(&mut app, &root.join("iptv.sqlite"), source);
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
    let guide = root.join("playlists/generated/guide.xml");
    std::fs::write(
        &guide,
        crate::support::guide(
            crate::support::now(),
            &[("one.example", "Channel One", "Generated guide program")],
        ),
    )
    .unwrap();
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
    store
        .set_guide_sources(
            source,
            &[GuideSource {
                feed: iptv_epg::FeedId(1001),
                provider_index: None,
                url: SecretUrl::new(guide.to_string_lossy()),
                name: Some("Generated local guide".into()),
                enabled: true,
                shift_minutes: 0,
            }],
        )
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
    // A first start downloads the guide, which the backup then leaves out.
    wait_for_guide(&mut app, &path, source);
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
    let pending = root.with_extension("restore-pending");
    review(&mut app);
    step(app.press_key("enter"));
    step(app.wait_for_gone("id:restore-review", Duration::from_secs(3)));
    assert!(!pending.exists(), "Cancel is the initial restore focus");
    schedule(&mut app);
    step(app.wait_for(&message("Scheduled for next launch")));
    assert!(pending.exists());
    {
        let staged = crate::isolated::open_database(pending.join("iptv.sqlite")).unwrap();
        let guide = staged.epg_guide(source).unwrap().unwrap();
        assert_eq!(guide.replaced_at, None, "staged as never downloaded");
        assert!(guide.bound_channels > 0, "the channel matches stay");
        assert_eq!(programs(&staged, source), 0);
        assert!(programs(&crate::isolated::open_database(&path).unwrap(), source) > 0);
    }
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

fn restore_from_welcome(root: &std::path::Path) {
    std::fs::create_dir_all(root).unwrap();
    let mut settings = Settings::new();
    settings
        .set("general.language", Value::text(&language_tag()))
        .unwrap();
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let mut app = start_app_at(Size::new(px(400.), px(700.)));
    let exposure = Exposure::default();
    let observed = exposure.clone();
    app.observe(move |_, batch| observed.record(batch));
    step(app.wait_for("id:wizard-welcome"));
    step(app.on_pick_paths(vec![
        std::env::var_os("IPTV_BACKUP_ARCHIVE").unwrap().into(),
    ]));
    step(app.press("id:wizard-welcome-restore"));
    step(app.wait_for("id:restore-review"));
    step(app.wait_for("text:generated.patinabackup"));
    exposure.shown(&app, "wizard-layer", false);
    exposure.shown(&app, "restore-review", true);

    // A short viewport starts on the review explanation. The remote can
    // reveal the whole replacement warning without leaving the safe focus.
    step(app.set_viewport(Size::new(px(400.), px(340.))));
    step(app.wait_for(&message(
        "Review the selected backup before scheduling the restore.",
    )));
    for _ in 0..4 {
        if warning_visible(app.screen()) {
            break;
        }
        step(app.press_key("down"));
    }
    step(app.wait_until(
        "the complete replacement warning remains reachable",
        Duration::from_secs(3),
        warning_visible,
    ));
    step(app.back());
    step(app.wait_for_gone("id:restore-review", Duration::from_secs(3)));
    exposure.shown(&app, "wizard-layer", true);
    assert!(!root.with_extension("restore-pending").exists());

    step(app.set_viewport(Size::new(px(400.), px(700.))));
    step(app.press("id:wizard-welcome-restore"));
    step(app.wait_for("id:restore-review"));
    step(app.press(&message("Schedule restore")));
    step(app.wait_for_within(
        &message("Restore scheduled. Close and reopen Patina to apply it"),
        Duration::from_secs(20),
    ));
    assert!(root.with_extension("restore-pending").exists());
}

fn warning_visible(screen: &nsh_testing::Screen) -> bool {
    let Ok(scroll_id) = screen.id("id:restore-review >> id:setup-scroll") else {
        return false;
    };
    let nodes = screen.layout_nodes();
    let Some(warning) = nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some("restore-warning"))
    else {
        return false;
    };
    let Some(scroll) = nodes.iter().find(|node| node.id == scroll_id) else {
        return false;
    };
    warning.bounds.origin.y.0 >= scroll.bounds.origin.y.0
        && warning.bounds.origin.y.0 + warning.bounds.size.height.0
            <= scroll.bounds.origin.y.0 + scroll.bounds.size.height.0
}

/// The source's stored programs within a day of now.
fn programs(store: &Store, source: SourceId) -> usize {
    let now = crate::support::now();
    let (from, to) = (now - 86_400, now + 86_400);
    let guide = store
        .load_epg(
            source,
            &iptv_store_sqlite::EpgWindow {
                from,
                to,
                keys: None,
                descriptions: false,
            },
        )
        .unwrap();
    guide
        .bindings()
        .map(|(key, _)| guide.programs(key, from, to).count())
        .sum()
}

/// Waits until `source`'s guide was downloaded and stored.
fn wait_for_guide(app: &mut Headless, path: &std::path::Path, source: SourceId) {
    step(crate::isolated::wait_outside(
        app,
        "the guide download",
        Duration::from_secs(20),
        || {
            let store = crate::isolated::open_database(path).unwrap();
            store
                .epg_guide(source)
                .unwrap()
                .is_some_and(|guide| guide.replaced_at.is_some())
                && programs(&store, source) > 0
        },
    ));
}

fn language_tag() -> String {
    std::env::var("IPTV_BACKUP_LANGUAGE").unwrap_or_else(|_| "en".into())
}

fn word(source: &str) -> String {
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

fn review(app: &mut Headless) {
    step(app.press(&message("Restore a backup")));
    step(app.wait_for("id:settings-editor"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for("id:restore-review"));
    step(app.wait_for("text:generated.patinabackup"));
    step(app.wait_for(&message("When you reopen Patina, this backup replaces any current playlists, settings, favorites, groups, and watch history.")));
}

fn schedule(app: &mut Headless) {
    review(app);
    step(app.press_key("left"));
    step(app.press_key("enter"));
    step(app.wait_for_within(
        &message("Restore scheduled. Close and reopen Patina to apply it"),
        Duration::from_secs(20),
    ));
}

fn start_app() -> Headless {
    start_app_at(Size::new(px(1920.), px(1080.)))
}

fn start_app_at(viewport: Size<Points>) -> Headless {
    Headless::start(EngineConfig {
        viewport,
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap()
}
