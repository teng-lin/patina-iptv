//! Combined startup must wait for the saved channel's own portal session.
//! Only generated catalogues and a gated local mock portal are used.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use crate::headless::Headless;
use iptv_providers::{
    HttpResponse, SecretUrl,
    model::{ContentKind, M3uSourceConfig, MacAddress, Source, SourceConfig, StalkerCredentials},
    stalker::{parse_channel_page, parse_genres},
};
use iptv_settings::{Settings, Value};
use iptv_store_sqlite::{SourceId, SourcePolicy, Store};
use nsh_core::{ImageData, MountItem, Size, element::Root, picker::PickedFile, prelude::*};
use nsh_engine::EngineConfig;

const IMPORT: Duration = Duration::from_secs(20);
const STREAM: &str = "http://stream.example.test/resumed-stalker.m3u8";
const GENRES: &str = r#"{"js":[{"id":"1","title":"News","censored":0}]}"#;
const CHANNELS: &str = concat!(
    r#"{"js":{"total_items":"1","max_page_items":"14","data":[{"id":"100","#,
    r#""name":"Saved portal channel","number":"1","#,
    r#""cmd":"ffmpeg http://stream.example.test/original/100.m3u8","#,
    r#""tv_genre_id":"1","use_http_tmp_link":"1","use_load_balancing":"0"}]}}"#,
);
const M3U: &str = "#EXTM3U\n#EXTINF:-1 tvg-id=\"generated-m3u\" group-title=\"News\",Ready M3U\nhttp://stream.example.test/m3u\n";

#[derive(Default)]
struct Gates {
    auth_started: bool,
    auth_released: bool,
    m3u_released: bool,
    reject_auth: bool,
    fresh_catalog: bool,
    auth_attempts: usize,
}

struct Portal {
    url: String,
    gates: Arc<(Mutex<Gates>, Condvar)>,
    links: Arc<AtomicUsize>,
}

impl Portal {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let gates = Arc::new((Mutex::new(Gates::default()), Condvar::new()));
        let links = Arc::new(AtomicUsize::new(0));
        let shared = gates.clone();
        let observed = links.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let gates = shared.clone();
                let links = observed.clone();
                std::thread::spawn(move || answer(stream, gates, links));
            }
        });
        Self { url, gates, links }
    }

    fn auth_started(&self) -> bool {
        self.gates.0.lock().unwrap().auth_started
    }

    fn release_m3u(&self) {
        self.gates.0.lock().unwrap().m3u_released = true;
        self.gates.1.notify_all();
    }

    fn release_auth(&self) {
        self.gates.0.lock().unwrap().auth_released = true;
        self.gates.1.notify_all();
    }

    fn reject_auth(&self) {
        self.gates.0.lock().unwrap().reject_auth = true;
    }

    fn recover_auth(&self) {
        let mut state = self.gates.0.lock().unwrap();
        state.reject_auth = false;
        state.fresh_catalog = true;
        state.auth_released = true;
        self.gates.1.notify_all();
    }

    fn auth_attempts(&self) -> usize {
        self.gates.0.lock().unwrap().auth_attempts
    }
}

impl Drop for Portal {
    fn drop(&mut self) {
        self.release_m3u();
        self.release_auth();
    }
}

fn answer(mut stream: TcpStream, gates: Arc<(Mutex<Gates>, Condvar)>, links: Arc<AtomicUsize>) {
    stream.set_read_timeout(Some(IMPORT)).unwrap();
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
    let authorized = request.lines().any(|line| {
        line.split_once(':').is_some_and(|(key, value)| {
            key.eq_ignore_ascii_case("authorization") && value.trim() == "Bearer generated-token"
        })
    });
    let body = if path == "/playlist.m3u" {
        let mut state = gates.0.lock().unwrap();
        while !state.m3u_released {
            state = gates.1.wait(state).unwrap();
        }
        M3U.to_owned()
    } else if action == "handshake" {
        let mut state = gates.0.lock().unwrap();
        state.auth_started = true;
        state.auth_attempts += 1;
        gates.1.notify_all();
        if state.reject_auth {
            r#"{"js":{}}"#.to_owned()
        } else {
            while !state.auth_released {
                state = gates.1.wait(state).unwrap();
            }
            r#"{"js":{"token":"generated-token","random":"generated-random"}}"#.to_owned()
        }
    } else if !authorized {
        "Authorization failed.".to_owned()
    } else {
        match action {
            "get_profile" => r#"{"js":{"status":0,"watchdog_timeout":60}}"#.to_owned(),
            "get_genres" => GENRES.to_owned(),
            "get_ordered_list" if gates.0.lock().unwrap().fresh_catalog => {
                CHANNELS.replace("Saved portal channel", "Authenticated portal channel")
            }
            "get_ordered_list" => CHANNELS.to_owned(),
            "create_link" => {
                links.fetch_add(1, Ordering::SeqCst);
                format!(r#"{{"js":{{"cmd":"ffmpeg {STREAM}"}}}}"#)
            }
            _ => panic!("unexpected generated portal action {action}"),
        }
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

#[track_caller]
fn step(result: Result<(), String>) {
    result.unwrap_or_else(|error| panic!("{error}"));
}

fn seed_portal(store: &mut Store, portal: &Portal) -> (SourceId, String) {
    let stalker = store
        .add_source(&Source {
            name: "Portal".into(),
            config: SourceConfig::Stalker(StalkerCredentials {
                portal_url: format!("{}/portal.php", portal.url),
                mac: MacAddress::parse("00:1A:79:00:00:42").unwrap(),
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
    let channel = channels.items.items[0].id.as_str().to_owned();
    store
        .replace_catalog(stalker, &groups.items, &channels.items.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(stalker, SourcePolicy::default())
        .unwrap();
    store.set_active_source(stalker).unwrap();
    (stalker, channel)
}

fn exercise(data: &Path, combined_key: bool) {
    std::fs::create_dir_all(data).unwrap();
    let mut settings = Settings::new();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    settings
        .set("general.resume_last_channel", Value::Bool(true))
        .unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    let portal = Portal::start();
    let mut store = crate::isolated::open_database(data.join("iptv.sqlite")).unwrap();
    let (stalker, channel) = seed_portal(&mut store, &portal);
    if combined_key {
        store
            .set_combined_last_channel(&format!("source:{}:{channel}", stalker.0))
            .unwrap();
    } else {
        std::fs::write(data.join(format!("last_channel-{}", stalker.0)), channel).unwrap();
    }
    let m3u = store
        .add_source(&Source {
            name: "M3U".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new(format!("{}/playlist.m3u", portal.url)),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let cached = iptv_providers::m3u::parse_m3u(&M3U.replace("Ready M3U", "Cached M3U"));
    store
        .replace_catalog(m3u, &cached.groups, &cached.channels.items, &[], &[])
        .unwrap();
    store
        .set_source_policy(m3u, SourcePolicy::default())
        .unwrap();
    store.set_all_playlists(true).unwrap();
    drop(store);

    let before = iptv_app::player_opens(0);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed = opened.clone();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::ComponentProps { props, .. } = item {
                for (key, value) in props {
                    if key.as_ref() == "source"
                        && let Some(url) = value.as_ref().and_then(|value| value["url"].as_str())
                    {
                        observed.lock().unwrap().push(url.into());
                    }
                }
            }
        }
    });
    step(app.wait_for_within("text:Saved portal channel · Portal", IMPORT));
    step(crate::isolated::wait_outside(
        &mut app,
        "portal authentication is waiting",
        IMPORT,
        || portal.auth_started(),
    ));
    // Refreshing the other source now produces can_resume=true only after the
    // saved portal channel is visible, while its own auth remains gated.
    portal.release_m3u();
    step(app.wait_for_within("text:Ready M3U · M3U", IMPORT));
    assert_eq!(iptv_app::player_opens(0), before);
    assert!(!iptv_app::player_has_source(0));
    assert!(opened.lock().unwrap().is_empty());
    assert_eq!(
        app.screen().count("text:▶").unwrap(),
        0,
        "no premature playing channel"
    );
    assert_eq!(
        app.screen()
            .count("text:This channel has no stream address.")
            .unwrap(),
        0
    );
    assert_eq!(portal.links.load(Ordering::SeqCst), 0);

    portal.release_auth();
    step(crate::isolated::wait_outside(
        &mut app,
        "saved portal stream opens after its login",
        IMPORT,
        || opened.lock().unwrap().iter().any(|url| url == STREAM),
    ));
    step(app.wait(Duration::from_millis(250)));
    assert_eq!(
        iptv_app::player_opens(0),
        before + 1,
        "one stream open after login"
    );
    assert_eq!(opened.lock().unwrap().as_slice(), &[STREAM]);
    assert_eq!(portal.links.load(Ordering::SeqCst), 1);
    assert!(iptv_app::player_has_source(0));
}

#[test]
fn saved_combined_channel_waits_for_its_own_portal_authentication() {
    let Some(home) = isolated!() else { return };
    exercise(&home.data_dir(), true);
}

#[test]
fn previous_single_playlist_channel_waits_for_its_own_portal_authentication() {
    let Some(home) = isolated!() else { return };
    exercise(&home.data_dir(), false);
}

#[test]
fn adding_playlist_reauthenticates_a_cached_portal_after_startup_auth_failed() {
    let Some(home) = isolated!() else { return };
    let data = home.data_dir();
    std::fs::create_dir_all(&data).unwrap();
    let mut settings = Settings::new();
    settings.set("guide.autoplay", Value::Bool(false)).unwrap();
    settings
        .set("general.resume_last_channel", Value::Bool(false))
        .unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    let portal = Portal::start();
    portal.reject_auth();
    let path = data.join("iptv.sqlite");
    let mut store = crate::isolated::open_database(&path).unwrap();
    let (stalker, _) = seed_portal(&mut store, &portal);
    drop(store);
    let mut app = Headless::start(EngineConfig {
        viewport: Size::new(px(1920.0), px(1080.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap();
    step(app.wait_for_within("text:Saved portal channel", IMPORT));
    step(crate::isolated::wait_outside(
        &mut app,
        "failed portal startup is recorded",
        IMPORT,
        || {
            crate::isolated::open_database(&path)
                .unwrap()
                .import_status(stalker)
                .unwrap()
                .last_error
                .is_some()
        },
    ));
    // An enabled Refresh now action confirms the UI has consumed completion:
    // this source is no longer RUNNING, but its cached context is still absent.
    step(app.press("text:☰  All channels"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for("text:Playlists >> nth=0"));
    step(app.press("text:Playlists >> nth=0"));
    step(app.wait_for("text:Portal"));
    step(app.press("text:Portal"));
    step(app.wait_for_within("text:Refresh now", IMPORT));
    step(app.back());
    step(app.wait_for("text:Close"));
    step(app.press("text:Close"));
    let failed_attempts = portal.auth_attempts();
    assert!(failed_attempts > 0);
    portal.recover_auth();
    app.on_pick(|_| {
        vec![PickedFile::new(
            "Additional.m3u",
            "audio/x-mpegurl",
            ImageData::new(M3U.as_bytes().to_vec()),
        )]
    });
    step(app.press("text:+ Add playlist"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:Additional.m3u"));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:Authenticated portal channel · Portal", IMPORT));
    assert!(
        portal.auth_attempts() > failed_attempts,
        "the cached portal must authenticate again"
    );
    assert!(
        crate::isolated::open_database(&path)
            .unwrap()
            .import_status(stalker)
            .unwrap()
            .last_error
            .is_none()
    );
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 2);
    assert_eq!(
        portal.links.load(Ordering::SeqCst),
        0,
        "browsing does not start a stream"
    );
}
