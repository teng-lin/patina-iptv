//! A recent search runs again when picked, headless: "new york" typed and
//! searched shows its channels; picked from the recent searches (after
//! Clear, and after closing and opening the screen again) it shows the
//! same matches.
//!
//! Synthetic data only: a playlist picked from a file.

use std::time::Duration;

use crate::headless::Headless;
use nsh_core::picker::PickedFile;
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(30);
/// Generous: a loaded machine (an instrumented coverage run) is slow.
const RESULTS: Duration = Duration::from_secs(10);

#[test]
fn a_recent_search_shows_its_matches_again() {
    let Some(_home) = isolated!() else { return };
    let playlist = "#EXTM3U\n\
         #EXTINF:-1 group-title=\"Local\",US : New York Metro\nhttp://stream.example.test/1.m3u8\n\
         #EXTINF:-1 group-title=\"Local\",US : Chicago Lakeside\nhttp://stream.example.test/2.m3u8\n\
         #EXTINF:-1 group-title=\"Local\",US : New Haven\nhttp://stream.example.test/3.m3u8\n";
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
    step(app.wait_for_within("text:US : Chicago Lakeside", IMPORT));

    // Typed and searched: its channel, not the others.
    open_search(&mut app);
    step(app.wait_for("text:No recent searches yet"));
    step(app.type_text("id:search-field", "new york"));
    step(app.wait_for_within("text:1   US : New York Metro", RESULTS));
    step(app.press_key("enter"));

    // Cleared: the recent search, picked, searches again.
    step(app.press("text:Clear"));
    step(app.wait_for("text:Recent searches"));
    step(app.press("text:new york"));
    step(app.wait_for_within("text:1   US : New York Metro", RESULTS));

    // Closed and opened again: the saved recent search does too.
    step(app.press("text:Close"));
    step(app.wait_for_gone("text:Recent searches", Duration::from_secs(2)));
    open_search(&mut app);
    step(app.wait_for("text:Recent searches"));
    step(app.press("text:new york"));
    step(app.wait_for_within("text:1   US : New York Metro", RESULTS));
}

/// The rail's Search, from the guide.
fn open_search(app: &mut Headless) {
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));
    step(app.press("text:☰  All channels"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for("text:Search >> nth=0"));
    step(app.press("text:Search >> nth=0"));
}

/// Recent searches an older build kept in a plaintext file move into the
/// encrypted store once: the file is deleted, the store holds them (not
/// as plaintext), and they are there after a restart.
#[test]
fn an_old_history_file_moves_into_the_store() {
    let Some(home) = isolated!() else { return };
    let data = home.data_dir();
    std::fs::create_dir_all(&data).unwrap();
    let legacy = data.join("search-history.json");
    std::fs::write(&legacy, r#"["synthetic legacy query"]"#).unwrap();
    let playlist = "#EXTM3U\n\
         #EXTINF:-1 group-title=\"Local\",US : New York Metro\nhttp://stream.example.test/1.m3u8\n";
    let start = || {
        Headless::start(EngineConfig {
            viewport: Size::new(px(1920.0), px(1080.0)),
            ..EngineConfig::new(Root::of::<iptv_app::App>())
        })
        .unwrap()
    };
    let step = |r: Result<(), String>| r.unwrap_or_else(|e| panic!("{e}"));
    let mut app = start();
    app.on_pick(move |_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(playlist.as_bytes().to_vec()),
        )]
    });
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
    step(app.wait_for_within("text:US : New York Metro", IMPORT));
    open_search(&mut app);
    step(app.wait_for("text:synthetic legacy query"));
    assert!(!legacy.exists(), "the plaintext file is deleted");
    step(app.type_text("id:search-field", "metro"));
    step(app.wait_for_within("text:1   US : New York Metro", RESULTS));
    step(app.press_key("enter"));
    step(app.press("text:Close"));
    drop(app);

    for name in ["iptv.sqlite", "iptv.sqlite-wal"] {
        let bytes = std::fs::read(data.join(name)).unwrap_or_default();
        assert!(
            !bytes
                .windows(b"synthetic legacy query".len())
                .any(|w| w == b"synthetic legacy query"),
            "{name} is encrypted"
        );
    }
    let mut app = start();
    step(app.wait_for_within("text:US : New York Metro", IMPORT));
    open_search(&mut app);
    step(app.wait_for("text:metro"));
    step(app.wait_for("text:synthetic legacy query"));
    assert!(!legacy.exists());
}
