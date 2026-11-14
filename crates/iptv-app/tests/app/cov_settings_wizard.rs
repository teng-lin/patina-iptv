//! The add-playlist wizard and the choosers over full-screen video,
//! headless: every step of the wizard by keys and by taps (the empty
//! state Back does not leave, the type list, a form's text entry with
//! Done, Cancel, Return and Back, Menu and Paste from the clipboard, a
//! field's error, a failed, a cancelled, an empty and an unsupported
//! import, and a file that is added); then the picture size, sleep timer,
//! audio delay and history choosers, picked by keys and by taps.
//!
//! Synthetic data only: a local mock server (404s, an empty playlist, a
//! request that never answers) and a playlist "picked" from a file.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use crate::headless::Headless;
use nsh_core::picker::PickedFile;
use nsh_core::{ImageData, Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use nsh_testing::Screen;

const PLAYLIST: &str = "#EXTM3U
#EXTINF:-1 tvg-id=\"one\" group-title=\"News\",Test Channel One
http://stream.example.test/one.m3u8
#EXTINF:-1 tvg-id=\"two\" group-title=\"News\",Test Channel Two
http://stream.example.test/two.m3u8
";

const IMPORT: Duration = Duration::from_secs(30);
const GONE: Duration = Duration::from_secs(3);

/// A local server: `/hold…` never answers, `/empty.m3u` is a playlist
/// without channels, everything else is a 404.
fn serve() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || answer(stream));
        }
    });
    port
}

fn answer(mut stream: TcpStream) {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]).into_owned();
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let (status, body) = if path.starts_with("/hold") {
        std::thread::sleep(Duration::from_secs(60));
        return;
    } else if path.starts_with("/empty.m3u") {
        ("200 OK", "#EXTM3U\n")
    } else {
        ("404 Not Found", "Not here")
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

#[track_caller]
fn step(r: Result<(), String>) {
    if let Err(e) = r {
        panic!("{e}");
    }
}

/// The text field of the entry sheet, if one is open.
fn field_id(screen: &Screen) -> Option<u32> {
    screen
        .layout_nodes()
        .into_iter()
        .filter(|n| matches!(n.kind, nsh_core::ViewKind::Input) && !n.hidden)
        .map(|n| n.id)
        .next_back()
}

/// The entry sheet's text field, as a selector, once it is open.
#[track_caller]
fn input(app: &mut Headless) -> String {
    step(app.wait_until("the entry sheet", GONE, |s| field_id(s).is_some()));
    format!("view:{}", field_id(app.screen()).unwrap())
}

/// Waits for the entry sheet to close.
#[track_caller]
fn sheet_closed(app: &mut Headless) {
    step(app.wait_until("the entry sheet closed", GONE, |s| field_id(s).is_none()));
}

/// Presses the entry sheet's button `label` (Cancel, Done).
#[track_caller]
fn sheet_button(app: &mut Headless, label: &str) {
    let field = field_id(app.screen()).expect("the entry sheet");
    let nodes = app.screen().layout_nodes();
    let parent = |id: u32| nodes.iter().find(|n| n.id == id).map(|n| n.parent);
    let inside = |mut id: u32, around: u32| {
        while let Some(up) = parent(id) {
            if up == around {
                return true;
            }
            id = up;
        }
        false
    };
    // The nearest view around the field that holds the button.
    let mut around = field;
    while let Some(up) = parent(around) {
        around = up;
        let holds = nodes.iter().any(|n| {
            n.text.as_ref().is_some_and(|t| *t.text.text == *label) && inside(n.id, around)
        });
        if holds {
            break;
        }
    }
    step(app.press(&format!("view:{around} >> text:{label}")));
}

/// Opens the text entry of the form row reading `row`, replaces the text
/// with `text` and keeps it with Done.
#[track_caller]
fn enter(app: &mut Headless, row: &str, text: &str) {
    step(app.press(&format!("text:{row}")));
    let field = input(app);
    step(app.clear(&field));
    step(app.type_text(&field, text));
    sheet_button(app, "Done");
    sheet_closed(app);
}

/// Taps "Add playlist" and waits for the form again with `banner`.
fn submit_for(app: &mut Headless, banner: &str) {
    step(app.press("text:Add playlist"));
    step(app.wait_for_within(&format!("text:{banner}"), IMPORT));
}

#[test]
fn the_wizard_and_the_choosers() {
    let Some(home) = isolated!() else { return };
    // The history chooser below follows a zap at once; the default switch
    // delay (and its cancellation by opening a chooser) has its own tests.
    let mut settings = iptv_settings::Settings::new();
    settings
        .set("guide.switch_delay", iptv_settings::Value::ms(0))
        .unwrap();
    let data = home.data_dir();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    let port = serve();
    let local = format!("http://127.0.0.1:{port}");
    let config = EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    let mut app = Headless::start(config).unwrap();
    app.on_pick(|_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(PLAYLIST.as_bytes().to_vec()),
        )]
    });

    // The empty state: Back there is not the wizard's (the app would be
    // left), so it stays. OK opens the type list, Back returns.
    step(app.wait_for("text:No playlists yet"));
    step(app.press_key("backspace"));
    step(app.wait(Duration::from_millis(100)));
    assert!(app.shows("No playlists yet"));
    step(app.press_key("enter"));
    step(app.wait_for("text:What did your provider give you?"));
    step(app.wait_for("text:▲▼ choose   OK next   Back back"));
    step(app.press_key("backspace"));
    step(app.wait_for("text:No playlists yet"));
    step(app.wait_for("text:OK add a playlist   Back leave"));
    step(app.press_key("enter"));
    step(app.wait_for("text:What did your provider give you?"));

    // Keys choose the portal type: its form, focused on its first field.
    for key in ["down", "down", "down", "up", "down", "enter"] {
        step(app.press_key(key));
    }
    step(app.wait_for("text:MAC address"));
    step(app.wait_for("text:▲▼ move   OK type   Menu paste   Back other types"));

    // Adding with nothing filled in: the required fields say so.
    step(app.press("text:Add playlist"));
    step(app.wait_for("text:Fill this in"));
    assert_eq!(app.screen().count("text:Fill this in").unwrap(), 2);

    // Menu pastes into the focused field: nothing on the clipboard says
    // so; a link fills the field and clears its error.
    app.set_clipboard(None);
    step(app.press_key("m"));
    step(app.wait_for("text:There is no text to paste."));
    let portal = format!("{local}/c/");
    app.set_clipboard(Some(format!("  {portal}  ")));
    step(app.press_key("m"));
    step(app.wait_for(&format!("text:{portal}")));
    assert_eq!(app.screen().count("text:Fill this in").unwrap(), 1);
    assert!(!app.shows("There is no text to paste."));

    // The MAC address by text entry (OK on the field, keys): shown as
    // dots, never in full.
    step(app.press_key("down"));
    step(app.press_key("enter"));
    let field = input(&mut app);
    step(app.type_text(&field, "00:1A:79:12:34:56"));
    sheet_button(&mut app, "Done");
    sheet_closed(&mut app);
    step(app.wait_for(&format!("text:{}", "•".repeat(16))));
    assert!(!app.shows("00:1A:79:12:34:56"));
    assert!(!app.shows("Fill this in"));

    // The name: Cancel keeps the field as it was, Return keeps the text,
    // Back during entry keeps it too.
    let name = "Optional: how the playlist is listed";
    step(app.press(&format!("text:{name}")));
    let field = input(&mut app);
    step(app.type_text(&field, "Dropped name"));
    sheet_button(&mut app, "Cancel");
    sheet_closed(&mut app);
    assert!(app.shows(name) && !app.shows("Dropped name"));
    step(app.press(&format!("text:{name}")));
    let field = input(&mut app);
    step(app.type_text(&field, "Synthetic portal\n"));
    sheet_closed(&mut app);
    step(app.wait_for("text:Synthetic portal"));
    step(app.press("text:Synthetic portal"));
    let field = input(&mut app);
    step(app.type_text(&field, " two"));
    step(app.back());
    sheet_closed(&mut app);
    step(app.wait_for("text:Synthetic portal two"));

    // The server is no portal: the failure screen, whose only way on is
    // another type.
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:This source cannot be used", IMPORT));
    step(app.wait_for("text:This server is not a service of the chosen type. Try another type."));
    step(app.press("text:Choose another type"));
    step(app.wait_for("text:What did your provider give you?"));

    // A link: not a link, then one the server does not have, then one
    // that never answers (cancelled), then an empty playlist.
    step(app.press("text:Playlist link"));
    let placeholder = "http://…/playlist.m3u";
    step(app.wait_for(&format!("text:{placeholder}")));
    enter(&mut app, placeholder, "nonsense");
    step(app.press("text:Add playlist"));
    step(app.wait_for("text:Start with http:// or https://"));
    let mut url = "nonsense".to_owned();
    for (path, banner) in [
        (
            "/missing.m3u",
            "This is not a playlist the app can read. Check the address.",
        ),
        (
            "/empty.m3u",
            "The playlist loaded but has nothing to watch in it.",
        ),
    ] {
        let next = format!("{local}{path}");
        enter(&mut app, &url, &next);
        url = next;
        submit_for(&mut app, banner);
    }
    let next = format!("{local}/hold.m3u");
    enter(&mut app, &url, &next);
    step(app.press("text:Add playlist"));
    step(app.wait_for("text:Adding the playlist"));
    step(app.wait_for("text:Cancel"));
    step(app.press("text:Cancel"));
    step(app.wait_for("text:Stopped. Your entries are kept; add the playlist again when ready."));
    // Again, and Back stops it.
    step(app.press("text:Add playlist"));
    step(app.wait_for("text:Adding the playlist"));
    step(app.press_key("backspace"));
    step(app.wait_for("text:Stopped. Your entries are kept; add the playlist again when ready."));

    // A login: the password is never shown.
    step(app.back());
    step(app.wait_for("text:What did your provider give you?"));
    step(app.press("text:Xtream Codes login"));
    step(app.wait_for("text:Password"));
    step(app.press("text:Password"));
    let field = input(&mut app);
    step(app.type_text(&field, "pw\n"));
    step(app.wait_for(&format!("text:{}", "•".repeat(4))));
    assert!(!app.shows("pw"));
    // A row's Paste fills that row (the server, the first).
    app.set_clipboard(Some("http://server.example.test:8080".to_owned()));
    step(app.press("text:Paste >> nth=0"));
    step(app.wait_for("text:http://server.example.test:8080"));

    // The file: picked, added, and Watch (OK) shows the guide.
    step(app.back());
    step(app.wait_for("text:What did your provider give you?"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:synthetic.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.wait_for("text:2 channels in 1 group"));
    step(app.press_key("enter"));
    step(app.wait_for_within("text:Test Channel Two", IMPORT));
    assert!(!app.shows("Playlist added"));

    choosers(&mut app);
}

/// The choosers over full-screen video.
fn choosers(app: &mut Headless) {
    // Taps: the channel plays, goes full screen, and a tap on the video
    // shows the play bar with its Picture size. (OK on "Watch" may have
    // started it already.)
    step(app.wait(Duration::from_millis(300)));
    if !app.shows("▶") {
        step(app.press("text:Test Channel One"));
        step(app.wait_for("text:▶"));
    }
    step(app.press("text:Test Channel One"));
    step(app.wait_for("text:/^\\d+ min left$/"));
    step(app.press("label:Video"));
    step(app.wait_for("label:Picture size"));
    step(app.press("label:Picture size"));
    step(app.wait_for("text:Use for all channels"));
    step(app.wait_for("text:✓  Normal"));
    step(app.press("text:Zoom"));
    step(app.wait_for_gone("text:Use for all channels", GONE));
    step(app.press("label:Picture size"));
    step(app.wait_for("text:✓  Zoom"));
    step(app.press("text:Use for all channels"));
    step(app.wait_for_gone("text:Use for all channels", GONE));
    // A second press on the button that opened it closes it.
    step(app.press("label:Picture size"));
    step(app.wait_for("text:Use for all channels"));
    step(app.press("label:Picture size"));
    step(app.wait_for_gone("text:Use for all channels", GONE));

    // The sleep timer from the control strip (Menu): a time starts it,
    // Off (by keys) stops it.
    step(app.press_key("m"));
    step(app.wait_for("text:/^Sleep timer/"));
    step(app.press("text:/^Sleep timer/"));
    step(app.wait_for("text:✓  Off"));
    step(app.press("text:30 minutes"));
    step(app.wait_for_gone("text:✓  Off", GONE));
    step(app.press("text:/^Sleep timer/"));
    step(app.wait_for("text:✓  30 minutes"));
    step(app.press_key("up"));
    step(app.press_key("up"));
    step(app.press_key("enter"));
    step(app.wait_for_gone("text:✓  30 minutes", GONE));
    step(app.press("text:/^Sleep timer/"));
    step(app.wait_for("text:✓  Off"));
    // Back as the remote's key is the platform's (a short Back): the
    // chooser stays; the platform's Back closes it.
    step(app.press_key("back"));
    step(app.wait(Duration::from_millis(100)));
    assert!(app.shows("✓  Off"));
    step(app.back());
    step(app.wait_for_gone("text:✓  Off", GONE));

    // The audio delay: Down then OK, one step later.
    step(app.press("text:Audio delay · 0 ms"));
    step(app.wait_for("text:✓  0 ms"));
    step(app.press_key("down"));
    step(app.press_key("enter"));
    step(app.wait_for("text:Audio delay · +50 ms"));

    // History: the channel zapped from and the one playing (marked); a
    // tap plays the other; a tap beside the chooser closes it.
    step(app.back());
    step(app.press_key("down"));
    step(app.press_key("shift+right"));
    step(app.wait_for("text:Recently watched"));
    step(app.wait_for("text:/^ *2  Test Channel Two  ▶ · /"));
    step(app.press("text:/^ *1  Test Channel One · /"));
    step(app.wait_for_gone("text:Recently watched", GONE));
    step(app.press_key("shift+right"));
    step(app.wait_for("text:/^ *1  Test Channel One  ▶ · /"));
    step(app.press("label:Close the chooser"));
    step(app.wait_for_gone("text:Recently watched", GONE));
}
