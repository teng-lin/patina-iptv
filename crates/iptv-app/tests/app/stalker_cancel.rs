//! Zapping away from a Stalker channel whose stream address is still being
//! asked for cancels that request: the portal sees its connection close.
//! Only generated catalogues and a local mock portal are used.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use crate::headless::Headless;
use iptv_providers::{
    HttpResponse,
    model::{ContentKind, MacAddress, Source, SourceConfig, StalkerCredentials},
    stalker::{parse_channel_page, parse_genres},
};
use iptv_settings::{Settings, Value};
use iptv_store_sqlite::SourcePolicy;
use nsh_core::{Size, element::Root, prelude::*};
use nsh_engine::EngineConfig;

const WAIT: Duration = Duration::from_secs(20);
const GENRES: &str = r#"{"js":[{"id":"1","title":"News","censored":0}]}"#;
const CHANNELS: &str = concat!(
    r#"{"js":{"total_items":"2","max_page_items":"14","data":["#,
    r#"{"id":"100","name":"Slow portal channel","number":"1","#,
    r#""cmd":"ffmpeg http://stream.example.test/slow/100.m3u8","#,
    r#""tv_genre_id":"1","use_http_tmp_link":"1","use_load_balancing":"0"},"#,
    r#"{"id":"101","name":"Quick portal channel","number":"2","#,
    r#""cmd":"ffmpeg http://stream.example.test/quick/101.m3u8","#,
    r#""tv_genre_id":"1","use_http_tmp_link":"1","use_load_balancing":"0"}]}}"#,
);

/// What the portal saw of the slow channel's link request.
#[derive(Default)]
struct Slow {
    asked: bool,
    /// The app closed the connection before an answer.
    abandoned: bool,
    quick_asked: bool,
}

fn serve() -> (String, Arc<(Mutex<Slow>, Condvar)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new((Mutex::new(Slow::default()), Condvar::new()));
    let shared = state.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let state = shared.clone();
            std::thread::spawn(move || answer(stream, &state));
        }
    });
    (url, state)
}

fn answer(mut stream: TcpStream, state: &(Mutex<Slow>, Condvar)) {
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let mut request = Vec::new();
    let mut buffer = [0; 2048];
    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&buffer[..n]),
        }
    }
    let request = String::from_utf8_lossy(&request);
    let path = request.split_whitespace().nth(1).unwrap_or("");
    let action = path
        .split_once('?')
        .into_iter()
        .flat_map(|(_, query)| query.split('&'))
        .find_map(|pair| pair.strip_prefix("action="))
        .unwrap_or("");
    let body = match action {
        "handshake" => r#"{"js":{"token":"generated-token","random":"generated-random"}}"#.into(),
        "get_profile" => r#"{"js":{"status":0,"watchdog_timeout":60}}"#.to_owned(),
        "get_genres" => GENRES.to_owned(),
        "get_ordered_list" => CHANNELS.to_owned(),
        "create_link" if path.contains("slow") => {
            state.0.lock().unwrap().asked = true;
            state.1.notify_all();
            // No answer: wait until the app hangs up (or the test ends).
            let closed = matches!(stream.read(&mut buffer), Ok(0) | Err(_));
            state.0.lock().unwrap().abandoned = closed;
            state.1.notify_all();
            return;
        }
        "create_link" => {
            state.0.lock().unwrap().quick_asked = true;
            state.1.notify_all();
            r#"{"js":{"cmd":"ffmpeg http://stream.example.test/quick-resolved.m3u8"}}"#.to_owned()
        }
        _ => r#"{"js":{}}"#.to_owned(),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

#[test]
fn zapping_away_cancels_a_pending_stalker_link_request() {
    let Some(home) = isolated!() else { return };
    let data = home.data_dir();
    std::fs::create_dir_all(&data).unwrap();
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(true))
        .unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    let (url, state) = serve();
    let mut store = crate::isolated::open_database(data.join("iptv.sqlite")).unwrap();
    let source = store
        .add_source(&Source {
            name: "Portal".into(),
            config: SourceConfig::Stalker(StalkerCredentials {
                portal_url: format!("{url}/portal.php"),
                mac: MacAddress::parse("00:1A:79:00:00:43").unwrap(),
                serial: None,
                device_id: None,
                device_id2: None,
                signature: None,
                signature2: None,
                login: None,
                password: None,
            }),
        })
        .unwrap();
    let groups = parse_genres(&HttpResponse::ok(GENRES), ContentKind::Live).unwrap();
    let channels = parse_channel_page(&HttpResponse::ok(CHANNELS)).unwrap();
    let slow = channels.items.items[0].id.as_str().to_owned();
    store
        .replace_catalog(source, &groups.items, &channels.items.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(source, SourcePolicy::default())
        .unwrap();
    store.set_active_source(source).unwrap();
    drop(store);
    std::fs::write(data.join(format!("last_channel-{}", source.0)), slow).unwrap();

    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    let asked = |what: fn(&Slow) -> bool| {
        let state = state.clone();
        move || what(&state.0.lock().unwrap())
    };
    let wait = crate::isolated::wait_outside;
    wait(
        &mut app,
        "the slow channel's link request",
        WAIT,
        asked(|s| s.asked),
    )
    .unwrap();
    // Another channel: the slow request is no longer wanted.
    app.press_key("down").unwrap();
    wait(
        &mut app,
        "the abandoned link request",
        WAIT,
        asked(|s| s.abandoned),
    )
    .unwrap();
    wait(
        &mut app,
        "the next channel's link",
        WAIT,
        asked(|s| s.quick_asked),
    )
    .unwrap();
}
