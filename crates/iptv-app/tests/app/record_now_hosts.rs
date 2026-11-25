//! Record now and the Recordings library per host (#275), headless, with
//! the platform simulated (`IPTV_TEST_PLATFORM`, `device.rs`) and the form
//! factor reported as Android's observer reports it (`device_signals.rs`):
//!
//! | Host | Record now | Recordings |
//! | --- | --- | --- |
//! | Android TV | play bar, program details, Record key | always |
//! | Android phone and tablet | under More | once a read finds recordings |
//! | iPad | nowhere | once a read finds recordings |
//! | Tizen | nowhere | never |
//!
//! A desktop keeps Record now in the play bar (`recordings.rs`), and says
//! as it starts when it cannot keep the computer awake.
//!
//! Synthetic data only: one generated channel on a local capture server.

use std::time::Duration;

use crate::headless::Headless;
use crate::personalization::step;
use crate::recordings::{GuideFixture, fixture, local_capture};
use nsh_core::platform::Idiom;
use nsh_core::{EventKind, EventPayload};
use nsh_tv::RemoteKey;

const WAIT: Duration = Duration::from_secs(15);

/// Runs the scenario in a process of its own for `platform`, its observer
/// simulated.
fn home(here: &str, platform: &str) -> Option<crate::isolated::Home> {
    crate::isolated::run_with_env(
        here,
        &[
            ("IPTV_TEST_DEVICE_SIGNALS", "1"),
            ("IPTV_TEST_PLATFORM", platform),
        ],
    )
}

/// The host reports `form`, as Android's activity does.
fn report(app: &mut Headless, form: &str) {
    let node = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("device-signals"))
        .expect("the device observer")
        .id;
    app.engine().control().dispatch_event(
        node,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "device".into(),
            data: serde_json::json!({ "form": form, "keyboard": false, "dpad": form == "tv" }),
        },
    );
    app.settle();
}

/// Whether the rail beside the groups lists `name`; the guide's channels
/// keep the focus afterwards.
fn rail_offers(app: &mut Headless, name: &str) -> bool {
    step(app.open_guide_groups());
    step(app.wait_for("id:navigation-rail >> label:Playlists"));
    let offered = shown(app, &format!("id:navigation-rail >> label:{name}"));
    step(app.press_key("right"));
    offered
}

fn shown(app: &Headless, selector: &str) -> bool {
    app.screen().matches(selector).unwrap()
}

/// Channel A full screen, its play bar up.
fn play_bar(app: &mut Headless) {
    step(app.press("text:A"));
    step(app.press("text:A"));
    step(app.wait_for_within("text:▶", WAIT));
    step(app.wait_for("label:Video"));
    show_bar(app);
}

/// The bar, by a tap on the video or, on a TV, OK.
fn show_bar(app: &mut Headless) {
    if shown(app, "id:play-bar-play-pause") {
        return;
    }
    step(app.press("label:Video"));
    if !shown(app, "id:play-bar-play-pause") {
        step(app.press_key("enter"));
    }
    step(app.wait_for("id:play-bar-play-pause"));
}

/// Whether the bar's More lists `name` (left open), or `false` without More.
fn more_lists(app: &mut Headless, name: &str) -> bool {
    if !shown(app, "id:play-bar-more") {
        return false;
    }
    let open = |app: &Headless| {
        app.screen()
            .accessibility_state("id:play-bar-more")
            .is_ok_and(|state| state.expanded == Some(true))
    };
    if !open(app) {
        step(app.press("id:play-bar-more"));
        app.settle();
        assert!(open(app), "More opened");
    }
    shown(app, &format!("id:chooser-items >> text:{name}"))
}

fn active(path: &std::path::Path) -> usize {
    crate::isolated::open_database(path)
        .unwrap()
        .recording_jobs()
        .unwrap()
        .iter()
        .filter(|job| job.status.active())
        .count()
}

fn jobs(path: &std::path::Path) -> usize {
    crate::isolated::open_database(path)
        .unwrap()
        .recording_jobs()
        .unwrap()
        .len()
}

#[test]
fn android_tv_offers_record_now_and_the_library_directly() {
    fn here() {}
    let Some(home) = home(std::any::type_name_of_val(&here), "android") else {
        return;
    };
    let (url, server) = local_capture();
    let (mut app, path, _, _) = fixture(&home, Idiom::Tv, &url, None);
    report(&mut app, "tv");
    // The library shows before it holds anything.
    assert!(rail_offers(&mut app, "Recordings"));
    play_bar(&mut app);
    assert!(shown(&app, "id:play-bar-record"), "Record is in the bar");
    assert!(!more_lists(&mut app, "Record"));
    assert_eq!(jobs(&path), 0);
    drop(app);
    server.join().unwrap();
}

#[test]
fn android_phones_and_tablets_put_record_now_under_more_and_show_the_library_once_it_holds_one() {
    fn here() {}
    let Some(home) = home(std::any::type_name_of_val(&here), "android") else {
        return;
    };
    let (url, server) = local_capture();
    let (mut app, path, _, _) = fixture(&home, Idiom::Phone, &url, None);
    report(&mut app, "phone");
    // Nothing recorded yet: no library.
    assert!(!rail_offers(&mut app, "Recordings"));
    play_bar(&mut app);
    assert!(!shown(&app, "id:play-bar-record"), "not in the row");
    assert!(more_lists(&mut app, "Record"), "under More");
    // Record now from More records the playing channel.
    step(app.press("id:chooser-items >> text:Record"));
    step(app.wait_until("a capture starts", WAIT, |_| jobs(&path) == 1));
    // Stopped from More as well.
    show_bar(&mut app);
    step(app.wait_until("the capture runs", WAIT, |_| active(&path) == 1));
    assert!(more_lists(&mut app, "Stop recording"));
    step(app.press("id:chooser-items >> text:Stop recording"));
    step(app.wait_until("the capture stops", WAIT, |_| active(&path) == 0));
    // A tablet's report keeps it under More.
    report(&mut app, "tablet");
    show_bar(&mut app);
    assert!(!shown(&app, "id:play-bar-record"));
    step(app.wait_until("Record is offered again", WAIT, |screen| {
        screen.matches("id:play-bar-more").unwrap_or(false)
    }));
    let mut listed = false;
    for _ in 0..20 {
        listed = more_lists(&mut app, "Record");
        if listed {
            break;
        }
        // Closed, the bar's next layout follows the stopped capture.
        step(app.press("id:play-bar-more"));
        step(app.wait(Duration::from_millis(200)));
        show_bar(&mut app);
    }
    assert!(listed, "under More on a tablet");
    // More closes on a second press; the bar's ‹ goes back to the guide.
    step(app.press("id:play-bar-more"));
    show_bar(&mut app);
    step(app.press("id:play-bar-back"));
    // The library's next read finds the recording: from then on the rail
    // offers Recordings.
    step(app.wait(Duration::from_millis(1500)));
    assert!(rail_offers(&mut app, "Recordings"));
    drop(app);
    server.join().unwrap();
}

#[test]
fn android_rail_follows_a_form_change_while_open() {
    fn here() {}
    let Some(home) = home(std::any::type_name_of_val(&here), "android") else {
        return;
    };
    let (url, server) = local_capture();
    let (mut app, _, _, _) = fixture(&home, Idiom::Tv, &url, None);
    report(&mut app, "tv");
    step(app.open_guide_groups());
    let recordings = "id:navigation-rail >> label:Recordings";
    step(app.wait_for(recordings));
    report(&mut app, "tablet");
    step(app.wait_for_gone(recordings, WAIT));
    step(app.wait_for("id:navigation-rail >> label:Playlists"));
    report(&mut app, "tv");
    step(app.wait_for(recordings));
    drop(app);
    server.join().unwrap();
}

#[test]
fn the_program_menu_keeps_stop_available_after_a_form_change() {
    fn here() {}
    let Some(home) = home(std::any::type_name_of_val(&here), "android") else {
        return;
    };
    let (url, server) = local_capture();
    let (mut app, path, _, _) = fixture(
        &home,
        Idiom::Desktop,
        &url,
        Some(GuideFixture::Listing("Generated future program")),
    );
    report(&mut app, "tv");
    step(app.wait_for("text:Generated live program"));
    step(app.press_key("m"));
    step(app.press("id:menu-panel >> label:Record"));
    step(app.wait_until("a capture runs", WAIT, |_| active(&path) > 0));
    step(app.press_key("m"));
    let stop = "id:menu-panel >> label:Stop recording";
    step(app.wait_for(stop));
    report(&mut app, "tablet");
    step(app.press(stop));
    step(app.wait_until("the capture stops", WAIT, |_| active(&path) == 0));
    drop(app);
    server.join().unwrap();
}

#[test]
fn an_ipad_offers_no_record_now_and_no_empty_library() {
    fn here() {}
    let Some(home) = home(std::any::type_name_of_val(&here), "ipad") else {
        return;
    };
    let (url, server) = local_capture();
    let (mut app, path, _, _) = fixture(
        &home,
        Idiom::Pad,
        &url,
        Some(GuideFixture::Listing("Generated future program")),
    );
    report(&mut app, "tablet");
    assert!(!rail_offers(&mut app, "Recordings"));
    step(app.wait_for("text:Generated live program"));
    step(app.press_key("m"));
    step(app.wait_for("id:menu-panel >> label:Details"));
    assert!(!shown(&app, "id:menu-panel >> label:Record"));
    step(app.back());
    no_record_now(&mut app, &path);
    drop(app);
    server.join().unwrap();
}

#[test]
fn a_tizen_tv_offers_neither_record_now_nor_the_library() {
    fn here() {}
    let Some(home) = home(std::any::type_name_of_val(&here), "tizen") else {
        return;
    };
    let (url, server) = local_capture();
    let (mut app, path, _, _) = fixture(&home, Idiom::Tv, &url, None);
    report(&mut app, "tv");
    assert!(!rail_offers(&mut app, "Recordings"));
    no_record_now(&mut app, &path);
    drop(app);
    server.join().unwrap();
}

/// Neither the bar, its More, the program details nor the Record key
/// records.
fn no_record_now(app: &mut Headless, path: &std::path::Path) {
    play_bar(app);
    assert!(!shown(app, "id:play-bar-record"));
    assert!(!more_lists(app, "Record"));
    step(app.press_remote(RemoteKey::Record));
    if shown(app, "id:play-bar-details") {
        step(app.press("id:play-bar-details"));
        step(app.wait_for("id:description-layer"));
        assert!(!shown(app, "id:description-layer >> label:Record"));
        step(app.press_remote(RemoteKey::Record));
    }
    std::thread::sleep(Duration::from_millis(500));
    app.settle();
    assert_eq!(jobs(path), 0, "nothing recorded");
}

#[test]
fn a_desktop_that_cannot_stay_awake_says_so_as_record_now_starts() {
    fn here() {}
    let Some(home) = crate::isolated::run_with_env(
        std::any::type_name_of_val(&here),
        &[("IPTV_TEST_KEEP_AWAKE", "fail")],
    ) else {
        return;
    };
    let (url, server) = local_capture();
    let (mut app, path, _, _) = fixture(&home, Idiom::Desktop, &url, None);
    play_bar(&mut app);
    assert!(
        shown(&app, "id:play-bar-record"),
        "a desktop's bar has Record"
    );
    step(app.press("id:play-bar-record"));
    step(app.wait_until("a capture starts", WAIT, |_| jobs(&path) == 1));
    step(app.wait_for_within("text:/could not keep this computer awake/", WAIT));
    step(app.press("id:play-bar-record"));
    drop(app);
    server.join().unwrap();
}
