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
    step(app.press_key("m"));
    step(app.wait_for("id:menu-panel"));
    step(app.press("id:menu-panel >> text:Settings"));
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
    // Adding another provider is administrative even with generic settings open.
    step(app.press("text:+ Add playlist"));
    wait(&mut app, "Enter parental PIN");
    digits(&mut app, "2468");
    continue_entry(&mut app);
    step(app.wait_for("text:What did your provider give you?"));
    step(app.back());
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
    step(app.wait_until(
        "authorized playback opens fullscreen",
        Duration::from_secs(5),
        |screen| {
            screen.layout_nodes().iter().any(|node| {
                node.kind == nsh_core::ViewKind::Native && node.bounds.size.width.0 > 1900.0
            })
        },
    ));
    // No credential or recovery code is persisted in the public settings JSON.
    let saved = std::fs::read_to_string(fixture.home.data_dir().join("settings.json")).unwrap();
    assert!(!saved.contains(&code));
    assert!(!saved.contains("8642"));
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
        .set("parental.pin_input", Value::Enum("number_keys".into()))
        .unwrap();
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
    use std::sync::atomic::{AtomicBool, Ordering};
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
    fn code(app: &Headless) -> Option<String> {
        app.screen()
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
                    title: "Generated future programme".into(),
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
        wait(&mut app, "Enter parental PIN");
        assert!(
            code(&app).is_none(),
            "hidden credential results never render their code before fresh verification"
        );
        assert!(!app.window_state("main").unwrap().unwrap().visible);
        assert!(
            !iptv_app::player_has_source(0),
            "the completed write never resumes hidden playback"
        );

        // Closing during an in-flight verification invalidates its grant.
        reopen(&mut app);
        let writer = crate::store_latency::hold_writer(&home);
        digits(&mut app, "2468");
        continue_entry(&mut app);
        wait(&mut app, "Checking…");
        step(app.press_window_close("main"));
        writer.execute_batch("ROLLBACK").unwrap();
        step(app.wait_for_gone("text:Checking…", Duration::from_secs(20)));
        assert!(
            code(&app).is_none(),
            "a stale verify cannot redisplay the held credential"
        );
        assert!(!iptv_app::player_has_source(0));

        reopen(&mut app);
        digits(&mut app, "2468");
        continue_entry(&mut app);
        wait(&mut app, "Save your recovery code");
        let saved = code(&app).expect("fresh PIN verification reveals the committed result");
        step(app.press_window_close("main"));
        assert!(
            code(&app).is_none(),
            "hiding an already visible notice gates it again"
        );
        reopen(&mut app);
        digits(&mut app, "2468");
        continue_entry(&mut app);
        wait(&mut app, "Save your recovery code");
        assert_eq!(
            code(&app).as_deref(),
            Some(saved.as_str()),
            "re-authentication preserves, rather than rotates or loses, the code"
        );

        step(app.press_window_close("main"));
        reopen(&mut app);
        step(app.back());
        assert!(code(&app).is_none());
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
