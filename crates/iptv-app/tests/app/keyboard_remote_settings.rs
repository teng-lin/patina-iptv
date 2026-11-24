//! Keyboard and Remote settings (#280), run headless with Android's
//! device observer simulated (`device.rs`): the page exists while keys are
//! connected, not after one was used. A touch-only phone has no page; a
//! tablet shows Keyboard settings as soon as a keyboard is attached, with
//! its shortcuts remappable without a key press first; a D-pad makes them
//! Remote settings; a desktop has Keyboard settings without the keys only
//! a remote has, and a TV keeps Remote settings. Guide autoplay and its
//! delay go with the keys on a handheld and to Advanced on a desktop or a
//! TV.
//!
//! Synthetic data only: the personalization playlist (M3U).

use std::time::Duration;

use crate::headless::Headless;
use crate::{device_signals, personalization};
use iptv_settings::{Settings, Value};
use nsh_core::Size;
use nsh_core::prelude::px;
use personalization::step;

const WAIT: Duration = Duration::from_secs(5);
const KEYBOARD: &str = "role:tab[Keyboard]";
const REMOTE: &str = "role:tab[Remote control]";
const ADVANCED: &str = "role:tab[Advanced]";
/// Guide autoplay's row.
const AUTOPLAY: &str = "text:Play the focused channel by itself";
/// The keys that seek in replays, now with the keys' settings.
const SEEK_KEYS: &str = "text:Seek in replays with";

fn fixture(here: &str, settings: Settings) -> Option<personalization::Fixture> {
    let home = crate::isolated::run_with_env(here, &[("IPTV_TEST_DEVICE_SIGNALS", "1")])?;
    let mut fixture = personalization::start(home, settings, false);
    step(fixture.app.set_viewport(Size::new(px(1280.0), px(720.0))));
    fixture.app.settle();
    Some(fixture)
}

/// Settings from the navigation rail, by touch.
fn open_settings(app: &mut Headless) {
    step(app.open_guide_groups());
    step(app.press("id:navigation-rail >> role:tab[Settings]"));
    step(app.wait_for_within("role:tab[General]", WAIT));
}

fn shows(app: &Headless, selector: &str) -> bool {
    app.screen().matches(selector).unwrap()
}

/// The pages listed now.
fn pages(app: &Headless) -> Vec<String> {
    app.screen()
        .texts()
        .into_iter()
        .skip_while(|t| *t != "General")
        .take_while(|t| *t != "About")
        .map(str::to_owned)
        .collect()
}

#[track_caller]
fn page_gone(app: &mut Headless, selector: &str) {
    step(app.wait_for_gone(selector, WAIT));
}

/// The view showing the page name `name` in the pages list.
fn page_node(app: &Headless, name: &str) -> nsh_core::NodeId {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.text.as_ref().is_some_and(|t| t.text.text == name))
        .unwrap_or_else(|| panic!("no page {name}"))
        .id
}

/// The keys' page, tapped: its rows, and the bindings editor's row with
/// the changed keys summed up (no row of their own).
fn keys_page(app: &mut Headless, page: &str, edit: &str) {
    step(app.wait_for_within(page, WAIT));
    step(app.press(page));
    step(app.wait_for_within(&format!("text:{edit}"), WAIT));
    assert!(shows(app, SEEK_KEYS), "the seek keys are here");
    assert!(shows(app, "text:Channel number pause"));
    assert!(!shows(app, "text:Changed keys"), "summed up by the editor");
    // No keys changed yet.
    step(app.wait_for(&format!("role:button[/^{edit}/] >> text:None")));
}

/// The keys the bindings editor, opened by a tap, offers: a tap on its
/// key row moves to the next one.
fn editor_keys(app: &mut Headless, edit: &str, title: &str) -> Vec<String> {
    step(app.press(&format!("text:{edit}")));
    step(app.wait_for_within(&format!("text:{title}"), WAIT));
    let key = || "id:remote-Key";
    step(app.wait_for(key()));
    // Once round: a remote's every key, its digits and a keyboard's own.
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..50 {
        let label = app
            .screen()
            .texts()
            .into_iter()
            .find(|t| t.starts_with("Key: "))
            .map(str::to_owned)
            .expect("the key row");
        if !seen.contains(&label) {
            seen.push(label);
        }
        step(app.press(key()));
    }
    step(app.press("id:remote-editor-screen >> text:Close"));
    step(app.wait_for_gone("id:remote-editor-screen", WAIT));
    seen
}

#[test]
fn a_touch_only_phone_has_no_keys_page() {
    fn here() {}
    let mut settings = Settings::new();
    settings.set("guide.autoplay", Value::Bool(true)).unwrap();
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here), settings) else {
        return;
    };
    let app = &mut fixture.app;
    device_signals::report(app, "phone", false, false);
    // The stored autoplay is off while its row is hidden: moving the
    // guide's focus to another channel tunes nothing.
    app.idle(Duration::from_millis(1500)).unwrap();
    let opens = iptv_app::player_opens(0);
    step(app.press_key("down"));
    app.idle(Duration::from_millis(1500)).unwrap();
    assert_eq!(iptv_app::player_opens(0), opens, "autoplay is off");
    // With a keyboard attached the stored value applies again.
    device_signals::report(app, "phone", true, false);
    step(app.press_key("down"));
    step(
        app.wait_until("autoplay tunes the next channel", WAIT, |_| {
            iptv_app::player_opens(0) > opens
        }),
    );

    device_signals::report(app, "phone", false, false);
    open_settings(app);
    let listed = pages(app);
    assert!(!shows(app, KEYBOARD) && !shows(app, REMOTE), "{listed:?}");
    // Autoplay and its delay are hidden with the keys, and nothing is
    // advanced on a handheld.
    assert!(!shows(app, ADVANCED), "{listed:?}");
}

#[test]
fn a_keyboard_attached_to_a_tablet_shows_keyboard_settings_at_once() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here), Settings::new()) else {
        return;
    };
    let app = &mut fixture.app;
    device_signals::report(app, "tablet", false, false);
    open_settings(app);
    assert!(!shows(app, KEYBOARD), "{:?}", pages(app));
    let parental = page_node(app, "Parental control");
    // Attached while Settings is open, before any key is pressed: the
    // page comes, after Playback, and the page shown stays chosen.
    device_signals::report(app, "tablet", true, false);
    step(app.wait_for_within(KEYBOARD, WAIT));
    // The pages after it keep their entries (a screen reader's place).
    assert_eq!(page_node(app, "Parental control"), parental);
    let listed = pages(app);
    let at = |name: &str| listed.iter().position(|p| p == name);
    assert_eq!(at("Keyboard"), at("Playback").map(|p| p + 1), "{listed:?}");
    assert!(!shows(app, ADVANCED), "{listed:?}");
    step(app.wait_for("text:Play the last channel on start"));
    // Its shortcuts can be remapped, by touch.
    keys_page(app, KEYBOARD, "Edit keyboard shortcuts");
    let keys = editor_keys(app, "Edit keyboard shortcuts", "Keyboard shortcuts");
    assert!(keys.iter().any(|k| k.starts_with("Key: Menu")), "{keys:?}");
    for remote_only in ["Key: Red", "Key: Green", "Key: Record", "Key: Stop"] {
        assert!(!keys.contains(&remote_only.to_owned()), "{keys:?}");
    }
    // Detached while its editor is open over its rows: the editor closes
    // with the page, and the page in its place is chosen.
    step(app.press("text:Edit keyboard shortcuts"));
    step(app.wait_for_within("id:remote-editor-screen", WAIT));
    device_signals::report(app, "tablet", false, false);
    step(app.wait_for_gone("id:remote-editor-screen", WAIT));
    page_gone(app, KEYBOARD);
    assert!(!shows(app, SEEK_KEYS));
    assert!(shows(app, "role:tab[Parental control]"));
}

#[test]
fn a_d_pad_shows_remote_settings_with_every_key() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here), Settings::new()) else {
        return;
    };
    let app = &mut fixture.app;
    device_signals::report(app, "phone", false, true);
    open_settings(app);
    assert!(!shows(app, KEYBOARD));
    keys_page(app, REMOTE, "Edit remote bindings");
    let keys = editor_keys(app, "Edit remote bindings", "Remote bindings");
    for key in ["Key: Red", "Key: Green", "Key: Record"] {
        assert!(keys.contains(&key.to_owned()), "{keys:?}");
    }
    assert!(!shows(app, ADVANCED));
}

#[test]
fn a_desktop_keeps_keyboard_settings_and_puts_zapping_in_advanced() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here), Settings::new()) else {
        return;
    };
    let app = &mut fixture.app;
    // A desktop build, unreported: a desktop with a keyboard.
    open_settings(app);
    assert!(!shows(app, REMOTE));
    keys_page(app, KEYBOARD, "Edit keyboard shortcuts");
    // Named with the key that stands in for a remote's; without a hold,
    // which a desktop's keys cannot report.
    step(app.press("text:Edit keyboard shortcuts"));
    step(app.wait_for_within("text:Keyboard shortcuts", WAIT));
    assert!(!shows(app, "id:remote-Press"), "no hold on a desktop");
    step(app.press("id:remote-editor-screen >> text:Close"));
    step(app.wait_for_gone("id:remote-editor-screen", WAIT));
    let keys = editor_keys(app, "Edit keyboard shortcuts", "Keyboard shortcuts");
    assert!(keys.contains(&"Key: Menu (M)".to_owned()), "{keys:?}");
    assert!(keys.contains(&"Key: Channel + (=)".to_owned()), "{keys:?}");
    assert!(!keys.contains(&"Key: Blue".to_owned()), "{keys:?}");
    // Autoplay and its delay are advanced settings.
    step(app.press(ADVANCED));
    step(app.wait_for_within(AUTOPLAY, WAIT));
    step(app.wait_for("text:Wait before tuning"));
}

#[test]
fn a_tv_keeps_remote_settings_and_puts_zapping_in_advanced() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here), Settings::new()) else {
        return;
    };
    let app = &mut fixture.app;
    device_signals::report(app, "tv", false, true);
    open_settings(app);
    assert!(!shows(app, KEYBOARD));
    keys_page(app, REMOTE, "Edit remote bindings");
    step(app.press(ADVANCED));
    step(app.wait_for_within(AUTOPLAY, WAIT));
}

/// A remote whose keys report their release (Android's raw key phases)
/// is offered a hold: the long-press time, and a hold in the bindings
/// editor.
#[test]
fn a_hold_is_offered_where_keys_report_their_release() {
    fn here() {}
    let Some(home) = crate::isolated::run_with_env(
        std::any::type_name_of_val(&here),
        &[("IPTV_TEST_DEVICE_SIGNALS", "1")],
    ) else {
        return;
    };
    iptv_app::use_remote_key_phases();
    let mut settings = Settings::new();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    let mut fixture = personalization::launch(
        home,
        settings,
        false,
        |config| config.viewport = Size::new(px(1280.0), px(720.0)),
        |_| {},
    );
    let app = &mut fixture.app;
    device_signals::report(app, "tv", false, true);
    open_settings(app);
    keys_page(app, REMOTE, "Edit remote bindings");
    assert!(shows(app, "text:Long press after"));
    step(app.press("text:Edit remote bindings"));
    step(app.wait_for_within("id:remote-Press", WAIT));
    step(app.wait_for("text:Press: Short"));
    step(app.press("id:remote-Press"));
    step(app.wait_for("text:Press: Hold"));
}
