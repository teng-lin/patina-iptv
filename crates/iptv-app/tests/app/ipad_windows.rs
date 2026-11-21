//! iPad portrait, Split View, Slide Over and Stage Manager windows (#138),
//! run headless on an iPad (`Idiom::Pad`, `Os::Ios`). An iPad Air 11-inch
//! turned either way, and the windows multitasking gives the app beside
//! another (about 320 to 700 points wide), at the default text size and at
//! large Dynamic Type sizes: the guide keeps its focused channel and the
//! channel playing in its preview, the header's shortcuts move under the
//! preview by `iptv_core::layout::GuideHeader`'s policy and stay inside
//! the window; full-screen video keeps its progress and the play bar's
//! primary controls; a show's details keep the selected episode; the
//! compact empty Movies and Shows and Search's field stay inside the
//! window. Nothing opens a stream, and the one `tv.player` is never
//! hidden, detached or re-parented. Another app taking the focus beside
//! Patina (the scene inactive but visible) changes nothing; after the
//! return from the background the app admits the native player's request
//! once (`docs/interaction.md`; the native stop itself is checked on the
//! simulator). A chooser keeps clear of the wrapped play bar, and the
//! guide's ‹ of an iPad window's controls.
//!
//! Synthetic data only: the personalization playlist (M3U), and a
//! generated on-demand catalogue on a local mock server.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::headless::Headless;
use crate::personalization::step;
use crate::tv_shared_controls::{Lifetime, persistent, preview_fixture_with};
use iptv_core::layout::{Class, GuideHeader, Shortcuts};
use iptv_settings::{Settings, Value};
use nsh_core::platform::{Idiom, Lifecycle, Os};
use nsh_core::{
    EventKind, EventPayload, MountItem, NodeId, Size, ViewKind, element::Root, prelude::*,
};
use nsh_engine::EngineConfig;
use serde_json::{Value as Json, json};

const WAIT: Duration = Duration::from_secs(5);
/// The play bar's buttons (`play_bar::control_id`): its panel's rows.
pub(crate) const BAR_CONTROLS: [&str; 12] = [
    "play-bar-mute",
    "play-bar-rewind",
    "play-bar-play-pause",
    "play-bar-forward",
    "play-bar-record",
    "play-bar-favourite",
    "play-bar-tracks",
    "play-bar-details",
    "play-bar-picture",
    "play-bar-channels",
    "play-bar-multiview",
    "play-bar-guide",
];
/// An iPad Air 11-inch (M4) full screen, in points.
const LANDSCAPE: (f32, f32) = (1180.0, 820.0);
const PORTRAIT: (f32, f32) = (820.0, 1180.0);
/// Portrait, then the multitasking windows: Split View's two-thirds,
/// half and third, a Slide Over or narrowest Split View window, and a
/// Stage Manager window, then landscape again.
const WINDOWS: [(f32, f32); 7] = [
    PORTRAIT,
    (678.0, 820.0),
    (507.0, 820.0),
    (375.0, 820.0),
    (320.0, 1180.0),
    (700.0, 560.0),
    LANDSCAPE,
];
/// Dynamic Type as iOS reports it (body size over 17 points): the default
/// Large, the largest standard size (xxxLarge, 23 pt) and Accessibility
/// Extra Large (AX3, 40 pt).
const TEXT: [f32; 3] = [1.0, 23.0 / 17.0, 40.0 / 17.0];

fn ipad(env: &mut nsh_core::platform::Environment) {
    env.idiom = Idiom::Pad;
    env.os = Os::Ios;
}

pub(crate) fn present(app: &mut Headless, (w, h): (f32, f32), text: f32) {
    step(app.set_viewport(Size::new(px(w), px(h))));
    step(app.set_environment("Dynamic Type", |e| e.font_scale = text));
    app.settle();
}

pub(crate) fn player(app: &Headless) -> NodeId {
    let players: Vec<_> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| n.kind == ViewKind::Native)
        .collect();
    assert_eq!(players.len(), 1, "one persistent player surface");
    players[0].id
}

/// The bounds of the first shown view `matches` picks.
fn find(app: &Headless, matches: impl Fn(&nsh_testing::LayoutNode) -> bool) -> Option<[f32; 4]> {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|n| !n.hidden && matches(n))
        .map(|n| {
            let b = n.bounds;
            [b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0]
        })
}

fn by_id(id: &'static str) -> impl Fn(&nsh_testing::LayoutNode) -> bool {
    move |n| n.test_id.as_deref() == Some(id)
}

fn bounds(app: &Headless, id: &'static str, what: &str) -> [f32; 4] {
    find(app, by_id(id)).unwrap_or_else(|| panic!("{id} shown: {what}"))
}

/// The bounds of what `selector` names, shown.
fn bounds_of(app: &Headless, selector: &str, what: &str) -> [f32; 4] {
    let id = app
        .screen()
        .id(selector)
        .unwrap_or_else(|e| panic!("{selector}: {e}: {what}"));
    find(app, |n| n.id == id).unwrap_or_else(|| panic!("{selector} shown: {what}"))
}

/// A control lies wholly inside the window, with a size.
#[track_caller]
fn inside(b: [f32; 4], (w, h): (f32, f32), what: &str) {
    assert!(
        b[2] > 0.0
            && b[3] > 0.0
            && b[0] >= -0.5
            && b[1] >= -0.5
            && b[0] + b[2] <= w + 0.5
            && b[1] + b[3] <= h + 0.5,
        "{b:?} inside {w}x{h}: {what}"
    );
}

/// The guide header an iPad window gets: the full-size guide (an iPad does
/// not scale it with its window) at the text size `text`.
pub(crate) fn policy((w, h): (f32, f32), text: f32) -> GuideHeader {
    GuideHeader::of(w, h, 1.0, text)
}

pub(crate) fn channel_selected(app: &Headless, name: &str) -> bool {
    app.screen()
        .accessibility_state(&format!("id:guide-ui >> label:{name}"))
        .is_ok_and(|s| s.selected)
}

/// The guide's header in an iPad window: the video under the preview, the
/// corner ‹ inside it, the shortcuts beside or under it by the policy and
/// inside the window, the programme's minutes left on screen.
#[track_caller]
pub(crate) fn header_fits(app: &Headless, player: NodeId, size: (f32, f32), text: f32) {
    header_fits_as(app, player, size, text, policy(size, text));
}

/// [`header_fits`], against the header `want` the window's guide scale
/// gives (a phone draws its guide smaller than an iPad).
#[track_caller]
pub(crate) fn header_fits_as(
    app: &Headless,
    player: NodeId,
    size: (f32, f32),
    text: f32,
    want: GuideHeader,
) {
    let (w, h) = size;
    let what = format!("{w}x{h} at text {text:.2}");
    let video = find(app, |n| n.id == player).expect("the video laid out");
    let preview = bounds(app, "guide-preview", &what);
    for (v, p) in video.iter().zip(preview) {
        assert!(
            (v - p).abs() < 0.5,
            "video {video:?} under the preview {preview:?}: {what}"
        );
    }
    assert_eq!(
        (video[2], video[3]),
        want.preview,
        "the policy's preview: {what}"
    );
    inside(video, size, &format!("the preview, {what}"));
    let back = bounds(app, "guide-corner-back", &what);
    assert!(
        back[1] + back[3] <= preview[1] + preview[3] + 0.5,
        "‹ {back:?} inside the preview {preview:?}: {what}"
    );
    let rows = bounds(app, "guide-timeline", &what);
    assert!(
        video[1] + video[3] <= rows[1],
        "the preview over the rows: {what}"
    );
    assert_eq!(rows[2], w, "the rows keep the window's width: {what}");
    assert_eq!(
        app.screen().matches("id:guide-header-shortcuts").unwrap(),
        want.shortcuts == Shortcuts::Below,
        "the shortcuts {:?} by the policy: {what}",
        want.shortcuts
    );
    if want.shortcuts == Shortcuts::Below {
        // The row scrolls sideways where the window is narrower than it:
        // the row lies inside the window, its first shortcuts start in it.
        let row = bounds(app, "guide-header-shortcuts", &what);
        assert!(row[1] >= preview[1] + preview[3], "row {row:?}: {what}");
        assert!(
            row[1] + row[3] <= rows[1] + 0.5,
            "row {row:?} over the rows: {what}"
        );
        inside(row, size, &format!("the shortcuts' row, {what}"));
        let list = bounds_of(app, "id:guide-header-shortcuts >> text:/^☰  /", &what);
        assert!(
            list[0] >= -0.5 && list[0] < w,
            "the list {list:?} starts in the window: {what}"
        );
        // Movies and Shows on the same row, by scrolling it where needed.
        for shortcut in ["open-movies", "open-shows"] {
            let b = bounds(app, shortcut, &what);
            assert!(
                b[1] >= row[1] - 0.5 && b[1] + b[3] <= row[1] + row[3] + 0.5,
                "{shortcut} {b:?} in the row {row:?}: {what}"
            );
        }
    } else {
        for shortcut in ["open-movies", "open-shows"] {
            inside(
                bounds(app, shortcut, &what),
                size,
                &format!("{shortcut}, {what}"),
            );
        }
    }
    let left = find(app, |n| {
        n.text
            .as_ref()
            .is_some_and(|t| t.text.text.ends_with(" min left"))
    })
    .unwrap_or_else(|| panic!("minutes left shown: {what}"));
    assert!(
        left[0] + left[2] <= w + 0.5,
        "minutes left {left:?}: {what}"
    );
    // The column clips what it has no room for; the header grows for the
    // title and the times, which come first, so the times (and the title
    // above them) are whole inside it, the description giving way.
    let details = bounds(app, "guide-header-details", &what);
    let times = find(app, |n| {
        n.text.as_ref().is_some_and(|t| t.text.text.contains(" – "))
            && n.bounds.origin.x.0 >= details[0] - 0.5
    })
    .unwrap_or_else(|| panic!("the programme's times shown: {what}"));
    for (part, b) in [("times", times), ("minutes left", left)] {
        assert!(
            b[1] >= details[1] - 0.5 && b[1] + b[3] <= details[1] + details[3] + 0.5,
            "{part} {b:?} whole inside the details {details:?}: {what}"
        );
    }
    assert!(
        details[1] + details[3] <= h,
        "the details {details:?} inside the window: {what}"
    );
}

#[test]
fn ipad_guide_keeps_the_focused_channel_and_the_preview_in_every_window() {
    let Some(home) = isolated!() else { return };
    // No autoplay: moving the focus must not tune, so any open is a fault.
    let (mut fixture, lifetime) = preview_fixture_with(home, |settings| {
        settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    });
    let app = &mut fixture.app;
    step(app.set_environment("iPad", ipad));
    present(app, LANDSCAPE, 1.0);
    let player = player(app);
    // Play B in the preview by a hardware keyboard, then focus C.
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_until("B plays in the preview", WAIT, |_| {
        iptv_app::player_has_source(0)
    }));
    step(app.press_key("down"));
    assert!(channel_selected(app, "Local C"));
    let opens = iptv_app::player_opens(0);

    for text in TEXT {
        for size in WINDOWS {
            present(app, size, text);
            let what = format!("{size:?} at text {text:.2}");
            assert!(channel_selected(app, "Local C"), "focus kept: {what}");
            assert!(!channel_selected(app, "Local B"), "one focus: {what}");
            assert!(iptv_app::player_has_source(0), "B plays on: {what}");
            assert_eq!(iptv_app::player_opens(0), opens, "no open: {what}");
            header_fits(app, player, size, text);
            persistent(app, &lifetime, player);
        }
    }

    // An iPad window's controls at its top left (a leading safe-area
    // inset): the guide's ‹ moves clear of them, still over the preview.
    present(app, LANDSCAPE, 1.0);
    for inset in [65.0, 0.0] {
        step(app.set_environment("window controls", |e| e.safe_area.left = px(inset)));
        app.settle();
        let back = bounds(app, "guide-corner-back", "window controls");
        let preview = bounds(app, "guide-preview", "window controls");
        assert!(
            back[0] >= inset.max(20.0) - 0.5 && back[0] + back[2] <= preview[0] + preview[2],
            "‹ {back:?} clear of a {inset}-point inset, over the preview {preview:?}"
        );
    }

    // Keys still move the focus in a Split View window (an iPad's keyboard
    // stays in the guide's rows: the preview takes the focus on TVs and
    // desktops only), and a tap on a shortcut under the preview still works.
    present(app, (375.0, 820.0), TEXT[2]);
    step(app.press_key("up"));
    assert!(channel_selected(app, "Local B"));
    step(app.press_key("down"));
    assert!(channel_selected(app, "Local C"));
    present(app, (375.0, 820.0), TEXT[1]);
    step(app.press("id:guide-header-shortcuts >> text:/^☰  /"));
    step(app.wait_for("id:guide-groups >> text:Favourites"));
    assert_eq!(iptv_app::player_opens(0), opens);
    persistent(app, &lifetime, player);
}

// --- Full screen and a show's details -----------------------------------

/// What the app gave its player, and how the player view lived.
#[derive(Default)]
struct Seen {
    life: Lifetime,
    props: std::collections::HashMap<String, Json>,
}

fn observe(app: &mut Headless) -> Arc<Mutex<Seen>> {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    observe_into(app, seen.clone());
    seen
}

fn observe_into(app: &mut Headless, shared: Arc<Mutex<Seen>>) {
    app.observe(move |_, batch| {
        let mut seen = shared.lock().unwrap();
        for item in &batch.items {
            match item {
                MountItem::Component { id, name } if name.as_ref() == "tv.player" => {
                    seen.life.players.push(*id)
                }
                MountItem::ComponentProps { props, .. } => {
                    for (key, value) in props {
                        let value = value.clone().unwrap_or(Json::Null);
                        if key.as_ref() == "source"
                            && let Some(url) =
                                crate::support::upstream_source(&value)["url"].as_str()
                            && seen.life.sources.last().map(String::as_str) != Some(url)
                        {
                            seen.life.sources.push(url.to_owned());
                        }
                        seen.props.insert(key.to_string(), value);
                    }
                }
                MountItem::Insert { child, .. } => {
                    *seen.life.inserts.entry(*child).or_default() += 1
                }
                MountItem::Remove { id } | MountItem::Delete { id } => seen.life.removed.push(*id),
                _ => {}
            }
        }
    });
}

fn sources(seen: &Arc<Mutex<Seen>>) -> Vec<String> {
    seen.lock().unwrap().life.sources.clone()
}

/// The player view was mounted once and never removed, moved or hidden.
#[track_caller]
fn kept(app: &Headless, seen: &Arc<Mutex<Seen>>, surface: NodeId) {
    let seen = seen.lock().unwrap();
    assert_eq!(seen.life.players, [surface], "one tv.player for the run");
    assert!(!seen.life.removed.contains(&surface), "never detached");
    assert_eq!(
        seen.life.inserts.get(&surface),
        Some(&1),
        "never re-parented"
    );
    assert!(
        app.screen()
            .layout_nodes()
            .iter()
            .any(|n| n.id == surface && !n.hidden),
        "never hidden"
    );
}

fn video_width(app: &Headless, surface: NodeId) -> f32 {
    find(app, |n| n.id == surface).expect("the video laid out")[2]
}

/// The bar over full-screen video, shown by a tap on the video when an
/// earlier step left it hidden.
fn show_bar(app: &mut Headless) {
    if find(app, by_id("play-bar-back")).is_none() {
        step(app.press("id:video"));
    }
    step(app.wait_for("id:play-bar-back"));
}

fn ipad_app() -> Headless {
    let mut config = EngineConfig {
        viewport: Size::new(px(LANDSCAPE.0), px(LANDSCAPE.1)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    ipad(&mut config.environment);
    Headless::start(config).unwrap()
}

#[test]
fn ipad_full_screen_and_details_keep_progress_and_episode_in_every_window() {
    let Some(home) = isolated!() else { return };
    let _server = crate::pane_collapse::library(&home);
    let mut app = ipad_app();
    let seen = observe(&mut app);
    step(app.wait_for_within("text:Generated group", Duration::from_secs(30)));
    let surface = player(&app);

    // A movie by touch, at 37 s of 2 min.
    step(app.press("id:open-movies"));
    step(app.wait_for("id:vod-screen"));
    step(app.press("label:/^Generated Aurora ·/"));
    step(app.wait_for("text:Play"));
    step(app.press("text:Play"));
    step(app.wait_for_gone("id:vod-screen", WAIT));
    let movie = sources(&seen);
    assert_eq!(movie.len(), 1, "{movie:?}");
    let token = seen.lock().unwrap().props["source"]["token"].clone();
    app.engine().control().dispatch_event(
        surface,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "timeline".into(),
            data: json!({"token": token, "seekId": 0, "ms": 37_000, "durationMs": 120_000, "seekable": true}),
        },
    );
    app.settle();
    show_bar(&mut app);
    step(app.wait_for("text:0:37"));
    // Every open counts, the same movie's again too: a resize that reopened
    // it would change the count and the source's token.
    let opens = iptv_app::player_opens(0);
    let source_token =
        |seen: &Arc<Mutex<Seen>>| seen.lock().unwrap().props["source"]["token"].clone();

    for text in TEXT {
        for size in WINDOWS {
            present(&mut app, size, text);
            show_bar(&mut app);
            let what = format!("movie at {size:?}, text {text:.2}");
            assert_eq!(sources(&seen), movie, "no stream opened: {what}");
            assert_eq!(iptv_app::player_opens(0), opens, "no open: {what}");
            assert_eq!(source_token(&seen), token, "the same source: {what}");
            assert_eq!(video_width(&app, surface), size.0, "full screen: {what}");
            assert!(app.shows("0:37"), "progress kept: {what}");
            for control in ["play-bar-back", "play-bar-play-pause"] {
                inside(
                    bounds(&app, control, &what),
                    size,
                    &format!("{control}, {what}"),
                );
            }
            // Every button of the bar, the secondary ones too: a narrow
            // window wraps them rather than cut them off at its edge.
            let buttons: Vec<_> = app
                .screen()
                .layout_nodes()
                .into_iter()
                .filter(|n| {
                    !n.hidden
                        && n.test_id
                            .as_deref()
                            .is_some_and(|id| id.starts_with("play-bar-") && id != "play-bar-seek")
                })
                .collect();
            assert!(buttons.len() >= 10, "the bar's buttons: {what}");
            for b in buttons {
                let r = b.bounds;
                inside(
                    [r.origin.x.0, r.origin.y.0, r.size.width.0, r.size.height.0],
                    size,
                    &format!("{:?}, {what}", b.test_id),
                );
            }
            kept(&app, &seen, surface);
        }
    }

    // A chooser over the video in a Split View window keeps clear of the
    // bar, whose buttons wrap onto more rows there: at the largest text it
    // is at its tallest.
    for text in [TEXT[0], TEXT[2]] {
        let size = (375.0, 820.0);
        present(&mut app, size, text);
        show_bar(&mut app);
        let what = format!("the picture size chooser at {size:?}, text {text:.2}");
        step(app.press("id:play-bar-picture"));
        step(app.wait_for("id:chooser-items"));
        app.settle();
        let panel = bounds(&app, "chooser-panel", &what);
        inside(panel, size, &what);
        let bar_top = app
            .screen()
            .layout_nodes()
            .into_iter()
            .filter(|n| {
                !n.hidden
                    && n.test_id
                        .as_deref()
                        .is_some_and(|id| BAR_CONTROLS.contains(&id))
            })
            .map(|n| n.bounds.origin.y.0)
            .fold(f32::INFINITY, f32::min);
        assert!(
            panel[1] + panel[3] <= bar_top + 0.5,
            "chooser {panel:?} above the bar's top {bar_top}: {what}"
        );
        step(app.back());
        // The chooser stays mounted, clear, while closed.
        app.settle();
        assert_eq!(iptv_app::player_opens(0), opens, "no open: {what}");
    }
    present(&mut app, LANDSCAPE, 1.0);
    show_bar(&mut app);
    step(app.press("id:play-bar-back"));
    step(app.wait_until("the guide's preview", WAIT, |s| {
        s.layout_nodes()
            .iter()
            .any(|n| n.kind == ViewKind::Native && n.bounds.size.width.0 < LANDSCAPE.0)
    }));
    assert_eq!(sources(&seen), movie);
    // ‹ from a movie returns to its details (#147); close them and the
    // library.
    step(app.wait_for("text:Play from beginning"));
    for _ in 0..2 {
        step(app.press("id:vod-screen >> label:Close"));
    }
    step(app.wait_for_gone("id:vod-screen", WAIT));
    assert_eq!(sources(&seen), movie);

    // A show's details with its second episode selected by touch.
    step(app.press("id:open-shows"));
    step(app.wait_for("id:vod-screen"));
    step(app.press("label:/^Generated Voyage ·/"));
    step(app.wait_for("text:/Generated First/"));
    let episodes = |app: &Headless| {
        [
            "label:/^1 · Generated First/",
            "label:/^2 · Generated Second/",
        ]
        .map(|e| {
            app.screen()
                .accessibility_state(e)
                .is_ok_and(|s| s.selected)
        })
    };
    let chosen = episodes(&app);
    assert_eq!(chosen.iter().filter(|s| **s).count(), 1, "{chosen:?}");
    for text in TEXT {
        for size in WINDOWS {
            present(&mut app, size, text);
            let what = format!("details at {size:?}, text {text:.2}");
            assert_eq!(episodes(&app), chosen, "episode kept: {what}");
            assert!(app.shows("Generated Voyage"), "details open: {what}");
            assert_eq!(sources(&seen), movie, "no stream opened: {what}");
            let close = bounds_of(&app, "id:vod-screen >> label:Close", &what);
            inside(close, size, &format!("Close, {what}"));
            kept(&app, &seen, surface);
        }
    }
    present(&mut app, LANDSCAPE, 1.0);
    for _ in 0..2 {
        step(app.press("id:vod-screen >> label:Close"));
    }
    step(app.wait_for_gone("id:vod-screen", WAIT));
    assert_eq!(sources(&seen), movie);
    kept(&app, &seen, surface);
}

// --- Empty Movies and Search in narrow windows ---------------------------

const CLOSE: &str = "id:vod-screen >> label:Close";
const M3U_ONLY: &str = "Movies and shows come from Xtream Codes and Stalker playlists. M3U playlists have live channels only.";

#[test]
fn ipad_empty_movies_and_search_fit_split_view_at_large_text() {
    let Some(home) = isolated!() else { return };
    let mut settings = Settings::new();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    let mut fixture = crate::personalization::launch(
        home,
        settings,
        true,
        |config| {
            config.viewport = Size::new(px(LANDSCAPE.0), px(LANDSCAPE.1));
            ipad(&mut config.environment);
        },
        |_| {},
    );
    let app = &mut fixture.app;
    step(app.press("id:open-movies"));
    step(app.wait_for(&format!("text:{M3U_ONLY}")));
    for text in TEXT {
        for size in WINDOWS {
            present(app, size, text);
            let what = format!("empty Movies at {size:?}, text {text:.2}");
            step(app.wait_for(&format!("text:{M3U_ONLY}")));
            if Class::of(size.0, text) == Class::Compact {
                assert_eq!(
                    app.screen().count("id:vod-category").unwrap(),
                    0,
                    "the categories give way in a compact window: {what}"
                );
            }
            let message = find(app, |n| {
                n.text.as_ref().is_some_and(|t| t.text.text == M3U_ONLY)
            })
            .unwrap();
            inside(message, size, &format!("the message, {what}"));
            inside(
                bounds(app, "vod-empty-action", &what),
                size,
                &format!("Add playlist, {what}"),
            );
            // A way out by touch (an iPad has no Back): the rail's Close,
            // or where a compact window closed the categories and a wider
            // one keeps them closed, the category heading that opens them
            // with the rail.
            let out = if app.screen().matches(CLOSE).unwrap() {
                CLOSE
            } else {
                "id:vod-screen >> label:All movies"
            };
            inside(bounds_of(app, out, &what), size, &format!("{out}, {what}"));
        }
    }
    present(app, LANDSCAPE, 1.0);
    if !app.screen().matches(CLOSE).unwrap() {
        step(app.press("id:vod-screen >> label:All movies"));
    }
    step(app.press(CLOSE));
    step(app.wait_for_gone("id:vod-screen", WAIT));

    // Search: the field and the results inside every window.
    step(app.press("text:/^☰  /"));
    step(app.press("text:Search"));
    step(app.wait_for("id:search-field"));
    step(app.type_text("id:search-field", "Local"));
    step(app.wait_for("text:/Channels · /"));
    for text in TEXT {
        for size in WINDOWS {
            present(app, size, text);
            let what = format!("Search at {size:?}, text {text:.2}");
            inside(
                bounds(app, "search-field", &what),
                size,
                &format!("the field, {what}"),
            );
            assert_eq!(
                app.screen().input_text("id:search-field").unwrap(),
                Some("Local"),
                "the query kept: {what}"
            );
            let results = bounds_of(app, "text:/Channels · /", &what);
            assert!(
                results[0] >= -0.5 && results[0] < size.0 && results[1] < size.1,
                "the results start inside the window {results:?}: {what}"
            );
        }
    }
}

// --- Multitasking: another app beside, and leaving the screen ------------

fn reauthorize(app: &mut Headless, surface: NodeId, seen: &Arc<Mutex<Seen>>) {
    let token = seen.lock().unwrap().props["source"]["token"].clone();
    app.engine().control().dispatch_event(
        surface,
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "reauthorize".into(),
            data: json!({"token": token, "allowed": true}),
        },
    );
    app.settle();
}

#[test]
fn ipad_another_app_beside_changes_nothing_and_a_reauthorized_return_reopens_once() {
    let Some(home) = isolated!() else { return };
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.switch_delay", Value::ms(0)).unwrap();
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let recorder = seen.clone();
    let mut fixture = crate::personalization::launch(
        home,
        settings,
        true,
        |config| {
            config.viewport = Size::new(px(LANDSCAPE.0), px(LANDSCAPE.1));
            ipad(&mut config.environment);
        },
        move |app| observe_into(app, recorder),
    );
    let app = &mut fixture.app;
    // Two-thirds of a Split View.
    present(app, (678.0, 820.0), 1.0);
    let surface = player(app);
    step(app.press("id:guide-ui >> label:Local A"));
    let opened = seen.clone();
    step(app.wait_until("the channel plays", WAIT, move |_| {
        !opened.lock().unwrap().life.sources.is_empty()
    }));
    let playing = sources(&seen);
    assert_eq!(playing.len(), 1, "{playing:?}");
    let opens = iptv_app::player_opens(0);
    let paused = |seen: &Arc<Mutex<Seen>>| {
        seen.lock().unwrap().props.get("paused") == Some(&Json::Bool(true))
    };
    assert!(!paused(&seen));

    // Another app beside takes the focus, then Patina gets it back, while
    // the window narrows to a third and widens: playback carries on.
    for (lifecycle, size) in [
        (Lifecycle::Inactive, (375.0, 820.0)),
        (Lifecycle::Active, (375.0, 820.0)),
        (Lifecycle::Inactive, (820.0, 1180.0)),
        (Lifecycle::Active, LANDSCAPE),
    ] {
        step(app.set_environment("multitasking", |e| e.lifecycle = lifecycle));
        present(app, size, 1.0);
        let what = format!("{lifecycle:?} at {size:?}");
        assert!(!paused(&seen), "plays on: {what}");
        assert!(iptv_app::player_has_source(0), "the stream stays: {what}");
        assert_eq!(iptv_app::player_opens(0), opens, "no open: {what}");
        assert_eq!(sources(&seen), playing, "same stream: {what}");
        assert_eq!(player(app), surface);
    }

    // Leaving the screen: the native player stops its stream in the
    // background and asks again on return (`mpv/ios.rs`; not run headless,
    // where no native player runs: the simulator run checks it by the mock
    // server's requests). Here, the app's side: given the player's
    // `reauthorize` after the return, it admits the stream once, on the
    // same player.
    step(app.set_environment("background", |e| e.lifecycle = Lifecycle::Background));
    app.settle();
    step(app.set_environment("foreground", |e| e.lifecycle = Lifecycle::Active));
    reauthorize(app, surface, &seen);
    step(app.wait_until("reopened once", WAIT, |_| {
        iptv_app::player_opens(0) == opens + 1
    }));
    step(app.wait(Duration::from_millis(200)));
    assert_eq!(iptv_app::player_opens(0), opens + 1, "one reopen");
    assert_eq!(sources(&seen), playing, "the same channel");
    kept(app, &seen, surface);
}
