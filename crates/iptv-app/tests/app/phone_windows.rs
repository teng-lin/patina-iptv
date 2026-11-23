//! A phone (#168), run headless as Android reports one (`Idiom::Phone`):
//! a small and a large phone held upright and on their side, at the
//! default text size and at large ones, the guide drawn at the phone's scale
//! (0.7). Upright, the guide's header stacks
//! a preview as wide as the window over the focused program
//! (`iptv_app::layout::GuideHeader::stacked`); on its side it keeps the
//! preview beside the program. The player's channel list stays inside an
//! upright window, a strip beside it for a tap, and in the preview layout
//! the video stays full screen under it until the window has room for it
//! beside the list. The guide keeps its focus and the channel playing, and
//! the one `tv.player` is never hidden, detached or re-parented.
//!
//! Synthetic data only: the personalization playlist (M3U).

use std::time::Duration;

use crate::ipad_windows::{BAR_CONTROLS, channel_selected, header_fits_as, player, present};
use crate::personalization::step;
use crate::tv_shared_controls::{persistent, preview_fixture_with};
use iptv_app::layout::{LIST_BESIDE_MIN, PREVIEW_MARGIN};
use iptv_settings::Value;
use nsh_core::platform::{Idiom, Os};

const WAIT: Duration = Duration::from_secs(5);
/// A small phone (360 × 792 points, a 1080 × 2376 screen at 480 dpi) and a
/// large one (412 × 915), upright.
const UPRIGHT: [(f32, f32); 2] = [(360.0, 792.0), (412.0, 915.0)];
/// The same phones on their side.
const SIDEWAYS: [(f32, f32); 2] = [(792.0, 360.0), (915.0, 412.0)];
/// Android's font scale: the default, a common user setting, and two
/// large ones.
const TEXT: [f32; 4] = [1.0, 1.15, 1.5, 2.0];

/// The header a phone's window gets: the guide at the phone's scale.
fn policy(size: (f32, f32), text: f32) -> iptv_app::layout::GuideHeader {
    iptv_app::phone_guide_header(size.0, size.1, text)
}

fn phone(env: &mut nsh_core::platform::Environment) {
    env.idiom = Idiom::Phone;
    env.os = Os::Android;
}

/// The bounds of the first shown view whose test id is `id`.
fn bounds(app: &crate::headless::Headless, id: &str) -> Option<[f32; 4]> {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|n| !n.hidden && n.test_id.as_deref() == Some(id))
        .map(|n| {
            let b = n.bounds;
            [b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0]
        })
}

fn video(app: &crate::headless::Headless, surface: nsh_core::NodeId) -> [f32; 4] {
    let node = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.id == surface)
        .expect("the video laid out");
    let b = node.bounds;
    [b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0]
}

#[test]
fn phone_guide_stacks_upright_and_keeps_the_preview_beside_on_its_side() {
    let Some(home) = isolated!() else { return };
    // No autoplay: moving the focus must not tune, so any open is a fault.
    let (mut fixture, lifetime) = preview_fixture_with(home, |settings| {
        settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    });
    let app = &mut fixture.app;
    step(app.set_environment("phone", phone));
    present(app, SIDEWAYS[1], 1.0);
    let surface = player(app);
    // Play B in the preview, then focus C.
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_until("B plays in the preview", WAIT, |_| {
        iptv_app::player_has_source(0)
    }));
    step(app.press_key("down"));
    assert!(channel_selected(app, "Local C"));
    let opens = iptv_app::player_opens(0);

    // At the phone's guide scale every size keeps the program's title and
    // times whole, the largest text on its side too.
    for text in TEXT {
        for size in UPRIGHT.into_iter().chain(SIDEWAYS) {
            present(app, size, text);
            let what = format!("{size:?} at text {text:.2}");
            assert!(channel_selected(app, "Local C"), "focus kept: {what}");
            assert!(iptv_app::player_has_source(0), "B plays on: {what}");
            assert_eq!(iptv_app::player_opens(0), opens, "no open: {what}");
            header_fits_as(app, surface, size, text, policy(size, text));
            persistent(app, &lifetime, surface);
            let want = policy(size, text);
            if want.stacked() {
                let v = video(app, surface);
                assert_eq!(
                    v[2],
                    (size.0 - 2.0 * PREVIEW_MARGIN).floor(),
                    "the preview spans the window: {what}"
                );
                let details = bounds(app, "guide-header-details").expect("details");
                assert!(
                    details[1] >= v[1] + v[3],
                    "the program {details:?} under the preview {v:?}: {what}"
                );
            }
        }
    }
    // Upright at the default text and the common 1.15 both stack; on its
    // side a phone never does.
    for text in [1.0, 1.15] {
        for size in UPRIGHT {
            assert!(policy(size, text).stacked(), "{size:?} at {text}");
        }
        for size in SIDEWAYS {
            assert!(!policy(size, text).stacked(), "{size:?} at {text}");
        }
    }
}

#[test]
fn phone_channel_list_stays_inside_an_upright_window() {
    let Some(home) = isolated!() else { return };
    // The preview layout, the one that puts the video beside the list.
    let (mut fixture, lifetime) = preview_fixture_with(home, |settings| {
        settings.set("guide.autoplay", Value::Bool(false)).unwrap();
        settings
            .set("guide.list_browse_mode", Value::choice("preview"))
            .unwrap();
    });
    let app = &mut fixture.app;
    step(app.set_environment("phone", phone));
    present(app, UPRIGHT[1], 1.0);
    let surface = player(app);
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_until("B plays", WAIT, |_| iptv_app::player_has_source(0)));
    // Full screen (the video fills the window), then the channel list
    // over it.
    step(app.press("id:guide-preview"));
    let full = UPRIGHT[1].0;
    step(app.wait_until("full screen", WAIT, |s| {
        s.layout_nodes()
            .iter()
            .any(|n| n.id == surface && n.bounds.size.width.0 >= full)
    }));
    step(app.press_key("left"));
    step(app.wait_for("id:channel-list"));
    let opens = iptv_app::player_opens(0);

    for size in UPRIGHT.into_iter().chain(SIDEWAYS) {
        present(app, size, 1.0);
        let (w, h) = size;
        let what = format!("{size:?}");
        let list = bounds(app, "channel-list").expect("the list shown");
        assert!(
            list[0] >= -0.5 && list[0] + list[2] <= w - LIST_BESIDE_MIN + 0.5,
            "the list {list:?} inside the window, a strip beside it: {what}"
        );
        let beside = bounds(app, "channel-list-beside").expect("beside");
        assert!(
            beside[2] >= LIST_BESIDE_MIN - 0.5,
            "room to tap beside {beside:?}: {what}"
        );
        let v = video(app, surface);
        if w < 500.0 {
            assert_eq!(v, [0.0, 0.0, w, h], "the video stays full screen: {what}");
        } else {
            assert!(
                v[0] >= list[0] + list[2] && v[0] + v[2] <= w + 0.5,
                "the video {v:?} beside the list {list:?}: {what}"
            );
        }
        assert_eq!(iptv_app::player_opens(0), opens, "no open: {what}");
        persistent(app, &lifetime, surface);
    }
    // A tap on the strip beside the list closes it, upright too. (The
    // layer stays mounted, unseen, while closed: on its side again, a list
    // still open would put the video beside it.)
    present(app, UPRIGHT[0], 1.0);
    step(app.press("id:channel-list-beside"));
    app.settle();
    present(app, SIDEWAYS[1], 1.0);
    assert_eq!(
        video(app, surface),
        [0.0, 0.0, SIDEWAYS[1].0, SIDEWAYS[1].1],
        "the list closed: the video full screen on its side"
    );
    assert_eq!(iptv_app::player_opens(0), opens);
    persistent(app, &lifetime, surface);
}

#[test]
fn phone_play_bar_keeps_transport_centered_on_its_side() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime) = preview_fixture_with(home, |settings| {
        settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    });
    let app = &mut fixture.app;
    step(app.set_environment("phone", phone));
    present(app, SIDEWAYS[1], 1.0);
    let surface = player(app);
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_until("B plays", WAIT, |_| iptv_app::player_has_source(0)));
    step(app.press("id:guide-preview"));
    let full = SIDEWAYS[1].0;
    step(app.wait_until("full screen", WAIT, |s| {
        s.layout_nodes()
            .iter()
            .any(|n| n.id == surface && n.bounds.size.width.0 >= full)
    }));
    // The smallest phones on their side (740 points), the small phone and
    // the large one, at the default text and the common 1.15.
    for size in [(740.0, 360.0), SIDEWAYS[0], SIDEWAYS[1]] {
        for text in [1.0, 1.15] {
            present(app, size, text);
            if bounds(app, "play-bar-back").is_none() {
                step(app.press("id:video"));
            }
            step(app.wait_for("id:play-bar-back"));
            let what = format!("{size:?} at text {text}");
            let rows: Vec<[f32; 4]> = BAR_CONTROLS
                .iter()
                .filter_map(|id| bounds(app, id))
                .collect();
            assert_eq!(rows.len(), BAR_CONTROLS.len(), "the bar's controls: {what}");
            let pause = bounds(app, "play-bar-play-pause").expect("play/pause");
            assert!(
                (pause[0] + pause[2] / 2.0 - size.0 / 2.0).abs() <= 0.5,
                "the transport is centered: {pause:?}, {what}"
            );
            let top = rows.iter().map(|b| b[1]).fold(f32::MAX, f32::min);
            for b in &rows {
                assert!(
                    b[0] >= -0.5
                        && b[0] + b[2] <= size.0 + 0.5
                        && b[1] >= -0.5
                        && b[1] + b[3] <= size.1 + 0.5,
                    "{b:?} inside: {what}"
                );
                assert!(b[2] >= 44.0 - 0.5, "a 44-point target {b:?}: {what}");
            }
            // The program title sits above the transport and auxiliary actions.
            let title = bounds(app, "bar-title").expect("the title line");
            assert!(title[1] + title[3] <= top + 0.5, "{title:?} above: {what}");
            assert!(
                title[0] + title[2] <= size.0 + 0.5,
                "{title:?} inside: {what}"
            );
            let timeline = bounds(app, "play-bar-time-line").expect("the bottom timeline");
            assert!(
                rows.iter()
                    .all(|control| control[1] + control[3] <= timeline[1] + 0.5),
                "the timeline is below all controls: {timeline:?}, {what}"
            );
            assert!(
                timeline[1] + timeline[3] <= size.1 + 0.5,
                "{timeline:?} inside: {what}"
            );
            persistent(app, &lifetime, surface);
        }
    }
}

/// A foldable phone unfolded becomes a tablet (Android derives the idiom
/// from the window's smallest width) and folded a phone again: the guide's
/// rows take each one's size as the window changes.
#[test]
fn an_unfolded_phone_takes_the_tablets_sizes() {
    let Some(home) = isolated!() else { return };
    let (mut fixture, lifetime) = preview_fixture_with(home, |settings| {
        settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    });
    let app = &mut fixture.app;
    step(app.set_environment("phone", phone));
    present(app, SIDEWAYS[1], 1.0);
    let surface = player(app);
    let row = |app: &crate::headless::Headless| bounds(app, "guide-row").expect("a row")[3];
    assert_eq!(row(app), 35.0, "a phone's rows");
    step(app.set_environment("unfolded", |e| e.idiom = Idiom::Pad));
    present(app, (1280.0, 800.0), 1.0);
    assert_eq!(row(app), 50.0, "the tablet's rows once unfolded");
    step(app.set_environment("folded", phone));
    present(app, SIDEWAYS[1], 1.0);
    assert_eq!(row(app), 35.0, "the phone's rows again");

    // Full screen with the bar up: its buttons follow too (the bar is made
    // again when the screen turns from a phone's into a tablet's).
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_until("B plays", WAIT, |_| iptv_app::player_has_source(0)));
    step(app.press("id:guide-preview"));
    let side = |app: &mut crate::headless::Headless| {
        if bounds(app, "play-bar-back").is_none() {
            step(app.press("id:video"));
        }
        step(app.wait_for("id:play-bar-mute"));
        bounds(app, "play-bar-mute").expect("mute")[2]
    };
    assert_eq!(side(app), 44.0, "a phone's buttons");
    step(app.set_environment("unfolded", |e| e.idiom = Idiom::Pad));
    present(app, (1280.0, 800.0), 1.0);
    assert_eq!(side(app), 48.0, "the tablet's buttons once unfolded");
    step(app.set_environment("folded", phone));
    present(app, SIDEWAYS[1], 1.0);
    assert_eq!(side(app), 44.0, "the phone's buttons again");
    persistent(app, &lifetime, surface);
}
