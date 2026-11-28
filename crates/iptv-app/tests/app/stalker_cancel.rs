//! Zapping away from a Stalker channel whose stream address is still being
//! asked for cancels that request: the portal sees its connection close.
//! Only generated catalogs and a local mock portal are used.
use std::{
    io::{ErrorKind, Read, Write},
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
    requests: usize,
    asked: bool,
    asked_count: usize,
    /// The app closed the connection before an answer.
    abandoned: bool,
    abandoned_count: usize,
    quick_asked: bool,
    quick_calls: usize,
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
    state.0.lock().unwrap().requests += 1;
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
            {
                let mut seen = state.0.lock().unwrap();
                seen.asked = true;
                seen.asked_count += 1;
            }
            state.1.notify_all();
            // No answer: wait until the app hangs up (or the test ends).
            // Several individually scaled waits can follow this request.
            // A mock-server deadline is not evidence of app cancellation.
            let budget = crate::isolated::scaled_duration(WAIT.saturating_mul(8));
            let closed = wait_for_cancel(&mut stream, budget);
            {
                let mut seen = state.0.lock().unwrap();
                seen.abandoned = closed;
                if closed {
                    seen.abandoned_count += 1;
                }
            }
            state.1.notify_all();
            return;
        }
        "create_link" => {
            {
                let mut seen = state.0.lock().unwrap();
                seen.quick_asked = true;
                seen.quick_calls += 1;
            }
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

fn wait_for_cancel(stream: &mut TcpStream, budget: Duration) -> bool {
    stream.set_read_timeout(Some(budget)).unwrap();
    loop {
        match stream.read(&mut [0; 256]) {
            Ok(0) => return true,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => {
                return matches!(
                    error.kind(),
                    ErrorKind::ConnectionReset
                        | ErrorKind::ConnectionAborted
                        | ErrorKind::BrokenPipe
                );
            }
            Ok(_) => {}
        }
    }
}

#[test]
fn a_mock_read_timeout_does_not_mean_the_application_canceled() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    assert!(!wait_for_cancel(&mut server, Duration::from_millis(30)));
    drop(client);
    assert!(wait_for_cancel(&mut server, Duration::from_secs(1)));
}

fn start_portal(home: &crate::isolated::Home) -> (Headless, Arc<(Mutex<Slow>, Condvar)>) {
    let data = home.data_dir();
    std::fs::create_dir_all(&data).unwrap();
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(true))
        .unwrap();
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
    // The cancellation scenario holds main playback while resolving an auxiliary.
    settings
        .set_in(
            "playlists.max_connections",
            &iptv_settings::Target::playlist(source.0.to_string()),
            Value::Int(2),
        )
        .unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    std::fs::write(data.join(format!("last_channel-{}", source.0)), slow).unwrap();

    let app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    (app, state)
}

#[test]
fn zapping_away_cancels_a_pending_stalker_link_request() {
    let Some(home) = isolated!() else { return };
    let (mut app, state) = start_portal(&home);
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

#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn retrying_an_auxiliary_player_keeps_the_main_stalker_request_alive() {
    use nsh_core::{EventKind, EventPayload, MountItem};
    use serde_json::json;
    let Some(home) = isolated!() else { return };
    let (mut app, state) = start_portal(&home);
    crate::isolated::wait_outside(&mut app, "pending main link", WAIT, || {
        state.0.lock().unwrap().asked
    })
    .unwrap();
    app.set_environment("a desktop", |e| {
        e.idiom = nsh_core::platform::Idiom::Desktop;
        e.os = if cfg!(target_os = "macos") {
            nsh_core::platform::Os::MacOs
        } else {
            nsh_core::platform::Os::Linux
        };
    })
    .unwrap();
    app.hover("label:Video").unwrap();
    app.press("label:TV guide").unwrap();
    let auxiliary = Arc::new(Mutex::new(None));
    let seen = auxiliary.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::ComponentProps { id, props } = item
                && props
                    .iter()
                    .any(|(key, value)| key.as_ref() == "popupSurface" && *value == Some(json!(1)))
            {
                *seen.lock().unwrap() = Some(*id);
            }
        }
    });
    app.right_click("id:guide-ui >> label:Quick portal channel")
        .unwrap();
    app.press("text:Open in new window").unwrap();
    crate::isolated::wait_outside(&mut app, "auxiliary link", WAIT, || {
        state.0.lock().unwrap().quick_calls == 1
    })
    .unwrap();
    app.wait_until("auxiliary source resolved", WAIT, |_| {
        iptv_app::player_has_source(1)
    })
    .unwrap();
    app.engine().control().dispatch_event(
        auxiliary.lock().unwrap().unwrap(),
        EventKind::NativeEvent,
        EventPayload::Native {
            name: "error".into(),
            data: json!({"code": "ERROR_CODE_DECODING_FAILED"}),
        },
    );
    app.wait_for("id:auxiliary-player-1 >> text:Try again")
        .unwrap();
    app.press("id:auxiliary-player-1 >> text:Try again")
        .unwrap();
    crate::isolated::wait_outside(&mut app, "auxiliary link retried", WAIT, || {
        state.0.lock().unwrap().quick_calls >= 2
    })
    .unwrap();
    app.idle(Duration::from_millis(150)).unwrap();
    assert!(
        !state.0.lock().unwrap().abandoned,
        "retrying B must not cancel A's runtime slot"
    );
}

#[test]
fn cached_static_stalker_playback_keeps_its_origin_without_a_portal_session() {
    let Some(home) = isolated!() else { return };
    let data = home.data_dir();
    std::fs::create_dir_all(&data).unwrap();
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    let (portal, requests) = serve();
    let mut store = crate::isolated::open_database(data.join("iptv.sqlite")).unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated cached portal".into(),
            config: SourceConfig::Stalker(StalkerCredentials {
                portal_url: format!("{portal}/portal.php"),
                mac: MacAddress::parse("00:1A:79:00:00:81").unwrap(),
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
    let channels = parse_channel_page(&HttpResponse::ok(concat!(
        r#"{"js":{"total_items":"1","max_page_items":"14","data":["#,
        r#"{"id":"181","name":"Generated static channel","number":"1","#,
        r#""cmd":"ffmpeg http://stream.example.test/generated-static.ts","#,
        r#""tv_genre_id":"1","use_http_tmp_link":"0","use_load_balancing":"0"}]}}"#,
    )))
    .unwrap();
    store
        .replace_catalog(source, &groups.items, &channels.items.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(
            source,
            SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    store.set_active_source(source).unwrap();
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let observed = opened.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let nsh_core::MountItem::ComponentProps { props, .. } = item {
                for (name, value) in props {
                    if name.as_ref() == "source"
                        && let Some(native) = value
                        && native["url"].is_string()
                    {
                        observed
                            .lock()
                            .unwrap()
                            .push(crate::support::upstream_source(native));
                    }
                }
            }
        }
    });
    app.wait_for_within("text:Generated static channel", WAIT)
        .unwrap();
    assert_eq!(
        requests.0.lock().unwrap().requests,
        0,
        "cached catalog needs no portal session"
    );
    let before = iptv_app::player_opens(0);
    app.press_key("home").unwrap();
    app.press_key("enter").unwrap();
    app.wait_until(
        "the cached static source reaches the native player",
        WAIT,
        |_| !opened.lock().unwrap().is_empty(),
    )
    .unwrap();
    let opened = opened.lock().unwrap();
    assert_eq!(opened.len(), 1);
    assert_eq!(
        opened[0]["url"],
        "http://stream.example.test/generated-static.ts"
    );
    assert!(opened[0]["headers"]["Authorization"].is_null());
    assert_eq!(iptv_app::player_opens(0), before + 1);
    assert_eq!(
        requests.0.lock().unwrap().requests,
        0,
        "static playback must not start a portal handshake or create_link"
    );
}

#[derive(Clone, Default)]
struct PendingCastPort;

impl iptv_runtime::cast::Port for PendingCastPort {
    fn discover(&self) -> iptv_runtime::cast::Task<'_, Vec<iptv_core::cast::Receiver>> {
        Box::pin(async {
            Ok(vec![iptv_core::cast::Receiver {
                id: "googlecast:slow-portal-tv".into(),
                name: "Generated TV".into(),
                protocol: iptv_core::cast::NetworkProtocol::GoogleCast,
            }])
        })
    }

    fn prepare(
        &self,
        _receiver: iptv_core::cast::Receiver,
        _source: iptv_core::playback::PlaySource,
        _pairing: iptv_runtime::cast::PairingPrompt,
    ) -> iptv_runtime::cast::Task<'_, Box<dyn iptv_runtime::cast::Prepared>> {
        Box::pin(async { Err(iptv_core::cast::Error::Cancelled) })
    }
}

#[cfg(target_os = "linux")]
fn start_portal_with_options(
    home: &crate::isolated::Home,
    resume_last: bool,
    external_news_group: bool,
) -> (Headless, Arc<(Mutex<Slow>, Condvar)>) {
    let data = home.data_dir();
    std::fs::create_dir_all(&data).unwrap();
    let mut settings = Settings::new();
    settings
        .set("general.resume_last_channel", Value::Bool(resume_last))
        .unwrap();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    let (url, state) = serve();
    let mut store = crate::isolated::open_database(data.join("iptv.sqlite")).unwrap();
    let source = store
        .add_source(&Source {
            name: "Portal".into(),
            config: SourceConfig::Stalker(StalkerCredentials {
                portal_url: format!("{url}/portal.php"),
                mac: MacAddress::parse("00:1A:79:00:00:44").unwrap(),
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
    let quick = channels.items.items[1].id.as_str().to_owned();
    store
        .replace_catalog(source, &groups.items, &channels.items.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(source, SourcePolicy::default())
        .unwrap();
    if external_news_group {
        store
            .set_guide_group_preference(
                source,
                groups.items[0].id.as_str(),
                iptv_core::organization::GroupPreference::ExternalPlayer(true),
            )
            .unwrap();
    }
    store.set_active_source(source).unwrap();
    drop(store);
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    if resume_last {
        std::fs::write(data.join(format!("last_channel-{}", source.0)), quick).unwrap();
    }
    let app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    (app, state)
}

#[test]
fn stopping_cast_while_a_stalker_link_is_pending_cancels_the_portal_request() {
    let Some(home) = isolated!() else { return };
    iptv_app::install_test_cast_port(Arc::new(PendingCastPort));
    let (mut app, state) = start_portal(&home);
    app.set_environment("a desktop", |e| {
        e.idiom = nsh_core::platform::Idiom::Desktop;
        e.os = if cfg!(target_os = "macos") {
            nsh_core::platform::Os::MacOs
        } else {
            nsh_core::platform::Os::Linux
        };
    })
    .unwrap();
    crate::isolated::wait_outside(&mut app, "local slow link", WAIT, || {
        state.0.lock().unwrap().asked_count == 1
    })
    .unwrap();
    if !app.screen().matches("id:play-bar-back").unwrap() {
        app.press("id:video").unwrap();
    }
    app.wait_for_within("id:play-bar-back", WAIT).unwrap();
    app.play_bar_option("Cast to TV").unwrap();
    app.wait_for_within("id:chooser-items >> text:Generated TV", WAIT)
        .unwrap();
    app.press("id:chooser-items >> text:Generated TV").unwrap();
    crate::isolated::wait_outside(&mut app, "cast slow link", WAIT, || {
        state.0.lock().unwrap().asked_count == 2
    })
    .unwrap();
    // Stopping playback while Cast is starting cancels both the local and Cast Stalker requests.
    app.press_remote(nsh_tv::RemoteKey::Stop).unwrap();
    crate::isolated::wait_outside(&mut app, "cast link cancelled", WAIT, || {
        state.0.lock().unwrap().abandoned_count == 2
    })
    .unwrap();
}

#[test]
#[cfg(target_os = "linux")]
fn closing_an_external_handoff_while_a_stalker_link_is_pending_cancels_the_portal_request() {
    let Some(home) = isolated!() else { return };
    iptv_app::set_external_player_handler(Some(true));
    let (mut app, state) = start_portal_with_options(&home, false, true);
    app.wait_for_within("text:News", WAIT).unwrap();
    // Enter the News group (which has external_player enabled).
    app.press("text:News").unwrap();
    app.wait_for_within("text:Slow portal channel", WAIT)
        .unwrap();
    // Selecting Slow portal channel starts an external-player Stalker link lookup.
    app.press("text:Slow portal channel").unwrap();
    crate::isolated::wait_outside(&mut app, "external slow link", WAIT, || {
        state.0.lock().unwrap().asked
    })
    .unwrap();
    // Pressing Back cancels the pending external handoff and its Stalker request.
    app.back().unwrap();
    crate::isolated::wait_outside(&mut app, "external link cancelled", WAIT, || {
        state.0.lock().unwrap().abandoned
    })
    .unwrap();
}
