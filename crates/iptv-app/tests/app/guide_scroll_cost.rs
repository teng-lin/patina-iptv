//! What a Down press in the guide costs, headless: the text layouts the
//! engine makes and the changes mounted (style, text, frame) for one row
//! down, averaged over a burst that scrolls the list, and the views the
//! guide has Android draw. A guard for scrolling on a TV box, where every
//! text layout is a `StaticLayout` made over JNI, every mounted change a
//! view updated on the UI thread, every drawn view a render node walked
//! each frame and every partly transparent one an offscreen layer
//! (the main costs of guide scrolling on Android).
//!
//! The text engine draws colours from its layouts, as Android's does: a
//! text whose colour changes is laid out again.
//!
//! Synthetic data only: 300 channels from a picked playlist, a local XMLTV
//! guide of half-hour programmes for each.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless::Headless;
use iptv_epg::format_xmltv_time;
use nsh_core::picker::PickedFile;
use nsh_core::{
    ApproxTextEngine, AttributedText, ImageData, MountItem, NodeId, Size, TextEngine, TextLayout,
    element::Root, prelude::*,
};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(30);
const CHANNELS: usize = 300;
/// Presses measured, after a few that bring the focus to the list's
/// scrolling edge.
const PRESSES: usize = 24;
/// Presses before: the list has made the rows it recycles.
const WARM: usize = 40;

fn playlist(guide: &str) -> String {
    let mut m3u = format!("#EXTM3U url-tvg=\"{guide}\"\n");
    for n in 1..=CHANNELS {
        m3u.push_str(&format!(
            "#EXTINF:-1 tvg-id=\"c{n}\" group-title=\"All\",Test Channel {n:03}\n\
             http://stream.example.test/{n}.m3u8\n"
        ));
    }
    m3u
}

/// Half-hour programmes from six hours back to twelve on, each channel's
/// shifted by a few minutes so the rows' cells differ.
fn guide(now: i64) -> String {
    let mut xml = String::from("<?xml version=\"1.0\"?><tv>");
    for n in 1..=CHANNELS {
        xml.push_str(&format!(
            "<channel id=\"c{n}\"><display-name>C{n}</display-name></channel>"
        ));
    }
    let start = now - now % 1800 - 6 * 3600;
    for n in 1..=CHANNELS {
        let shift = (n as i64 % 3) * 600;
        for k in 0..36 {
            let (a, b) = (start + shift + k * 1800, start + shift + (k + 1) * 1800);
            xml.push_str(&format!(
                "<programme channel=\"c{n}\" start=\"{}\" stop=\"{}\">\
                 <title>Programme {n}-{k}</title>\
                 <desc>A synthetic description of programme {n}-{k}, long enough to take two \
                 lines of the header.</desc></programme>",
                format_xmltv_time(a),
                format_xmltv_time(b)
            ));
        }
    }
    xml + "</tv>"
}

/// The approximate engine, counting what it typesets; it draws colours
/// from its layouts (as Android's `StaticLayout` engine does).
#[derive(Clone, Default)]
struct Counting {
    prepared: Arc<AtomicUsize>,
    laid_out: Arc<AtomicUsize>,
}

impl TextEngine for Counting {
    type Prepared = <ApproxTextEngine as TextEngine>::Prepared;

    fn prepare(&self, text: &AttributedText) -> Self::Prepared {
        self.prepared.fetch_add(1, Ordering::Relaxed);
        ApproxTextEngine.prepare(text)
    }

    fn measure(&self, prepared: &Self::Prepared, max_width: Option<Points>) -> Size<Points> {
        ApproxTextEngine.measure(prepared, max_width)
    }

    fn layout(&self, prepared: &Self::Prepared, size: Size<Points>) -> TextLayout {
        self.laid_out.fetch_add(1, Ordering::Relaxed);
        ApproxTextEngine.layout(prepared, size)
    }

    fn register_font(&self, data: &[u8]) -> Result<(), String> {
        ApproxTextEngine.register_font(data)
    }
}

/// What the mounted batches did: changes by kind (`ViewStyle`, `Text`,
/// `Frame`, ...) while counting, and each view's opacity.
#[derive(Default)]
struct Mounted {
    counting: bool,
    kinds: BTreeMap<String, usize>,
    opacity: HashMap<NodeId, f32>,
}

fn kind(item: &MountItem) -> String {
    let debug = format!("{item:?}");
    debug
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The views Android draws (attached and not hidden: it records a view
/// whatever its opacity), and of those the ones partly transparent, which
/// it draws through an offscreen layer each.
fn drawn(app: &Headless, opacity: &HashMap<NodeId, f32>) -> (usize, usize) {
    let shown = app.screen().layout_nodes();
    let shown = shown.iter().filter(|n| !n.hidden);
    let layers =
        |n: &&nsh_testing::LayoutNode| opacity.get(&n.id).is_some_and(|&o| o > 0.0 && o < 1.0);
    (shown.clone().count(), shown.filter(layers).count())
}

#[test]
fn a_down_press_mounts_little_and_the_guide_draws_no_empty_cells_or_layers() {
    let Some(home) = isolated!() else { return };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let guide_path = home.path().join("guide.xml");
    std::fs::write(&guide_path, guide(now)).unwrap();
    let playlist = playlist(&guide_path.display().to_string());

    let config = EngineConfig {
        viewport: Size::new(px(1280.0), px(720.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    let engine = Counting::default();
    let counts = engine.clone();
    let mut app = Headless::start_with_text_engine(config, move || engine).unwrap();
    let mounted: Arc<Mutex<Mounted>> = Arc::default();
    let into = mounted.clone();
    app.observe(move |_, batch| {
        let mut m = into.lock().unwrap();
        for item in &batch.items {
            if let MountItem::ViewStyle { id, style } = item {
                m.opacity.insert(*id, style.opacity);
            }
            if m.counting {
                *m.kinds.entry(kind(item)).or_default() += 1;
            }
        }
    });
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
    step(app.wait_for_within("text:Programme 1-12", IMPORT));
    // Keys from here (the press above was a touch), down to where the
    // list scrolls with the focus.
    for _ in 0..WARM {
        step(app.press_key("down"));
    }
    step(app.wait(Duration::from_secs(1)));

    mounted.lock().unwrap().counting = true;
    let (prepared, laid_out) = (
        counts.prepared.load(Ordering::Relaxed),
        counts.laid_out.load(Ordering::Relaxed),
    );
    for _ in 0..PRESSES {
        step(app.press_key("down"));
    }
    step(app.wait(Duration::from_secs(1)));
    assert!(
        app.shows(&format!("Test Channel {:03}", 1 + WARM + PRESSES)),
        "the focus scrolled the list: {:?}",
        app.screen().texts()
    );
    let per = |n: usize| n as f32 / PRESSES as f32;
    let prepared = per(counts.prepared.load(Ordering::Relaxed) - prepared);
    let laid_out = per(counts.laid_out.load(Ordering::Relaxed) - laid_out);
    let m = mounted.lock().unwrap();
    let (drawn, layers) = drawn(&app, &m.opacity);
    let of = |k: &str| per(m.kinds.get(k).copied().unwrap_or(0));
    let report = format!(
        "per Down press: {prepared:.1} texts prepared, {laid_out:.1} laid out; mounted {:?}; \
         views drawn {drawn}, through a layer {layers}",
        m.kinds
            .iter()
            .map(|(k, n)| (k.as_str(), per(*n)))
            .collect::<Vec<_>>()
    );
    eprintln!("{report}");
    // Measured (2026-11-10): 31 texts laid out a press (two rows'
    // titles change colour, a row scrolls in, the header), 35 styles and
    // 16–21 frames mounted; 1121 views drawn, none through a layer. Before
    // (every cell restyled, empty slots drawn, past cells dimmed by
    // opacity): the same mounts, 1719 views drawn, 64–85 through a layer.
    assert!(laid_out <= 40.0, "{report}");
    assert!(of("ViewStyle") <= 45.0 && of("Text") <= 40.0, "{report}");
    assert!(of("Create") < 1.0, "rows are recycled, not made: {report}");
    assert_eq!(layers, 0, "no view drawn through a layer: {report}");
    assert!(drawn <= 1300, "empty slots are not drawn: {report}");
}
