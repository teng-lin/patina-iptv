//! Synthetic credentials exercise the real PIN modal, remote input and gates.
use crate::{
    headless::Headless,
    personalization::{self, step},
};
use iptv_settings::{Settings, Value};
use std::time::Duration;

fn digits(app: &mut Headless, pin: &str) {
    for digit in pin.chars() {
        step(app.press_key(&digit.to_string()));
    }
}
/// Enters `pin` on the on-screen keypad with arrows and OK alone, from
/// the first key, then presses its Continue the same way. The keypad is
/// 1 2 3 / 4 5 6 / 7 8 9 / Delete 0 Continue.
fn keypad(app: &mut Headless, pin: &str) {
    let mut at = 0_usize;
    let mut go = |app: &mut Headless, to: usize| {
        while at / 3 != to / 3 {
            let down = at / 3 < to / 3;
            step(app.press_key(if down { "down" } else { "up" }));
            at = if down { at + 3 } else { at - 3 };
        }
        while at != to {
            let right = at < to;
            step(app.press_key(if right { "right" } else { "left" }));
            at = if right { at + 1 } else { at - 1 };
        }
        step(app.press_key("enter"));
    };
    for digit in pin.bytes() {
        let digit = usize::from(digit - b'0');
        go(app, if digit == 0 { 10 } else { digit - 1 });
    }
    let masked = "\u{2022}".repeat(pin.len());
    step(app.wait_for(&format!("id:parental-layer >> text:{masked}")));
    go(app, 11);
}
fn continue_entry(app: &mut Headless) {
    step(app.press("id:parental-layer >> text:Continue"));
}
fn wait(app: &mut Headless, text: &str) {
    step(app.wait_for_within(
        &format!("id:parental-layer >> text:{text}"),
        Duration::from_secs(20),
    ));
}
fn settings_menu(app: &mut Headless) {
    crate::personalization::open_program_menu(app);
    step(app.wait_for("id:menu-panel"));
    step(app.press("id:menu-panel >> text:Settings"));
}

#[test]
fn guide_data_mutations_wait_for_pin_and_cancel_keeps_the_store() {
    let Some(home) = isolated!() else { return };
    let guide = home.path().join("generated-guide.xml");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    std::fs::write(&guide, format!(
        "<tv><channel id=\"local-a\"><display-name>Local A</display-name></channel>\
         <programme channel=\"local-a\" start=\"{}\" stop=\"{}\"><title>Generated bulletin</title></programme></tv>",
        iptv_epg::format_xmltv_time(now - 60), iptv_epg::format_xmltv_time(now + 3600),
    )).unwrap();
    let mut settings = Settings::new();
    for (key, value) in [
        ("general.resume_last_channel", Value::Bool(false)),
        ("guide.autoplay", Value::Bool(false)),
        ("epg.update", Value::choice("on_request")),
        (
            "parental.require_pin_for",
            Value::choice("blocked_content_and_settings"),
        ),
        ("parental.unlock_window", Value::choice("every_time")),
    ] {
        settings.set(key, value).unwrap();
    }
    let fixture = personalization::launch_prepared(
        home,
        settings,
        false,
        |path, source| {
            crate::isolated::open_database(path)
                .unwrap()
                .set_guide_sources(
                    source,
                    &[iptv_store_sqlite::GuideSource {
                        feed: iptv_epg::FeedId(700),
                        provider_index: None,
                        url: iptv_providers::SecretUrl::new(guide.to_string_lossy()),
                        name: Some("Generated protected feed".into()),
                        enabled: true,
                        shift_minutes: 0,
                    }],
                )
                .unwrap();
        },
        |_| {},
        |_| {},
    );
    let (mut app, path, source) = (fixture.app, fixture.path, fixture.source);
    step(app.open_guide_groups());
    step(app.press_key("right"));
    personalization::menu(&mut app, "Protect channel with PIN");
    wait(&mut app, "Choose a new PIN");
    digits(&mut app, "2468");
    continue_entry(&mut app);
    wait(&mut app, "Confirm new PIN");
    digits(&mut app, "2468");
    continue_entry(&mut app);
    wait(&mut app, "Save your recovery code");
    step(app.press("text:I saved the code"));
    personalization::menu(&mut app, "Manage PIN-protected content…");
    wait(&mut app, "Blocked content");
    step(app.press("text:Lock now"));
    step(app.open_guide_groups());
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.press("text:Playlists >> nth=0"));
    step(app.press("id:playlists-list >> text:Synthetic personalization"));
    for _ in 0..30 {
        step(app.press_key("down"));
    }
    step(app.press_key("up"));
    step(app.press_key("up"));
    step(app.press("id:playlist-actions-list >> text:Guide data"));
    step(app.wait_for("text:Guide data · Synthetic personalization"));
    let snapshot = || {
        let store = crate::isolated::open_database(&path).unwrap();
        (
            store.epg_guide(source).unwrap(),
            store.guide_sources(source).unwrap(),
        )
    };
    let before = snapshot();
    assert_eq!(before.0.as_ref().unwrap().feeds[0].program_count, 1);
    // Update, editing and confirmed Clear all require the PIN even when
    // Guide data was reached through Playlists instead of Settings.
    step(app.press("id:guide-data-list >> text:Update the guide now"));
    wait(&mut app, "Enter parental PIN");
    step(app.press("id:parental-cancel"));
    assert_eq!(snapshot(), before);
    step(app.press("id:guide-data-list >> text:1.  Generated protected feed"));
    step(app.press("id:guide-source-actions >> text:Switch off"));
    wait(&mut app, "Enter parental PIN");
    step(app.press("id:parental-cancel"));
    assert_eq!(snapshot(), before);
    step(app.back());
    step(app.press("id:guide-data-list >> text:Delete stored guide data"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    wait(&mut app, "Enter parental PIN");
    step(app.press("id:parental-cancel"));
    assert_eq!(snapshot(), before);
    // Unlock resumes the selected update, then editing. Closing the
    // per-use screen revokes that grant before Clear is attempted.
    step(app.press("id:guide-data-list >> text:Update the guide now"));
    wait(&mut app, "Enter parental PIN");
    digits(&mut app, "2468");
    continue_entry(&mut app);
    step(app.wait_for("text:Guide updated"));
    step(app.press("id:guide-data-list >> text:1.  Generated protected feed"));
    step(app.press("id:guide-source-actions >> text:No time shift"));
    step(app.wait_until(
        "authorized source edit is saved",
        Duration::from_secs(5),
        |_| snapshot().1[0].shift_minutes == 30,
    ));
    step(app.back());
    step(app.back());
    step(app.press("id:playlist-actions-list >> text:Guide data"));
    step(app.press("id:guide-data-list >> text:Delete stored guide data"));
    let before_clear = snapshot();
    assert_eq!(before_clear.0.as_ref().unwrap().feeds[0].program_count, 1);
    step(app.press_key("down"));
    step(app.press_key("enter"));
    wait(&mut app, "Enter parental PIN");
    digits(&mut app, "2468");
    continue_entry(&mut app);
    step(app.wait_for("text:Guide data deleted"));
    assert!(snapshot().0.is_none());
    assert_eq!(
        snapshot().1,
        before_clear.1,
        "Clear retains the guide source"
    );
}

#[test]
fn setup_block_remote_unlock_change_and_recovery() {
    let Some(home) = isolated!() else { return };
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    let fixture = personalization::start(home, settings, false);
    let (mut app, path, source) = (fixture.app, fixture.path, fixture.source);
    personalization::menu(&mut app, "Protect channel with PIN");
    wait(&mut app, "Choose a new PIN");
    // The on-screen keypad works with D-pad/OK alone (2, 4, 6, 8, Continue).
    for key in [
        "right", "enter", "down", "left", "enter", "right", "right", "enter", "down", "left",
        "enter", "down", "right", "enter",
    ] {
        step(app.press_key(key));
    }
    wait(&mut app, "Confirm new PIN");
    digits(&mut app, "2469");
    continue_entry(&mut app);
    wait(&mut app, "PINs did not match; choose a new PIN");
    digits(&mut app, "2468");
    continue_entry(&mut app);
    digits(&mut app, "2468");
    // Delay the credential commit so Back/Cancel races are deterministic.
    let writer = crate::store_latency::hold_writer(&fixture.home);
    continue_entry(&mut app);
    wait(&mut app, "Checking…");
    assert!(
        app.screen()
            .accessibility_state("id:parental-cancel")
            .unwrap()
            .disabled
    );
    step(app.back());
    assert!(app.shows("Checking…"));
    assert!(app.press("id:parental-cancel").is_err());
    writer.execute_batch("ROLLBACK").unwrap();
    wait(&mut app, "Save your recovery code");
    let code = app
        .screen()
        .texts()
        .into_iter()
        .find(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap()
        .to_owned();
    step(app.press("text:I saved the code"));
    step(app.wait_for("text:Content blocked"));
    let store = crate::isolated::open_database(&path).unwrap();
    assert!(store.parental_credential().unwrap().is_some());
    assert!(
        store
            .parental_blocks()
            .unwrap()
            .iter()
            .any(|target| target.source == source.0)
    );
    drop(store);
    // The manager has a footer after its content and Lock now. Saturating
    // Down must reach that actual Cancel control, rather than an invisible
    // keypad index or the destructive Lock now action.
    personalization::menu(&mut app, "Manage PIN-protected content…");
    wait(&mut app, "Blocked content");
    for _ in 0..5 {
        step(app.press_key("down"));
    }
    assert_eq!(
        app.screen()
            .span_style("id:parental-cancel >> text:Cancel")
            .unwrap()
            .color,
        nsh_core::Color((iptv_app::theme::FOCUSED_TEXT << 8) | 0xff)
    );
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:Blocked content", Duration::from_secs(3)));
    assert!(
        !crate::isolated::open_database(&path)
            .unwrap()
            .parental_blocks()
            .unwrap()
            .is_empty()
    );
    // A manager selection completes one authorized operation without asking
    // for the same administrative PIN again after closing the manager.
    personalization::menu(&mut app, "Manage PIN-protected content…");
    // Cancel deliberately retired the previous administrative grant.
    wait(&mut app, "Enter parental PIN");
    digits(&mut app, "2468");
    continue_entry(&mut app);
    wait(&mut app, "Blocked content");
    let unblock = app
        .screen()
        .texts()
        .into_iter()
        .find(|text| text.starts_with("Unblock "))
        .unwrap()
        .to_owned();
    step(app.press(&format!("text:{unblock}")));
    step(app.wait_for("text:Content unblocked"));
    assert!(!app.shows("Enter parental PIN"));
    personalization::menu(&mut app, "Protect channel with PIN");
    step(app.wait_for("text:Content blocked"));
    // Close the settings grant explicitly before trying another viewing action.
    personalization::menu(&mut app, "Manage PIN-protected content…");
    wait(&mut app, "Blocked content");
    step(app.press("text:Lock now"));
    // The default blocked-content-only policy leaves unrelated settings open.
    settings_menu(&mut app);
    step(app.wait_for("id:settings-screen"));
    assert!(!app.shows("Enter parental PIN"));
    step(app.back());
    // Adding another provider from Playlists still requires the administrative PIN.
    crate::browse_fixture::open_add_playlist(&mut app);
    wait(&mut app, "Enter parental PIN");
    digits(&mut app, "2468");
    continue_entry(&mut app);
    step(app.wait_for("id:wizard-form"));
    step(app.back());
    step(app.back());
    step(app.press_key("tab"));
    step(app.press_key("enter"));
    wait(&mut app, "Enter parental PIN");
    digits(&mut app, "1357");
    continue_entry(&mut app);
    wait(&mut app, "Incorrect PIN or recovery code");
    step(app.press("text:Forgot PIN?"));
    wait(&mut app, "Recover parental PIN");
    step(app.type_text("id:parental-recovery-input", &code));
    continue_entry(&mut app);
    wait(&mut app, "Choose a new PIN");
    digits(&mut app, "8642");
    continue_entry(&mut app);
    digits(&mut app, "8642");
    continue_entry(&mut app);
    wait(&mut app, "Save your recovery code");
    let rotated = app
        .screen()
        .texts()
        .into_iter()
        .find(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap()
        .to_owned();
    assert_ne!(code, rotated);
    step(app.press("text:I saved the code"));
    wait(&mut app, "Enter parental PIN");
    digits(&mut app, "8642");
    continue_entry(&mut app);
    step(app.wait_for_gone("text:Enter parental PIN", Duration::from_secs(20)));
    // No credential or recovery code is persisted in the public settings JSON.
    let saved = std::fs::read_to_string(fixture.home.data_dir().join("settings.json")).unwrap();
    assert!(!saved.contains(&code));
    assert!(!saved.contains("8642"));
}

/// A PIN longer than four digits, entered on the keypad by D-pad (a remote
/// without number keys) and on number keys, when setting it and at unlock
/// (#282). OK on the keypad presses keys until Continue; after number keys
/// it submits.
#[test]
fn six_digit_pins_by_dpad_and_number_keys() {
    let Some(home) = isolated!() else { return };
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    settings
        .set(
            "parental.require_pin_for",
            Value::Enum("blocked_content_and_settings".into()),
        )
        .unwrap();
    let fixture = personalization::start(home, settings, false);
    let mut app = fixture.app;
    personalization::menu(&mut app, "Protect channel with PIN");
    wait(&mut app, "Choose a new PIN");
    keypad(&mut app, "246813");
    wait(&mut app, "Confirm new PIN");
    keypad(&mut app, "246813");
    wait(&mut app, "Save your recovery code");
    step(app.press("text:I saved the code"));
    step(app.wait_for("text:Content blocked"));
    personalization::menu(&mut app, "Manage PIN-protected content…");
    wait(&mut app, "Blocked content");
    step(app.press("text:Lock now"));
    settings_menu(&mut app);
    wait(&mut app, "Enter parental PIN");
    keypad(&mut app, "246813");
    step(app.wait_for("text:Parental control"));
    // Changing it on number keys: OK submits after the sixth digit.
    step(app.press("text:Parental control"));
    step(app.press("text:Set the PIN"));
    wait(&mut app, "Enter current PIN");
    digits(&mut app, "246813");
    step(app.press_key("enter"));
    wait(&mut app, "Choose a new PIN");
    digits(&mut app, "135790");
    step(app.press_key("enter"));
    wait(&mut app, "Confirm new PIN");
    digits(&mut app, "135790");
    step(app.press_key("enter"));
    wait(&mut app, "Save your recovery code");
    step(app.press("text:I saved the code"));
    step(app.back());
    step(app.back());
    settings_menu(&mut app);
    wait(&mut app, "Enter parental PIN");
    digits(&mut app, "135790");
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:Enter parental PIN", Duration::from_secs(20)));
}

#[test]
fn number_key_entry_protects_settings_and_changes_an_existing_pin() {
    let Some(home) = isolated!() else { return };
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    settings
        .set(
            "parental.require_pin_for",
            Value::Enum("blocked_content_and_settings".into()),
        )
        .unwrap();
    let fixture = personalization::start(home, settings, false);
    let (mut app, path) = (fixture.app, fixture.path);
    personalization::menu(&mut app, "Protect channel with PIN");
    wait(&mut app, "Choose a new PIN");
    digits(&mut app, "2468");
    step(app.press_key("enter"));
    wait(&mut app, "Confirm new PIN");
    digits(&mut app, "2468");
    step(app.press_key("enter"));
    wait(&mut app, "Save your recovery code");
    step(app.press("text:I saved the code"));
    step(app.wait_for("text:Content blocked"));
    let previous = crate::isolated::open_database(&path)
        .unwrap()
        .parental_credential()
        .unwrap()
        .unwrap()
        .salt;
    personalization::menu(&mut app, "Manage PIN-protected content…");
    wait(&mut app, "Blocked content");
    step(app.press("text:Lock now"));
    settings_menu(&mut app);
    wait(&mut app, "Enter parental PIN");
    digits(&mut app, "2468");
    step(app.press_key("enter"));
    step(app.wait_for("text:Parental control"));
    step(app.press("text:Parental control"));
    step(app.press("text:Set the PIN"));
    wait(&mut app, "Enter current PIN");
    // A change verifies the existing PIN; choosing a replacement alone cannot bypass it.
    for pin in ["2469", "1357", "1357"] {
        digits(&mut app, pin);
        step(app.press_key("enter"));
    }
    wait(&mut app, "Incorrect PIN or recovery code");
    for pin in ["2468", "1357", "1357"] {
        digits(&mut app, pin);
        step(app.press_key("enter"));
    }
    wait(&mut app, "Save your recovery code");
    step(app.press("text:I saved the code"));
    let replaced = crate::isolated::open_database(&path)
        .unwrap()
        .parental_credential()
        .unwrap()
        .unwrap()
        .salt;
    assert_ne!(previous, replaced);
    step(app.back());
    step(app.back());
    settings_menu(&mut app);
    wait(&mut app, "Enter parental PIN");
    digits(&mut app, "2468");
    step(app.press_key("enter"));
    wait(&mut app, "Incorrect PIN or recovery code");
    digits(&mut app, "1357");
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:Enter parental PIN", Duration::from_secs(20)));
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod hidden_window {
    use super::*;
    use nsh_core::element::Root;
    use nsh_core::prelude::*;
    use nsh_engine::EngineConfig;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    static REOPEN: AtomicBool = AtomicBool::new(false);

    #[component]
    fn WindowHarness() -> impl IntoElement {
        let window = nsh_core::element::desktop::main_window();
        let _timer = set_interval(Duration::from_millis(10), move || {
            if REOPEN.swap(false, Ordering::SeqCst) {
                window.focus();
            }
        });
        <iptv_app::App as nsh_core::element::Component>::element(())
    }
    fn reopen(app: &mut Headless) {
        REOPEN.store(true, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            app.settle();
            if app
                .window_state("main")
                .unwrap()
                .is_some_and(|window| window.visible)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the recording window did not acknowledge reopening"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!REOPEN.load(Ordering::SeqCst));
    }
    fn code(screen: &nsh_testing::Screen) -> Option<String> {
        screen
            .texts()
            .into_iter()
            .find(|text| text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(str::to_owned)
    }

    #[test]
    fn hiding_a_window_during_pin_commit_gates_the_notice_and_stops_video() {
        use iptv_core::recording::{JobId, Request};
        use iptv_providers::{
            SecretUrl,
            model::{M3uSourceConfig, Source, SourceConfig},
        };
        use std::time::{SystemTime, UNIX_EPOCH};
        let Some(home) = isolated!() else { return };
        let root = home.data_dir();
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("iptv.sqlite");
        let mut store = crate::isolated::open_database(&path).unwrap();
        let source = store
            .add_source(&Source {
                name: "Generated recording owner".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://example.test/generated.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        let catalog = iptv_providers::m3u::parse_m3u(personalization::M3U);
        store
            .replace_catalog(source, &catalog.groups, &catalog.channels.items, &[], &[])
            .unwrap();
        store
            .set_source_policy(
                source,
                iptv_store_sqlite::SourcePolicy {
                    refresh_on_start: false,
                    ..Default::default()
                },
            )
            .unwrap();
        store.set_active_source(source).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        store
            .schedule_recording(
                JobId(681),
                &Request {
                    source: source.0,
                    channel: catalog.channels.items[0].id.as_str().into(),
                    channel_name: "Generated recording channel".into(),
                    title: "Generated future program".into(),
                    start: now + 3600,
                    end: now + 7200,
                },
                now,
            )
            .unwrap();
        drop(store);
        let mut settings = Settings::new();
        settings
            .set("guide.stay_on_guide", Value::Bool(true))
            .unwrap();
        settings.set("guide.autoplay", Value::Bool(false)).unwrap();
        settings
            .set("general.resume_last_channel", Value::Bool(false))
            .unwrap();
        std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
        let mut config = EngineConfig {
            viewport: nsh_core::Size::new(px(1920.0), px(1080.0)),
            ..EngineConfig::new(Root::of::<WindowHarness>())
        };
        config.environment.os = if cfg!(target_os = "macos") {
            nsh_core::platform::Os::MacOs
        } else if cfg!(windows) {
            nsh_core::platform::Os::Windows
        } else {
            nsh_core::platform::Os::Linux
        };
        config.environment.idiom = nsh_core::platform::Idiom::Desktop;
        let mut app = Headless::start(config).unwrap();
        // Headless excludes hidden windows from its visible screen. Observe
        // mounted content separately to check that the notice itself is gated.
        let mounts = Arc::new(Mutex::new(nsh_testing::Screen::default()));
        let observed = mounts.clone();
        app.observe(move |_, batch| observed.lock().unwrap().apply(batch));
        step(app.wait_for("text:Local A"));
        step(app.press_key("home"));
        step(app.press_key("enter"));
        step(
            app.wait_until("generated preview plays", Duration::from_secs(5), |_| {
                iptv_app::player_has_source(0)
            }),
        );
        personalization::menu(&mut app, "Protect channel with PIN");
        wait(&mut app, "Choose a new PIN");
        digits(&mut app, "2468");
        continue_entry(&mut app);
        digits(&mut app, "2468");
        let writer = crate::store_latency::hold_writer(&home);
        continue_entry(&mut app);
        wait(&mut app, "Checking…");
        step(app.press_window_close("main"));
        assert!(!app.window_state("main").unwrap().unwrap().visible);
        assert!(!iptv_app::player_has_source(0));
        writer.execute_batch("ROLLBACK").unwrap();
        step(crate::isolated::wait_outside(
            &mut app,
            "PIN committed while the window is hidden",
            Duration::from_secs(20),
            || {
                crate::isolated::open_database(&path)
                    .ok()
                    .and_then(|store| store.parental_credential().ok().flatten())
                    .is_some()
            },
        ));
        step(app.wait_until(
            "PIN entry mounted while hidden",
            Duration::from_secs(20),
            |_| {
                mounts
                    .lock()
                    .unwrap()
                    .matches("text:Enter parental PIN")
                    .unwrap()
            },
        ));
        assert!(
            code(&mounts.lock().unwrap()).is_none(),
            "hidden credential results never render their code before fresh verification"
        );
        assert!(!app.window_state("main").unwrap().unwrap().visible);
        assert!(
            !iptv_app::player_has_source(0),
            "the completed write never resumes hidden playback"
        );

        // Closing during an in-flight verification invalidates its grant.
        reopen(&mut app);
        wait(&mut app, "Enter parental PIN");
        assert!(code(app.screen()).is_none());
        let writer = crate::store_latency::hold_writer(&home);
        digits(&mut app, "2468");
        continue_entry(&mut app);
        wait(&mut app, "Checking…");
        step(app.press_window_close("main"));
        writer.execute_batch("ROLLBACK").unwrap();
        reopen(&mut app);
        wait(&mut app, "Enter parental PIN");
        step(app.wait_for_gone("text:Checking…", Duration::from_secs(20)));
        assert!(
            code(app.screen()).is_none(),
            "a stale verify cannot redisplay the held credential"
        );
        assert!(!iptv_app::player_has_source(0));

        digits(&mut app, "2468");
        continue_entry(&mut app);
        wait(&mut app, "Save your recovery code");
        let saved =
            code(app.screen()).expect("fresh PIN verification reveals the committed result");
        step(app.press_window_close("main"));
        step(app.wait_until(
            "PIN entry mounted after hiding",
            Duration::from_secs(20),
            |_| {
                mounts
                    .lock()
                    .unwrap()
                    .matches("text:Enter parental PIN")
                    .unwrap()
            },
        ));
        assert!(
            code(&mounts.lock().unwrap()).is_none(),
            "hiding an already visible notice gates it again"
        );
        reopen(&mut app);
        digits(&mut app, "2468");
        continue_entry(&mut app);
        wait(&mut app, "Save your recovery code");
        assert_eq!(
            code(app.screen()).as_deref(),
            Some(saved.as_str()),
            "re-authentication preserves, rather than rotates or loses, the code"
        );

        step(app.press_window_close("main"));
        reopen(&mut app);
        step(app.back());
        assert!(code(app.screen()).is_none());
        personalization::menu(&mut app, "Protect channel with PIN");
        wait(&mut app, "Enter parental PIN");
        assert!(
            !iptv_app::player_has_source(0),
            "Cancel drops the held result and leaves access locked"
        );
        let store = crate::isolated::open_database(&path).unwrap();
        assert!(store.parental_credential().unwrap().is_some());
    }
}
