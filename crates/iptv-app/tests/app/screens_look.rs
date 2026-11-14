//! The secondary screens in the lists' type, headless: the wizard's
//! options, a settings row's label and the search field use the list item
//! size (20 sp) in Roboto, titles the screen title size (22 sp), headings
//! the section size (17 sp), all scaled in a smaller desktop window
//! (`scale.rs`, `ui_kit.rs`).
//!
//! Synthetic data only: a playlist picked from a file.

use std::time::Duration;

use crate::headless::Headless;
use nsh_core::picker::PickedFile;
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(30);
const ROBOTO: &str = "Roboto, sans-serif";

#[test]
fn secondary_screens_use_the_shared_type() {
    let Some(_home) = isolated!() else { return };
    let playlist = "#EXTM3U\n\
         #EXTINF:-1 group-title=\"News\",Test Channel One\nhttp://stream.example.test/1.m3u8\n\
         #EXTINF:-1 group-title=\"News\",Test Channel Two\nhttp://stream.example.test/2.m3u8\n";
    let config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    let mut app = Headless::start(config).unwrap();
    app.on_pick(move |_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(playlist.as_bytes().to_vec()),
        )]
    });
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));

    // The wizard: its title, its options as two-line rows, a form row.
    step(app.wait_for("text:No playlists yet"));
    assert!(has(&mut app, "No playlists yet", 22.0));
    step(app.press("text:Add playlist"));
    step(app.wait_for("text:Playlist file"));
    assert!(has(&mut app, "Playlist file", 20.0), "a wizard option");
    assert!(has(&mut app, "An M3U file saved on this device", 16.0));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    assert!(has(&mut app, "Choose a file…", 20.0), "a form row's value");
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:synthetic.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:Test Channel Two", IMPORT));

    // The two-tone marker retains the original total width; borders must
    // not silently grow its outer box.
    let marker = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.test_id.as_deref() == Some("guide-now-line"))
        .expect("the guide now line");
    assert!((marker.bounds.size.width.0 - 2.0).abs() < 0.01);

    // Search, from the rail: the field at the item size, the heading.
    open_rail(&mut app);
    step(app.press("text:Search >> nth=0"));
    step(app.wait_for("text:No recent searches yet"));
    assert!(has(&mut app, "No recent searches yet", 17.0), "a heading");
    for (w, h, scale) in [(1280.0, 720.0, 0.875), (1920.0, 1080.0, 1.0)] {
        step(app.set_viewport(Size::new(px(w), px(h))));
        let field = field_height(&mut app);
        // The headless engine's line is 1.2 × the size; padding 13 dp.
        let want = (1.2 * 20.0 + 2.0 * 13.0) * scale;
        assert!(
            (field - want).abs() < 0.5,
            "search field {field} at {w}x{h}"
        );
    }
    step(app.press("text:Close"));
    step(app.wait_for_gone("text:No recent searches yet", Duration::from_secs(2)));

    // Settings: the title, a row's label and its value.
    open_rail(&mut app);
    step(app.press("text:Settings >> nth=0"));
    let label = "Play the last channel on start";
    step(app.wait_for(&format!("text:{label}")));
    for (w, h, scale) in [(1920.0, 1080.0, 1.0), (1280.0, 720.0, 0.875)] {
        step(app.set_viewport(Size::new(px(w), px(h))));
        assert!(has(&mut app, label, 20.0 * scale), "settings row {w}x{h}");
        assert!(has(&mut app, "Settings", 22.0 * scale), "title {w}x{h}");
        assert!(has(&mut app, "General", 20.0 * scale), "a page {w}x{h}");
    }

    // A button in a row reads on the row's light pill: the wizard's link
    // form by keys (the focus shows), its link row focused: that row's
    // Paste in the pill's text colour, another row's in the list's.
    step(app.press("text:Close"));
    step(app.wait_for_gone("id:settings-screen", Duration::from_secs(2)));
    step(app.press("text:+ Add playlist"));
    step(app.wait_for("text:Playlist link"));
    step(app.press_key("down"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for("text:Paste"));
    let paste = |app: &mut Headless, nth: usize| {
        app.screen()
            .span_style(&format!("text:Paste >> nth={nth}"))
            .unwrap()
            .color
    };
    let dark = nsh_core::Color((iptv_app::theme::FOCUSED_TEXT << 8) | 0xff);
    let light = nsh_core::Color((iptv_app::theme::LIST_TEXT << 8) | 0xff);
    assert_eq!(paste(&mut app, 0), dark, "Paste on the focused row");
    assert_eq!(paste(&mut app, 1), light, "Paste on another row");
}

/// The groups, then the rail beside them (Left), its labels shown.
fn open_rail(app: &mut Headless) {
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));
    step(app.press("text:☰  All channels"));
    step(app.press_key("left"));
    step(app.press_key("left"));
}

/// Whether a span reading `text` is in Roboto at `size` (to a hundredth).
fn has(app: &mut Headless, text: &str, size: f32) -> bool {
    let mut found = Vec::new();
    while let Ok(style) = app
        .screen()
        .span_style(&format!("text:{text} >> nth={}", found.len()))
    {
        let family = style.font_family.map(|f| f.name().to_string());
        found.push((family, style.font_size.0));
    }
    let ok = found
        .iter()
        .any(|(f, s)| f.as_deref() == Some(ROBOTO) && (s - size).abs() < 0.01);
    if !ok {
        eprintln!("`{text}`: {found:?}, want {size}");
    }
    ok
}

/// The search field's height.
fn field_height(app: &mut Headless) -> f32 {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|n| n.test_id.as_deref() == Some("search-field"))
        .map(|n| n.bounds.size.height.0)
        .expect("the search field")
}
