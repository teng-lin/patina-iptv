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
    personalization::menu(&mut app, "Block channel");
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
    continue_entry(&mut app);
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
    personalization::menu(&mut app, "Manage blocked channels…");
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
    personalization::menu(&mut app, "Manage blocked channels…");
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
    personalization::menu(&mut app, "Block channel");
    step(app.wait_for("text:Content blocked"));
    // Close the settings grant explicitly before trying another viewing action.
    personalization::menu(&mut app, "Manage blocked channels…");
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
    personalization::menu(&mut app, "Block channel");
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
    personalization::menu(&mut app, "Manage blocked channels…");
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
