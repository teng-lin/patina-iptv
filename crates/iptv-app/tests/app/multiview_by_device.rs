//! Multiview by device and window size (#279), run headless with
//! Android's device observer simulated (`device.rs`): a tablet offers two
//! tiles in an expanded window, and a session collapses to the tile being
//! heard when its window goes to split screen, which plays on in the same
//! player with no stream opened; a phone keeps Multiview under More, on
//! its side only and not at the account's limit; a TV at the account's
//! limit shows it disabled and says why.
//!
//! Synthetic data only: the generated desktop fixture of
//! `desktop_workflows`.

use std::time::Duration;

use crate::desktop_workflows::{Fixture, channel, show_controls, start};
use crate::device_signals::report;
use crate::headless::Headless;
use crate::personalization::step;
use nsh_core::{Size, prelude::*};

const PICKER: &str = "id:multiview-picker";
const TILE: &str = "id:multiview-tile";
const BAR_MULTIVIEW: &str = "id:play-bar-multiview";
const MORE: &str = "id:play-bar-more";
const MORE_MULTIVIEW: &str = "id:chooser-items >> text:Multiview";

fn fixture(here: &str) -> Option<Fixture> {
    let home = crate::isolated::run_with_env(here, &[("IPTV_TEST_DEVICE_SIGNALS", "1")])?;
    Some(start(home, |_| {}))
}

fn resize(app: &mut Headless, (width, height): (f32, f32)) {
    step(app.set_viewport(Size::new(px(width), px(height))));
    app.settle();
}

/// Desk 03 full screen.
fn full_screen(app: &mut Headless) {
    crate::browse_fixture::tap_fullscreen(app, &format!("id:guide-ui >> text:{}", channel(3)));
    step(
        app.wait_until("Desk 03 plays", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0)
        }),
    );
}

/// The play bar shown (a tap on the video, if hovering left it hidden).
fn bar(app: &mut Headless) {
    show_controls(app, "id:play-bar-play-pause");
}

/// Whether More lists Multiview (More is opened and closed again).
fn more_lists_multiview(app: &mut Headless) -> bool {
    bar(app);
    if app.screen().count(MORE).unwrap() == 0 {
        return false;
    }
    step(app.press(MORE));
    step(app.wait_for("id:chooser-items"));
    let listed = app.screen().matches(MORE_MULTIVIEW).unwrap();
    step(app.back());
    app.settle();
    listed
}

/// The shown source's account reports the streams it allows at once, as
/// its import does (through the simulated observer's test seam).
fn account_streams(app: &mut Headless, limit: Option<u32>) {
    app.engine().control().dispatch_event(
        crate::device_signals::node(app, "device-signals"),
        nsh_core::EventKind::NativeEvent,
        nsh_core::EventPayload::Native {
            name: "accountStreams".into(),
            data: serde_json::json!({ "limit": limit }),
        },
    );
    app.settle();
}

#[test]
fn a_tablet_collapses_multiview_to_the_heard_tile_in_split_screen() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here)) else {
        return;
    };
    let app = &mut fixture.app;
    let seen = fixture.seen.clone();
    report(app, "tablet", false, false);
    resize(app, (1280.0, 720.0));
    full_screen(app);

    // Expanded: Multiview on the bar; the session holds two tiles, the
    // second taking the tablet's last decoder.
    show_controls(app, "id:app-ui >> label:Multiview");
    step(app.press("id:app-ui >> label:Multiview"));
    let row = format!("{PICKER} >> text:/  {}$/", channel(5));
    step(app.wait_for(&row));
    step(app.press(&row));
    step(app.wait_for_gone(PICKER, Duration::from_secs(2)));
    step(
        app.wait_until("two tiles play", Duration::from_secs(5), |_| {
            (0..2).all(iptv_app::player_has_source)
        }),
    );
    assert_eq!(app.screen().count(TILE).unwrap(), 2);
    let opens = iptv_app::player_opens(0);
    let main = seen.player(0).expect("the full-screen player");
    let second = seen.player(1).expect("the second tile's player");

    // Split screen (a medium window): the grid goes, Desk 03 (heard, on
    // the full-screen player) plays on in that same player, filling the
    // window, with no stream opened; the second tile's player goes.
    resize(app, (600.0, 720.0));
    step(app.wait_for_gone(TILE, Duration::from_secs(2)));
    let nodes = app.screen().layout_nodes();
    let player = nodes
        .iter()
        .find(|n| n.id == main)
        .expect("the same player");
    assert_eq!(
        (player.bounds.size.width.0, player.bounds.size.height.0),
        (600.0, 720.0),
        "the heard tile full screen"
    );
    assert!(
        nodes.iter().all(|n| n.id != second),
        "the other tile's player goes"
    );
    assert!(iptv_app::player_has_source(0), "the heard tile plays on");
    assert_eq!(iptv_app::player_opens(0), opens, "no stream opened");
    // Multiview is left out there, and comes back in the full window.
    bar(app);
    assert!(app.screen().count(BAR_MULTIVIEW).unwrap() == 0);
    assert!(!more_lists_multiview(app));
    resize(app, (1280.0, 720.0));
    bar(app);
    step(app.wait_for(BAR_MULTIVIEW));
    assert_eq!(iptv_app::player_opens(0), opens);
}

#[test]
fn a_phone_offers_multiview_under_more_on_its_side_only() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here)) else {
        return;
    };
    let app = &mut fixture.app;
    report(app, "phone", false, false);
    // A large phone on its side: under More only, however much room the
    // bar has.
    resize(app, (915.0, 412.0));
    full_screen(app);
    bar(app);
    assert_eq!(app.screen().count(BAR_MULTIVIEW).unwrap(), 0);
    assert!(more_lists_multiview(app), "on its side, under More");

    // At the account's connection limit it is left out.
    account_streams(app, Some(1));
    app.settle();
    assert!(!more_lists_multiview(app), "at the account's limit");
    account_streams(app, Some(2));
    app.settle();
    assert!(more_lists_multiview(app), "with a stream to spare");

    // Upright, it is left out.
    resize(app, (412.0, 915.0));
    bar(app);
    step(app.wait_for(MORE));
    assert!(!more_lists_multiview(app), "upright");
    assert_eq!(app.screen().count(BAR_MULTIVIEW).unwrap(), 0);
}

#[test]
fn a_tv_at_the_accounts_limit_shows_multiview_disabled_with_its_reason() {
    fn here() {}
    let Some(mut fixture) = fixture(std::any::type_name_of_val(&here)) else {
        return;
    };
    let app = &mut fixture.app;
    report(app, "tv", false, true);
    resize(app, (1280.0, 720.0));
    full_screen(app);
    account_streams(app, Some(1));
    app.settle();

    // Shown, disabled: a press says why and opens nothing.
    bar(app);
    step(app.wait_for(BAR_MULTIVIEW));
    step(app.press(BAR_MULTIVIEW));
    step(app.wait_for("text:Your provider allows one stream at a time"));
    assert_eq!(app.screen().count(PICKER).unwrap(), 0);
    assert_eq!(app.screen().count(TILE).unwrap(), 0);

    // With a stream to spare it opens.
    account_streams(app, None);
    app.settle();
    bar(app);
    step(app.press(BAR_MULTIVIEW));
    step(app.wait_for(PICKER));
}
