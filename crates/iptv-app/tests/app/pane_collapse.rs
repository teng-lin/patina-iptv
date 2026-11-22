//! Collapsing panes without losing your place (#86): resizing between an
//! expanded window and compact ones keeps what the viewer chose — the
//! focused channel or the preview, the poster browser's title, a show's
//! selected episode — on the same `tv.player`, and sends no playback
//! command. A compact guide header moves its shortcuts under the preview
//! (`iptv_app::layout::Shortcuts`), and the video keeps following the
//! preview's rectangle.
//!
//! Synthetic data only: the personalization playlist, and a generated
//! on-demand catalog on a local mock server.

use std::time::Duration;

use crate::headless::Headless;
use crate::personalization::step;
use crate::support::Server;
use crate::tv_shared_controls::{persistent, preview_fixture_with, selected};
use iptv_providers::{
    HttpResponse, Secret,
    model::{ContentKind, Source, SourceConfig, XtreamCredentials},
};
use iptv_settings::Value;
use nsh_core::{NodeId, Size, ViewKind, element::Root, platform::Idiom, prelude::*};
use nsh_engine::EngineConfig;

const EXPANDED: (f32, f32) = (1920.0, 1080.0);
/// Compact windows: a narrow desktop window and a phone-sized one.
const COMPACT: [(f32, f32); 2] = [(480.0, 800.0), (400.0, 600.0)];

fn sizes() -> impl Iterator<Item = (f32, f32)> {
    [
        COMPACT[0], EXPANDED, COMPACT[1], EXPANDED, COMPACT[0], COMPACT[1],
    ]
    .into_iter()
}

fn resize(app: &mut Headless, (w, h): (f32, f32)) {
    step(app.set_viewport(Size::new(px(w), px(h))));
    app.settle();
}

/// The guide's windows: the expanded and compact ones, a medium window too
/// narrow for three header columns, and short ones where the preview would
/// otherwise vanish or the corner ‹ land on the shortcuts.
const GUIDE: [(f32, f32); 3] = [(560.0, 800.0), (480.0, 240.0), (480.0, 320.0)];

fn guide_sizes() -> impl Iterator<Item = (f32, f32)> {
    sizes().chain(GUIDE).chain([EXPANDED, GUIDE[1], COMPACT[1]])
}

/// Whether the shortcuts go under the preview, by the app's own policy.
fn below((w, h): (f32, f32)) -> bool {
    iptv_app::desktop_guide_header(w, h).shortcuts == iptv_app::layout::Shortcuts::Below
}

fn by_id(id: &'static str) -> impl Fn(&nsh_testing::LayoutNode) -> bool {
    move |n| n.test_id.as_deref() == Some(id)
}

fn player(app: &Headless) -> NodeId {
    let players: Vec<_> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| n.kind == ViewKind::Native)
        .collect();
    assert_eq!(players.len(), 1, "one persistent player surface");
    players[0].id
}

fn bounds(app: &Headless, matches: impl Fn(&nsh_testing::LayoutNode) -> bool) -> [f32; 4] {
    let node = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| !n.hidden && matches(n))
        .expect("laid out");
    let b = node.bounds;
    [b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0]
}

fn channel_selected(app: &Headless, name: &str) -> bool {
    app.screen()
        .accessibility_state(&format!("id:guide-ui >> label:{name}"))
        .is_ok_and(|s| s.selected)
}

/// The video sits exactly under the guide's preview, inside the window and
/// above the guide's rows.
#[track_caller]
fn video_follows_the_preview(app: &Headless, player: NodeId, (w, h): (f32, f32)) {
    let video = bounds(app, |n| n.id == player);
    let preview = bounds(app, by_id("guide-preview"));
    for (v, p) in video.iter().zip(preview) {
        assert!(
            (v - p).abs() < 0.5,
            "video {video:?} under preview {preview:?}"
        );
    }
    let want = iptv_app::desktop_guide_header(w, h).preview;
    assert_eq!(
        (video[2], video[3]),
        want,
        "the policy's preview at {w}x{h}"
    );
    assert!(
        video[3] >= iptv_app::layout::PREVIEW_MIN_H,
        "the preview shrinks, never goes: {video:?}"
    );
    assert!(video[0] + video[2] <= w, "the preview fits the window");
    let rows = bounds(app, |n| n.test_id.as_deref() == Some("guide-timeline"));
    assert!(
        video[1] + video[3] <= rows[1],
        "the preview stays above the rows"
    );
    assert_eq!(rows[2], w, "the rows keep the window's width");
}

/// The compact header's parts stay apart and inside the window: the corner
/// ‹ inside the preview, the shortcuts' row under it, the program's
/// minutes left on screen (its times line wraps rather than clip).
#[track_caller]
fn header_fits(app: &Headless, (w, h): (f32, f32)) {
    let preview = bounds(app, by_id("guide-preview"));
    // A desktop window always shows the ‹ over the preview.
    {
        let back = bounds(app, by_id("guide-corner-back"));
        assert!(
            back[1] + back[3] <= preview[1] + preview[3] + 0.5,
            "‹ {back:?} inside the preview {preview:?} at {w}x{h}"
        );
    }
    if below((w, h)) {
        let row = bounds(app, by_id("guide-header-shortcuts"));
        assert!(row[1] >= preview[1] + preview[3], "row {row:?} at {w}x{h}");
        let rows = bounds(app, by_id("guide-timeline"));
        assert!(
            row[1] + row[3] <= rows[1] + 0.5,
            "row {row:?} over the rows"
        );
    }
    let left = bounds(app, |n| {
        n.text
            .as_ref()
            .is_some_and(|t| t.text.text.ends_with(" min left"))
    });
    assert!(
        left[0] + left[2] <= w + 0.5,
        "minutes left {left:?} at {w}x{h}"
    );
}

#[test]
fn resizing_the_guide_keeps_the_focused_channel_and_the_playing_preview() {
    let Some(home) = isolated!() else { return };
    // No autoplay: moving the focus must not tune, so any open is a fault.
    let (mut fixture, lifetime) = preview_fixture_with(home, |settings| {
        settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    });
    let app = &mut fixture.app;
    // A desktop window: the one a viewer resizes freely.
    step(app.set_environment("resizable desktop window", |env| {
        env.idiom = Idiom::Desktop;
        env.os = nsh_core::platform::Os::Linux;
    }));
    let player = player(app);
    // Play B in the preview, then focus C's program.
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(
        app.wait_until("B plays in the preview", Duration::from_secs(5), |_| {
            iptv_app::player_has_source(0)
        }),
    );
    step(app.press_key("down"));
    assert!(channel_selected(app, "Local C"));
    let opens = iptv_app::player_opens(0);

    for size in guide_sizes() {
        resize(app, size);
        assert!(channel_selected(app, "Local C"), "focus kept at {size:?}");
        assert!(!channel_selected(app, "Local B"), "one focus at {size:?}");
        assert!(iptv_app::player_has_source(0), "B plays on at {size:?}");
        assert_eq!(iptv_app::player_opens(0), opens, "no open at {size:?}");
        assert_eq!(
            app.screen().matches("id:guide-header-shortcuts").unwrap(),
            below(size),
            "shortcuts below the preview by the policy, {size:?}"
        );
        for shortcut in ["Favorites", "Movies", "Shows", "+ Add playlist"] {
            assert!(
                !app.screen()
                    .matches(&format!("id:guide-header >> text:{shortcut}"))
                    .unwrap(),
                "{shortcut} is absent from the header at {size:?}"
            );
        }
        video_follows_the_preview(app, player, size);
        header_fits(app, size);
        persistent(app, &lifetime, player);
    }

    // Keys still move the focus in a compact window, and the preview's
    // focus survives a resize the same way.
    resize(app, COMPACT[1]);
    step(app.press_key("up"));
    assert!(channel_selected(app, "Local B"));
    step(app.press_key("down"));
    assert!(channel_selected(app, "Local C"));
    step(app.press_key("up"));
    step(app.press_key("up"));
    step(app.press_key("up"));
    assert!(channel_selected(app, "Local C"), "Up wraps to the last row");
    step(app.press_key("tab"));
    step(app.press_key("left"));
    step(app.press_key("up"));
    assert!(selected(app, "guide-preview"), "Up from Search");
    for size in guide_sizes() {
        resize(app, size);
        assert!(selected(app, "guide-preview"), "preview focus at {size:?}");
    }
    step(app.press_key("down"));
    assert!(!selected(app, "guide-preview"));
    step(app.press_key("tab"));
    assert!(channel_selected(app, "Local C"), "back to the same row");
    // A tap on a compact header's shortcut still works.
    step(app.press("id:guide-header-shortcuts >> text:/^☰  /"));
    step(app.wait_for("id:guide-groups >> text:Favorites"));
    assert_eq!(iptv_app::player_opens(0), opens);
    persistent(app, &lifetime, player);
}

/// A generated movie and show on a local Xtream mock.
pub(crate) fn library(home: &crate::isolated::Home) -> Server {
    let server = Server::start(&[(
        "/player_api.php",
        r#"{
  "info": {"name": "Generated Voyage", "plot": "Generated series plot"},
  "episodes": {
    "1": [
      {"id": 101, "episode_num": 1, "title": "Generated First", "container_extension": "mp4"},
      {"id": 102, "episode_num": 2, "title": "Generated Second", "container_extension": "mp4"}
    ]
  }
}"#
        .into(),
    )]);
    let root = crate::isolated::data_dir(home.path());
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = iptv_settings::Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let mut store = crate::isolated::open_database(root.join("iptv.sqlite")).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated on-demand playlist".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: server.base.clone(),
                username: Secret::new("generated-user"),
                password: Secret::new("generated-pass"),
                user_agent: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1 group-title=\"Generated group\",Generated live\nhttps://example.test/live.ts\n",
    );
    let movies = iptv_providers::xtream::parse_vod_streams(&HttpResponse::ok(
        r#"[{"stream_id": 10, "name": "Generated Aurora", "container_extension": "mp4"},
            {"stream_id": 11, "name": "Generated Brook", "container_extension": "mp4"}]"#,
    ))
    .unwrap();
    let series = iptv_providers::xtream::parse_series_list(&HttpResponse::ok(
        r#"[{"series_id":19,"name":"Generated Voyage","category_id":"8"}]"#,
    ))
    .unwrap();
    let categories = iptv_providers::xtream::parse_categories(
        &HttpResponse::ok(r#"[{"category_id":"8","category_name":"Generated serials"}]"#),
        ContentKind::Series,
    )
    .unwrap();
    let mut groups = parsed.groups;
    groups.extend(categories.items);
    store
        .replace_catalog(
            source,
            &groups,
            &parsed.channels.items,
            &movies.items,
            &series.items,
        )
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
    server
}

fn is_selected(app: &Headless, selector: &str) -> bool {
    app.screen()
        .accessibility_state(selector)
        .is_ok_and(|s| s.selected)
}

#[test]
fn resizing_movies_and_shows_keeps_the_title_and_the_episode() {
    let Some(home) = isolated!() else { return };
    let _server = library(&home);
    let mut config = EngineConfig {
        viewport: Size::new(px(EXPANDED.0), px(EXPANDED.1)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.idiom = Idiom::Tv;
    let mut app = Headless::start(config).unwrap();
    step(app.wait_for_within("text:Generated group", Duration::from_secs(20)));
    let surface = player(&app);
    let opens = iptv_app::player_opens(0);

    // The poster browser: the second title focused by keys.
    crate::browse_fixture::open_rail_item(&mut app, "Movies");
    step(app.wait_for("id:vod-title >> nth=1"));
    step(app.press_key("right"));
    step(app.press_key("right"));
    step(app.wait_for("id:vod-focused-metadata >> text:Generated Brook"));
    let brook = "label:/^Generated Brook ·/";
    assert!(is_selected(&app, brook));
    for size in sizes() {
        resize(&mut app, size);
        assert!(is_selected(&app, brook), "title kept at {size:?}");
        assert!(
            !is_selected(&app, "label:/^Generated Aurora ·/"),
            "one title at {size:?}"
        );
        step(app.wait_for("id:vod-focused-metadata >> text:Generated Brook"));
        assert_eq!(player(&app), surface);
        assert_eq!(iptv_app::player_opens(0), opens, "no open at {size:?}");
    }
    // Keys still browse in a compact window.
    resize(&mut app, COMPACT[1]);
    step(app.press_key("left"));
    step(app.wait_for("id:vod-focused-metadata >> text:Generated Aurora"));
    // The remote's Back leaves the browser (no Close button on a TV).
    for _ in 0..4 {
        if !app.screen().matches("id:vod-screen").unwrap() {
            break;
        }
        step(app.back());
    }
    step(app.wait_for_gone("id:vod-screen", Duration::from_secs(2)));

    // A show's details: the second episode selected by keys.
    resize(&mut app, EXPANDED);
    crate::browse_fixture::open_rail_item(&mut app, "Shows");
    step(app.wait_for("id:vod-screen"));
    step(app.press("label:/^Generated Voyage ·/"));
    step(app.wait_for("text:/Generated First/"));
    step(app.press_key("down"));
    step(app.press_key("down"));
    step(app.press_key("right"));
    let second = "label:/^2 · Generated Second/";
    let first = "label:/^1 · Generated First/";
    assert!(is_selected(&app, second));
    for size in sizes() {
        resize(&mut app, size);
        assert!(is_selected(&app, second), "episode kept at {size:?}");
        assert!(!is_selected(&app, first), "one episode at {size:?}");
        assert!(app.screen().matches("text:Generated Voyage").unwrap());
        assert_eq!(player(&app), surface);
        assert_eq!(iptv_app::player_opens(0), opens, "no open at {size:?}");
    }
    // Keys still move between episodes in a compact window, and the
    // details stay open: Back closes them, not the resize.
    resize(&mut app, COMPACT[1]);
    step(app.press_key("left"));
    assert!(is_selected(&app, first));
    step(app.press_key("right"));
    assert!(is_selected(&app, second));
    assert_eq!(iptv_app::player_opens(0), opens);
    assert_eq!(player(&app), surface);
}
