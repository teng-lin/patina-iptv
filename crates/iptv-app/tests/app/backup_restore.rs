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
    step(app.open_guide_groups());
    step(app.press_key("right"));
    crate::personalization::open_program_menu(&mut app);
    step(app.wait_for(&message("Manage favorites…")));
    step(app.press(&message("Settings >> nth=0")));
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
    iptv_presentation::i18n::translate(
        iptv_presentation::i18n::resolve(&language_tag(), None),
        source,
    )
    .to_owned()
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

/// Opens Settings on its first page (General) from the guide.
fn open_settings(app: &mut Headless) {
    crate::personalization::open_program_menu(app);
    step(app.wait_for(&message("Manage favorites…")));
    step(app.press(&message("Settings >> nth=0")));
    step(app.wait_for(&message("Clear history")));
}

#[track_caller]
fn absent(app: &Headless, label: &str) {
    assert!(
        app.screen().id(&message(label)).is_err(),
        "{label} is shown"
    );
}

/// The iPad has no backup document transport yet: no Backup or Restore
/// row, nor their Data and privacy heading.
#[test]
fn ipad_settings_show_no_backup_rows() {
    let Some(home) = isolated!() else { return };
    iptv_app::set_pickers_for_test(iptv_app::Pickers::of(
        iptv_core::capabilities::Platform::Ipad,
    ));
    let mut fixture = crate::personalization::start(home, Settings::default(), false);
    let app = &mut fixture.app;
    open_settings(app);
    for label in ["Back up", "Restore a backup", "Data and privacy"] {
        absent(app, label);
    }
}

/// Each action follows its own picker: a device that creates files but
/// cannot open them offers Back up alone, and one that opens files but
/// cannot create them offers Restore alone, both under Data and privacy.
#[test]
fn each_backup_action_follows_its_own_picker() {
    let Some(home) = isolated!() else { return };
    iptv_app::set_pickers_for_test(iptv_app::Pickers {
        create: true,
        open: false,
    });
    let mut fixture = crate::personalization::start(home, Settings::default(), false);
    {
        let app = &mut fixture.app;
        open_settings(app);
        step(app.wait_for(&message("Data and privacy")));
        step(app.wait_for(&message("Back up")));
        absent(app, "Restore a backup");
    }
    let crate::personalization::Fixture { app, home, .. } = fixture;
    drop(app);

    iptv_app::set_pickers_for_test(iptv_app::Pickers {
        create: false,
        open: true,
    });
    let mut app = start_app();
    step(app.wait_for_within("text:★ Local A", Duration::from_secs(20)));
    step(app.open_guide_groups());
    step(app.press_key("right"));
    open_settings(&mut app);
    step(app.wait_for(&message("Data and privacy")));
    step(app.wait_for(&message("Restore a backup")));
    absent(&app, "Back up");
    drop(app);
    drop(home);
}

/// A device without document pickers (an Android TV box without a usable
/// one) offers neither action, yet a restore scheduled earlier stays
/// reachable: it can be reviewed, kept and canceled, never replaced.
#[test]
fn without_pickers_a_scheduled_restore_stays_reviewable_and_cancelable() {
    let Some(home) = isolated!() else { return };
    // A backup takes local playlists only from the app's storage.
    let playlist = home.path().join("synthetic.m3u");
    let mut fixture = crate::personalization::launch_prepared(
        home,
        Settings::default(),
        false,
        |path, source| {
            let stored = path.parent().unwrap().join("playlists/generated/list.m3u");
            std::fs::create_dir_all(stored.parent().unwrap()).unwrap();
            std::fs::copy(&playlist, &stored).unwrap();
            crate::isolated::open_database(path)
                .unwrap()
                .update_source(
                    source,
                    &Source {
                        name: "Synthetic personalization".into(),
                        config: SourceConfig::M3u(M3uSourceConfig {
                            location: SecretUrl::new(stored.to_string_lossy()),
                            user_agent: None,
                            referrer: None,
                        }),
                    },
                )
                .unwrap();
        },
        |_| {},
        |_| {},
    );
    step(fixture.app.open_guide_groups());
    step(fixture.app.press_key("right"));
    let pending = fixture.home.data_dir().with_extension("restore-pending");
    let archive = fixture.home.path().join("generated.patinabackup");
    {
        // A desktop's pickers: both actions, under Data and privacy.
        let app = &mut fixture.app;
        open_settings(app);
        step(app.wait_for(&message("Data and privacy")));
        step(app.on_pick_save_path(Some(archive.clone())));
        app.on_dialog(|_| DialogAnswer::Press(word("Choose file…")));
        step(app.press(&message("Back up")));
        step(app.wait_for_within(&message("Backup saved"), Duration::from_secs(20)));
        step(app.on_pick_paths(vec![archive.clone()]));
        schedule(app);
        assert!(pending.exists());
    }
    let crate::personalization::Fixture { app, home, .. } = fixture;
    drop(app);

    iptv_app::set_pickers_for_test(iptv_app::Pickers::NONE);
    let mut app = start_app();
    step(app.wait_for_within("text:★ Local A", Duration::from_secs(20)));
    step(app.open_guide_groups());
    step(app.press_key("right"));
    open_settings(&mut app);
    step(app.wait_for(&message("Scheduled for next launch")));
    step(app.wait_for(&message("Data and privacy")));
    absent(&app, "Back up");
    let buttons = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = buttons.clone();
    // The first review keeps the restore, the second cancels it.
    let mut answers = vec![word("Cancel restore"), word("Keep scheduled")];
    app.on_dialog(move |dialog| {
        let texts: Vec<String> = dialog.buttons.iter().map(|b| b.text.clone()).collect();
        seen.lock().unwrap().push(texts);
        DialogAnswer::Press(answers.pop().unwrap())
    });
    for _ in 0..2 {
        step(app.press(&message("Restore a backup")));
        step(app.wait_for(&message(
            "A restore is already scheduled. Next, choose what happens to it.",
        )));
        step(app.press_key("down"));
        step(app.press_key("enter"));
        step(app.wait_for_gone("id:settings-editor", Duration::from_secs(3)));
    }
    step(app.wait_for(&message("Scheduled restore canceled")));
    assert!(!pending.exists());
    let buttons = buttons.lock().unwrap().clone();
    assert_eq!(buttons.len(), 2, "{buttons:?}");
    for shown in &buttons {
        assert!(shown.contains(&word("Cancel restore")), "{shown:?}");
        assert!(
            !shown.contains(&word("Choose another backup…")),
            "{shown:?}"
        );
    }
    // Nothing scheduled and no picker: the entry point goes too.
    step(app.wait_for_gone(&message("Restore a backup"), Duration::from_secs(3)));
    absent(&app, "Data and privacy");
    drop(app);
    drop(home);
}
/// Rewrites only generated archives, retaining their file ordering and lengths.
fn rewrite_generated_archive(
    archive: &[u8],
    edit: impl FnOnce(&mut serde_json::Value, &mut Vec<Vec<u8>>),
) -> Vec<u8> {
    let size = u64::from_le_bytes(archive[9..17].try_into().unwrap()) as usize;
    let mut manifest: serde_json::Value = serde_json::from_slice(&archive[17..17 + size]).unwrap();
    let mut offset = 17 + size;
    let mut payloads = Vec::new();
    for entry in manifest["files"].as_array().unwrap() {
        let length = entry["length"].as_u64().unwrap() as usize;
        payloads.push(archive[offset..offset + length].to_vec());
        offset += length;
    }
    edit(&mut manifest, &mut payloads);
    if let Some(entries) = manifest
        .get_mut("files")
        .and_then(serde_json::Value::as_array_mut)
    {
        for (entry, payload) in entries.iter_mut().zip(&payloads) {
            entry["length"] = serde_json::json!(payload.len());
        }
    }
    let header = serde_json::to_vec(&manifest).unwrap();
    let mut bytes = b"PATINABK\x01".to_vec();
    bytes.extend_from_slice(&(header.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&header);
    for payload in payloads {
        bytes.extend_from_slice(&payload);
    }
    bytes
}

fn generated_tree(
    path: &std::path::Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn walk(
        root: &std::path::Path,
        path: &std::path::Path,
        files: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
    ) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    walk(path, path, &mut files);
    files
}

#[test]
fn first_run_explains_backup_compatibility_and_keeps_the_scheduled_restore() {
    let Some(home) = isolated!() else { return };
    let root = home.data_dir();
    std::fs::create_dir_all(&root).unwrap();
    let mut app = start_app_at(Size::new(px(400.), px(700.)));
    step(app.wait_for("id:wizard-welcome"));
    exercise_compatibility_failures(&mut app, home.path(), &root, true);
    step(app.wait_for("id:wizard-welcome"));
    assert!(
        crate::isolated::open_database(root.join("iptv.sqlite"))
            .unwrap()
            .sources()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn settings_explains_backup_compatibility_and_keeps_current_data_and_scheduled_restore() {
    let Some(home) = isolated!() else { return };
    let mut fixture = crate::personalization::start(home, Settings::default(), false);
    open_settings(&mut fixture.app);
    let root = fixture.home.data_dir();
    let settings = std::fs::read(root.join("settings.json")).unwrap();
    let playlist = std::fs::read(fixture.home.path().join("synthetic.m3u")).unwrap();
    exercise_compatibility_failures(&mut fixture.app, fixture.home.path(), &root, false);
    let store = crate::isolated::open_database(&fixture.path).unwrap();
    assert_eq!(
        store.source(fixture.source).unwrap().unwrap().source.name,
        "Synthetic personalization"
    );
    let channels = store
        .load_channels(fixture.source, &ChannelQuery::default())
        .unwrap();
    assert_eq!(channels.len(), 3);
    assert!(channels.iter().all(|channel| {
        store
            .is_favorite(fixture.source, &channel.channel.id)
            .unwrap()
    }));
    assert_eq!(std::fs::read(root.join("settings.json")).unwrap(), settings);
    assert_eq!(
        std::fs::read(fixture.home.path().join("synthetic.m3u")).unwrap(),
        playlist
    );
    step(fixture.app.wait_for(&message("Scheduled for next launch")));
}

#[track_caller]
fn compatibility_step(
    app: &mut Headless,
    context: &str,
    action: impl FnOnce(&mut Headless) -> Result<(), String>,
) {
    if let Err(error) = action(app) {
        panic!(
            "{context}: {error}\npending dialog: {:?}\nall native dialog requests: {:#?}",
            app.pending_dialog(),
            app.screen().dialogs(),
        );
    }
}

fn select_and_schedule_generated(
    app: &mut Headless,
    archive: &std::path::Path,
    welcome: bool,
    context: &str,
    replace_pending: bool,
) -> usize {
    // Parental admission awaits storage before the pending-restore prompt, so
    // caller-drive both native dialogs rather than relying on input settling.
    app.answer_dialogs_myself();
    let dialogs_before_request = app.screen().dialogs().len();
    compatibility_step(app, context, |app| {
        app.on_pick_paths(vec![archive.to_owned()])
    });
    if welcome {
        compatibility_step(app, context, |app| app.press("id:wizard-welcome-restore"));
    } else {
        compatibility_step(app, context, |app| app.press(&message("Restore a backup")));
        compatibility_step(app, context, |app| app.wait_for("id:settings-editor"));
        compatibility_step(app, context, |app| app.press_key("down"));
        compatibility_step(app, context, |app| app.press_key("enter"));
    }
    if replace_pending {
        compatibility_step(app, context, |app| {
            app.wait_until(
                "the scheduled restore replacement prompt",
                Duration::from_secs(20),
                |screen| screen.dialogs().len() > dialogs_before_request,
            )
        });
        assert_eq!(
            app.pending_dialog()
                .and_then(|dialog| dialog.title.as_deref()),
            Some(word("Restore scheduled").as_str()),
            "{context}: {:?}",
            app.screen().dialogs(),
        );
        compatibility_step(app, context, |app| {
            app.answer_dialog(DialogAnswer::Press(word("Choose another backup…")))
        });
    }
    compatibility_step(app, context, |app| app.wait_for("id:restore-review"));
    // Async worker dialogs can arrive after press().done() returns. wait_until()
    // pumps them but does not dispatch on_dialog handlers: inspect and answer the
    // failure explicitly so the test does not depend on worker timing.
    app.answer_dialogs_myself();
    let dialogs_before_schedule = app.screen().dialogs().len();
    compatibility_step(app, context, |app| app.press(&message("Schedule restore")));
    dialogs_before_schedule
}

fn exercise_compatibility_failures(
    app: &mut Headless,
    home: &std::path::Path,
    root: &std::path::Path,
    welcome: bool,
) {
    let snapshot = home.join("generated-backup.sqlite");
    Store::open_in_memory()
        .unwrap()
        .snapshot_to(&snapshot)
        .unwrap();
    let database = std::fs::read(&snapshot).unwrap();
    let mut settings = Settings::new().to_json();
    // The supported first settings version is still a restorable backup.
    settings["version"] = serde_json::json!(1);
    let settings = serde_json::to_vec(&settings).unwrap();
    let manifest = serde_json::to_vec(&serde_json::json!({
        "format": "patina-whole-app", "version": 1, "root": root,
        "files": [
            {"name": "iptv.sqlite", "length": database.len()},
            {"name": "settings.json", "length": settings.len()}
        ]
    }))
    .unwrap();
    let mut baseline = b"PATINABK\x01".to_vec();
    baseline.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
    baseline.extend_from_slice(&manifest);
    baseline.extend_from_slice(&database);
    baseline.extend_from_slice(&settings);
    let selected = home.join("generated.patinabackup");
    std::fs::write(&selected, &baseline).unwrap();
    select_and_schedule_generated(app, &selected, welcome, "supported baseline", false);
    compatibility_step(app, "supported baseline", |app| {
        app.wait_for_within(
            &message("Restore scheduled. Close and reopen Patina to apply it"),
            Duration::from_secs(20),
        )
    });
    let pending = root.with_extension("restore-pending");
    let queued = generated_tree(&pending);
    assert!(!queued.is_empty());

    let mut cases = Vec::new();
    let version_message = |template: &str, found: u64, supported: u64| {
        word(template)
            .replace("{found}", &found.to_string())
            .replace("{supported}", &supported.to_string())
    };
    let newer =
        word("Use a newer Patina version that supports this backup, or choose another backup.");
    let format_detail = version_message(
        "Backup format version {found} is not supported; this Patina supports version {supported}.",
        2,
        1,
    );
    let settings_detail = version_message(
        "This Patina cannot restore these backup settings (version {found}; supported version {supported}).",
        u64::from(iptv_settings::backup::VERSION + 1),
        u64::from(iptv_settings::backup::VERSION),
    );
    let database_detail = version_message(
        "Backup database schema {found} is not supported; this Patina supports schema {supported}.",
        u64::from(iptv_store_sqlite::SCHEMA_VERSION + 1),
        u64::from(iptv_store_sqlite::SCHEMA_VERSION),
    );
    let mut future_envelope = baseline.clone();
    future_envelope[8] = 2;
    cases.push((
        "newer envelope",
        future_envelope,
        format_detail.clone(),
        newer.clone(),
    ));
    cases.push((
        "newer manifest with changed layout",
        rewrite_generated_archive(&baseline, |header, _| {
            header["version"] = serde_json::json!(2);
            header.as_object_mut().unwrap().remove("files");
            header.as_object_mut().unwrap().remove("root");
        }),
        format_detail.clone(),
        newer.clone(),
    ));
    cases.push((
        "newer settings with changed layout",
        rewrite_generated_archive(&baseline, |_, payloads| {
            payloads[1] = serde_json::to_vec(&serde_json::json!({
            "format": iptv_settings::backup::FORMAT, "version": iptv_settings::backup::VERSION + 1,
            "global": ["generated future layer"]
        })).unwrap();
        }),
        settings_detail,
        newer.clone(),
    ));
    cases.push(("newer database schema", rewrite_generated_archive(&baseline, |_, payloads| {
        let future = home.join("generated-future.sqlite");
        std::fs::write(&future, &payloads[0]).unwrap();
        let db = rusqlite::Connection::open(&future).unwrap();
        db.execute("INSERT INTO schema_migrations(version,name,applied_at) VALUES (?1,'generated_future',0)", [iptv_store_sqlite::SCHEMA_VERSION + 1]).unwrap();
        db.pragma_update(None, "user_version", iptv_store_sqlite::SCHEMA_VERSION + 1).unwrap();
        drop(db);
        payloads[0] = std::fs::read(future).unwrap();
    }), database_detail, newer));
    cases.push((
        "foreign backup format",
        rewrite_generated_archive(&baseline, |header, _| {
            header["format"] = serde_json::json!("generated-private-format-token");
        }),
        word("This backup format is not supported."),
        word("Choose another backup, or use a Patina version that supports this backup."),
    ));

    let compatible =
        word("Choose another backup, or use a Patina version that supports this backup.");
    cases.push((
        "unsupported settings enum",
        rewrite_generated_archive(&baseline, |_, payloads| {
            payloads[1] = serde_json::to_vec(&serde_json::json!({
                "format": iptv_settings::backup::FORMAT,
                "version": iptv_settings::backup::VERSION,
                "global": {"appearance.font_size": "generated_larger"}
            })).unwrap();
        }),
        version_message(
            "This Patina cannot restore these backup settings (version {found}; supported version {supported}).",
            u64::from(iptv_settings::backup::VERSION),
            u64::from(iptv_settings::backup::VERSION),
        ),
        compatible.clone(),
    ));
    cases.push((
        "foreign settings format",
        rewrite_generated_archive(&baseline, |_, payloads| {
            payloads[1] = serde_json::to_vec(&serde_json::json!({
                "format": "generated-private-format-token", "version": 1
            }))
            .unwrap();
        }),
        word("This backup contains settings in an unsupported format."),
        compatible,
    ));
    let damaged = word(
        "This backup is damaged or incomplete. Choose another backup, or create a new backup on the original device.",
    );
    cases.push((
        "truncated archive",
        b"PATINABK".to_vec(),
        damaged.clone(),
        String::new(),
    ));
    cases.push((
        "invalid UTF-8 settings",
        rewrite_generated_archive(&baseline, |_, payloads| {
            payloads[1] = vec![0xff, 0xfe];
        }),
        damaged,
        String::new(),
    ));

    cases.push((
        "missing referenced playlist",
        rewrite_generated_archive(&baseline, |_, payloads| {
            let incomplete = home.join("generated-missing-local.sqlite");
            std::fs::write(&incomplete, &payloads[0]).unwrap();
            let mut store = Store::open(&incomplete).unwrap();
            store.add_source(&Source {
                name: "Generated missing local playlist".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new(root.join("playlists/generated/list.m3u").to_string_lossy()),
                    user_agent: None,
                    referrer: None,
                }),
            }).unwrap();
            drop(store);
            payloads[0] = std::fs::read(incomplete).unwrap();
        }),
        word("This backup is damaged or incomplete. Choose another backup, or create a new backup on the original device."),
        String::new(),
    ));
    let count = cases.len();
    let entrypoint = if welcome { "first run" } else { "Settings" };
    for (index, (name, bytes, explanation, next_step)) in cases.into_iter().enumerate() {
        let context = format!("{entrypoint}, case {}/{count}: {name}", index + 1);
        std::fs::write(&selected, bytes).unwrap();
        let dialogs_before_schedule =
            select_and_schedule_generated(app, &selected, welcome, &context, true);
        compatibility_step(app, &context, |app| {
            app.wait_until(
                "the persistent backup compatibility explanation",
                Duration::from_secs(20),
                |screen| screen.dialogs().len() > dialogs_before_schedule,
            )
        });
        let diagnostics = format!(
            "{context}\npending dialog: {:?}\nall native dialog requests: {:#?}",
            app.pending_dialog(),
            app.screen().dialogs(),
        );
        let dialog = app.pending_dialog().expect(&diagnostics);
        assert_eq!(
            dialog.title.as_deref(),
            Some(word("Backup could not be restored").as_str()),
            "{diagnostics}",
        );
        let description = dialog.message.as_deref().unwrap_or_default();
        assert!(description.contains(&explanation), "{diagnostics}");
        if !next_step.is_empty() {
            assert!(description.contains(&next_step), "{diagnostics}");
        }
        assert!(
            description.contains(&word(
                "Your current data and any previously scheduled restore have been kept."
            )),
            "{diagnostics}"
        );
        assert!(
            !description.contains("generated-private-format-token"),
            "{diagnostics}"
        );
        assert_eq!(
            dialog
                .buttons
                .iter()
                .map(|button| &button.text)
                .collect::<Vec<_>>(),
            [&word("Close")],
            "{diagnostics}",
        );
        compatibility_step(app, &context, |app| {
            app.answer_dialog(DialogAnswer::Press(word("Close")))
        });
        assert_eq!(
            generated_tree(&pending),
            queued,
            "{context}: rejection changed the accepted restore",
        );
        compatibility_step(app, &context, |app| {
            app.wait_for_gone("id:restore-review", Duration::from_secs(3))
        });
    }
}
