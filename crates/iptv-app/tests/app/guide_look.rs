//! The guide's look, headless: programme titles are set in the bundled
//! Roboto Condensed, channel names and the rest in Roboto (the app's
//! default family), every list falling back to the system's sans-serif;
//! the sizes are the TiviMate-like ones in a full-screen window and
//! smaller in a 1280 × 720 one (`scale.rs`), following a resize; a name
//! too long for the channel column ends in two dots; rows are 50 dp apart
//! at full size. The lists (the groups column, the player's channel list)
//! name groups and channels in the same type as the guide's channel
//! column, and programmes in its programme type.
//!
//! Synthetic data only: a playlist picked from a file, with a local XMLTV
//! guide.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iptv_epg::format_xmltv_time;
use nsh_core::picker::PickedFile;
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use nsh_testing::Headless;

const IMPORT: Duration = Duration::from_secs(30);
/// The rows of the playlist.
const FILLERS: usize = 80;
const LONG: &str = "Synthetic Channel With A Name Far Too Long For The Column";

fn guide(now: i64) -> String {
    let mut xml = String::from("<?xml version=\"1.0\"?><tv>");
    for (id, title) in [("one", "Quiet Harbour"), ("two", "Golden Engine")] {
        xml.push_str(&format!(
            "<channel id=\"{id}\"><display-name>{id}</display-name></channel>\
             <programme channel=\"{id}\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>",
            format_xmltv_time(now - 1800),
            format_xmltv_time(now + 1800)
        ));
    }
    xml + "</tv>"
}

#[test]
fn the_guide_uses_the_bundled_fonts_and_proportions() {
    let Some(home) = isolated!() else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let mut playlist = format!(
        "#EXTM3U url-tvg=\"{}\"\n\
         #EXTINF:-1 tvg-id=\"one\" group-title=\"News\",Test Channel One\nhttp://stream.example.test/1.m3u8\n\
         #EXTINF:-1 tvg-id=\"two\" group-title=\"News\",{LONG}\nhttp://stream.example.test/2.m3u8\n",
        guide_path.display()
    );
    // Rows to scroll through (no programmes).
    for n in 3..=FILLERS {
        playlist.push_str(&format!(
            "#EXTINF:-1 group-title=\"News\",Filler {n}\nhttp://stream.example.test/{n}.m3u8\n"
        ));
    }
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
    step(app.wait_for("text:No playlists yet"));
    step(app.press("text:Add playlist"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:synthetic.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:Golden Engine", IMPORT));

    let roboto = Some("Roboto, sans-serif".to_owned());
    // Full screen: the full sizes.
    check_sizes(&mut app, 1.0);
    assert_eq!(row_pitch(&mut app), 50.0);
    check_ruler(&mut app);
    // A 1280 × 720 window: 7/8 of them, the rows in whole px.
    step(app.set_viewport(Size::new(px(1280.0), px(720.0))));
    check_sizes(&mut app, 0.875);
    assert_eq!(row_pitch(&mut app), 44.0);
    check_ruler(&mut app);
    // Full screen again.
    step(app.set_viewport(Size::new(px(1920.0), px(1080.0))));
    check_sizes(&mut app, 1.0);
    assert_eq!(row_pitch(&mut app), 50.0);

    // Text without a family of its own takes the app's default (font-sans).
    let default = styles(&mut app, "+ Add playlist");
    assert!(
        default.iter().all(|(f, _, _)| f.is_none() || *f == roboto),
        "default family: {default:?}"
    );

    // The long name is cut with two dots; the short one is whole.
    let cut = app
        .screen()
        .texts()
        .into_iter()
        .find(|t| t.starts_with("Synthetic Channel") && t.ends_with(".."))
        .map(str::to_owned);
    assert!(cut.is_some(), "a cut name: {:?}", app.screen().texts());
    assert!(!cut.unwrap().ends_with("…"));
    assert!(app.screen().matches("text:Test Channel One").unwrap());

    // Far down the list, the focused row stays on screen through resizes,
    // and keys keep it there: the list's row math is the rows' pitch.
    let mut focus = 1;
    for (w, h, pitch) in [
        (1920.0, 1080.0, 50.0),
        (1280.0, 720.0, 44.0),
        (1920.0, 1080.0, 50.0),
        (1280.0, 720.0, 44.0),
    ] {
        step(app.set_viewport(Size::new(px(w), px(h))));
        for _ in 0..17 {
            step(app.press_key("down"));
            focus += 1;
            // Below the header (a third of the window) and the ruler, once
            // the list has followed the key (a later frame under load).
            let shown = |y: f32| y >= h / 3.0 && y + pitch <= h;
            let y = number_y_when(&mut app, focus, shown);
            assert!(y.is_some_and(shown), "row {focus} at {y:?} in {w}x{h}");
        }
        let (above, at) = (number_y(&mut app, focus - 1), number_y(&mut app, focus));
        assert_eq!((at.unwrap() - above.unwrap()).round(), pitch, "{w}x{h}");
    }
    assert!(focus < FILLERS);

    // The lists: the groups column (opened from the list's name) and the
    // player's channel list (mounted while hidden) in the guide's type,
    // full screen and in a smaller window.
    step(app.press("text:☰  All channels"));
    step(app.wait_for("text:News"));
    for (w, h, scale) in [(1920.0, 1080.0, 1.0), (1280.0, 720.0, 0.875)] {
        step(app.set_viewport(Size::new(px(w), px(h))));
        let name = (roboto.clone(), 20.0 * scale, 400);
        // A resize reaches the views on a later frame: wait for it.
        let guide = styles_when(&mut app, "Test Channel One", |g| close_to(g, &name));
        assert!(close_to(&guide, &name), "guide names: {guide:?}");
        // (The header's group line reads "News" too, in its own type.)
        let group = styles(&mut app, "News");
        assert!(close_to(&group, &name), "group rows: {group:?}");
        let row = styles(&mut app, "1  Test Channel One");
        assert!(close_to(&row, &name), "channel list row: {row:?}");
        // What is on, in the list: the programmes' family.
        let on = app
            .screen()
            .texts()
            .into_iter()
            .find(|t| t.ends_with("  Quiet Harbour"))
            .map(str::to_owned)
            .expect("the list's programme line");
        let condensed = Some("'Roboto Condensed', Roboto, sans-serif".to_owned());
        assert!(close_to(
            &styles(&mut app, &on),
            &(condensed, 16.0 * scale, 400)
        ));
    }
}

/// Whether one of `found` is `want` (the size to a hundredth).
fn close_to(found: &[(Option<String>, f32, u16)], want: &(Option<String>, f32, u16)) -> bool {
    found
        .iter()
        .any(|(f, s, w)| *f == want.0 && (s - want.1).abs() < 0.01 && *w == want.2)
}

/// The family, size and weight of every span reading `text` (hidden
/// layers, such as the channel list, are mounted too).
fn styles(app: &mut Headless, text: &str) -> Vec<(Option<String>, f32, u16)> {
    let mut out = Vec::new();
    while let Ok(style) = app
        .screen()
        .span_style(&format!("text:{text} >> nth={}", out.len()))
    {
        let family = style.font_family.map(|f| f.name().to_string());
        out.push((family, style.font_size.0, style.font_weight));
    }
    out
}

/// [`styles`] once `ok` holds for them, or after 3 s.
fn styles_when(
    app: &mut Headless,
    text: &str,
    ok: impl Fn(&[(Option<String>, f32, u16)]) -> bool,
) -> Vec<(Option<String>, f32, u16)> {
    let start = std::time::Instant::now();
    loop {
        // The screen moves only as its batches are applied.
        app.catch_up();
        let found = styles(app, text);
        if ok(&found) || start.elapsed() > std::time::Duration::from_secs(3) {
            return found;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Whether `text` has a span in `family` at `size` (to a hundredth) and
/// `weight`, within 3 s (a resize reaches the views on a later frame).
fn has(app: &mut Headless, text: &str, family: &str, size: f32, weight: u16) -> bool {
    let want = (Some(family.to_owned()), size, weight);
    let found = styles_when(app, text, |found| close_to(found, &want));
    let ok = close_to(&found, &want);
    if !ok {
        eprintln!("{text}: wanted {family} {size} {weight}, found {found:?}");
    }
    ok
}

/// The guide's type at size scale `scale`.
fn check_sizes(app: &mut Headless, scale: f32) {
    const ROBOTO: &str = "Roboto, sans-serif";
    const CONDENSED: &str = "'Roboto Condensed', Roboto, sans-serif";
    // Programme titles: Roboto Condensed 20.
    assert!(has(app, "Golden Engine", CONDENSED, 20.0 * scale, 400));
    // Channel names: Roboto 20.
    assert!(has(app, "Test Channel One", ROBOTO, 20.0 * scale, 400));
    // The focused programme's title: in its cell, and in the header in
    // Roboto Medium 22.
    assert!(has(app, "Quiet Harbour", CONDENSED, 20.0 * scale, 400));
    assert!(has(app, "Quiet Harbour", ROBOTO, 22.0 * scale, 500));
}

/// The ruler's half-hour times each span their slot (to the next one, but
/// for the inset): they slide with the programmes in a strip as wide as
/// the programme area, and a time sized to its text would be fitted to
/// the room left between it and that strip's edge, which the slide does
/// not move: near the right edge a platform's text engine (CoreText,
/// Pango) cut it to "2" or "…" although it showed whole on screen.
fn check_ruler(app: &mut Headless) {
    let mut times: Vec<(String, f32, f32, f32)> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter_map(|n| {
            let t = n.text.as_ref()?.text.text.to_string();
            // "11:30 PM" on the scenarios' 12-hour clock ("23:30" on a
            // 24-hour one), or past midnight "Wed 11 · 12:00 AM" (not a
            // programme's "11:10 PM – 12:10 AM", nor the clock's
            // "Tue 10 Nov  11:11 PM").
            let last = t.rsplit(" · ").next().unwrap_or(&t);
            let clock = last
                .strip_suffix(" AM")
                .or_else(|| last.strip_suffix(" PM"))
                .unwrap_or(last);
            let time = clock.split_once(':').is_some_and(|(hour, minute)| {
                (1..=2).contains(&hour.len())
                    && minute.len() == 2
                    && hour.parse::<u32>().is_ok()
                    && minute.parse::<u32>().is_ok()
            }) && !t.contains('–')
                && (last.len() == t.len() || t.contains(" · "));
            time.then_some((
                t,
                n.bounds.origin.x.0,
                n.bounds.origin.y.0,
                n.bounds.size.width.0,
            ))
        })
        .collect();
    // The ruler's row: the highest of the times (lists below show times too).
    let top = times.iter().map(|t| t.2).fold(f32::MAX, f32::min);
    times.retain(|t| (t.2 - top).abs() < 0.5);
    times.sort_by(|a, b| a.1.total_cmp(&b.1));
    // Late in the evening fewer times fit before the next day's label
    // ("Wed 11 · 00:00"): after 23:00 only two (23:00, 23:30), and one
    // pair is a slot to check.
    assert!(times.len() >= 2, "ruler times: {times:?}");
    for pair in times.windows(2) {
        let slot = pair[1].1 - pair[0].1;
        let width = pair[0].3;
        assert!(
            width <= slot + 0.5 && width >= slot - 16.0,
            "ruler time {} is {width} wide in a {slot} slot: {times:?}",
            pair[0].0
        );
    }
}

/// Where row `row`'s channel number is (y), if it is mounted.
fn number_y(app: &mut Headless, row: usize) -> Option<f32> {
    let row = row.to_string();
    let found: Vec<f32> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|n| n.text.as_ref().is_some_and(|t| *t.text.text == *row))
        .map(|n| n.bounds.origin.y.0)
        .collect();
    assert!(found.len() <= 1, "row {row}: {found:?}");
    found.first().copied()
}

/// [`number_y`] once `ok` holds for it, or after 3 s (applying the
/// screen's batches meanwhile).
fn number_y_when(app: &mut Headless, row: usize, ok: impl Fn(f32) -> bool) -> Option<f32> {
    let start = std::time::Instant::now();
    loop {
        app.catch_up();
        let y = number_y(app, row);
        if y.is_some_and(&ok) || start.elapsed() > std::time::Duration::from_secs(3) {
            return y;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The distance between the first two rows: their channel numbers.
fn row_pitch(app: &mut Headless) -> f32 {
    let (one, two) = (number_y(app, 1).unwrap(), number_y(app, 2).unwrap());
    (two - one).round()
}
