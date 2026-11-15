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
    // Close the settings grant explicitly before trying another viewing action.
    personalization::menu(&mut app, "Manage blocked channels…");
    wait(&mut app, "Blocked content");
    step(app.press("text:Lock now"));
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
