//! A cold start resuming a live HLS channel from a large stored catalog
//! while its provider is slow to deliver the live edge's segment: the
//! stream reaches the player with the segment's first packets, before the
//! guide window is read, instead of after the whole segment (#153). The
//! guide then arrives with the resumed channel in it and the stream is
//! not opened again. Its first screen is on screen before the window is
//! read (#160).
//!
//! Synthetic data only: generated channels and programs, and a local
//! server standing in for the provider.
use crate::headless::Headless;
use iptv_store_sqlite::ChannelQuery;
use nsh_core::{ImageData, Size, element::Root, picker::PickedFile, prelude::*};
use nsh_engine::EngineConfig;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Generated channels besides the resumed one.
const CHANNELS: usize = 3_000;
const IMPORT: Duration = Duration::from_secs(120);

/// This process's startup marks, in order.
static MARKS: Marks = Marks(Mutex::new(Vec::new()));
struct Marks(Mutex<Vec<String>>);
impl log::Log for Marks {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record) {
        let line = record.args().to_string();
        if let Some(phase) = line.strip_prefix("iptv startup: ") {
            let phase = phase.split(" +").next().unwrap_or_default().to_owned();
            self.0.lock().unwrap().push(phase);
        }
    }
    fn flush(&self) {}
}
fn marked(phase: &str) -> Option<usize> {
    MARKS.0.lock().unwrap().iter().position(|p| p == phase)
}

/// A live HLS provider whose segments send a PAT and one packet, then hold
/// the rest until released.
struct Provider {
    url: String,
    segments: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
}
impl Provider {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/live.m3u8", listener.local_addr().unwrap());
        let segments = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicBool::new(false));
        let (requested, held) = (segments.clone(), released.clone());
        std::thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                let (requested, held) = (requested.clone(), held.clone());
                std::thread::spawn(move || serve(socket, &requested, &held));
            }
        });
        Self {
            url,
            segments,
            released,
        }
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.released.store(true, Ordering::SeqCst);
    }
}
fn packet(value: u8) -> Vec<u8> {
    let mut bytes = vec![value; 188];
    bytes[0] = 0x47;
    bytes
}
fn serve(mut socket: TcpStream, segments: &AtomicUsize, released: &AtomicBool) {
    let mut request = String::new();
    let mut reader = BufReader::new(socket.try_clone().unwrap());
    let mut line = String::new();
    while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
        request.push_str(&line);
        line.clear();
    }
    if request.starts_with("GET /live.m3u8 ") {
        let body = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n\
                    #EXTINF:2,\nsegment.ts\n";
        let _ = write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        return;
    }
    segments.fetch_add(1, Ordering::SeqCst);
    let mut pat = packet(0);
    pat[..4].copy_from_slice(&[0x47, 0x40, 0, 0x10]);
    let _ = write!(
        socket,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        64 * 188
    );
    let _ = socket.write_all(&[pat, packet(1)].concat());
    let _ = socket.flush();
    // Only the test (or its end) releases the rest.
    while !released.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = socket.write_all(&packet(2).repeat(62));
}

fn app() -> Headless {
    Headless::start(EngineConfig {
        viewport: Size::new(px(1280.0), px(720.0)),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    })
    .unwrap()
}

#[test]
fn a_slow_live_edge_does_not_hold_the_resumed_stream_behind_the_guide() {
    let Some(home) = isolated!() else { return };
    log::set_logger(&MARKS).expect("this process's only logger");
    log::set_max_level(log::LevelFilter::Info);
    crate::performance::prepare(&home);
    let provider = Provider::start();
    let now = crate::support::now();
    let hour = now - now.rem_euclid(3600);
    let guide_path = home.path().join("synthetic-guide.xml");
    let mut guide = String::from("<tv>");
    let mut playlist = format!(
        "#EXTM3U url-tvg=\"{}\"\n\
         #EXTINF:-1 tvg-id=\"resumed\" group-title=\"Synthetic\",Resumed Channel\n{}\n",
        guide_path.display(),
        provider.url
    );
    for channel in 0..=CHANNELS {
        let id = if channel == 0 {
            "resumed".to_owned()
        } else {
            playlist.push_str(&format!(
                "#EXTINF:-1 tvg-id=\"c{channel}\" group-title=\"Synthetic\",Channel {channel:05}\n\
                 http://stream.example.test/{channel}\n"
            ));
            format!("c{channel}")
        };
        guide.push_str(&format!(
            "<channel id=\"{id}\"><display-name>{id}</display-name></channel>"
        ));
        for h in -6..18 {
            let start = hour + h * 3600;
            guide.push_str(&format!(
                "<programme channel=\"{id}\" start=\"{}\" stop=\"{}\"><title>Show {h}</title></programme>",
                iptv_epg::format_xmltv_time(start),
                iptv_epg::format_xmltv_time(start + 3600)
            ));
        }
    }
    guide.push_str("</tv>");
    std::fs::write(guide_path, guide).unwrap();

    // First run: the playlist is added; nothing plays.
    let mut first = app();
    first.on_pick(move |_| {
        vec![PickedFile::new(
            "synthetic.m3u",
            "audio/x-mpegurl",
            ImageData::new(playlist.as_bytes().to_vec()),
        )]
    });
    first.wait_for("text:No playlists yet").unwrap();
    first.press("text:Add playlist").unwrap();
    first.wait_for("text:Playlist file").unwrap();
    first.press("text:Playlist file").unwrap();
    first.wait_for("text:Choose a file…").unwrap();
    first.press("text:Choose a file…").unwrap();
    first.wait_for("text:synthetic.m3u").unwrap();
    first.press("text:Add playlist").unwrap();
    first
        .wait_for_within("text:Playlist added", IMPORT)
        .unwrap();
    first.press("text:Watch").unwrap();
    first.wait_for_within("text:Show 0", IMPORT).unwrap();
    drop(first);
    assert_eq!(
        provider.segments.load(Ordering::SeqCst),
        0,
        "nothing played"
    );

    // The resumed channel played last; the cold start reads only the store.
    let data = home.data_dir();
    let mut store = crate::isolated::open_database(data.join("iptv.sqlite")).unwrap();
    let source = store.sources().unwrap()[0].id;
    let resumed = store
        .load_channels(source, &ChannelQuery::default())
        .unwrap()
        .into_iter()
        .find(|row| row.name == "Resumed Channel")
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
    drop(store);
    std::fs::write(
        data.join(format!("last_channel-{}", source.0)),
        resumed.channel.id.as_str(),
    )
    .unwrap();

    let opens = iptv_app::player_opens(0);
    let started = Instant::now();
    let mut app = app();
    app.wait_until("the resumed stream is published", IMPORT, |_| {
        iptv_app::player_has_source(0)
    })
    .unwrap();
    assert_eq!(
        provider.segments.load(Ordering::SeqCst),
        1,
        "one shared ingest of the live edge"
    );
    // Well inside the provider client's 30 s read timeout and the relay's
    // stall bound: no timeout and no fallback produced this publication.
    let published = started.elapsed();
    assert!(
        published < Duration::from_secs(15),
        "published with the segment's first packets, not after all of it: {published:?}"
    );
    assert!(
        !provider.released.load(Ordering::SeqCst),
        "the segment is still held"
    );
    app.wait_until("the guide window is read", IMPORT, |_| {
        marked("guide window read").is_some()
    })
    .unwrap();
    let (ready, opened, window) = (
        marked("live input ready").expect("the shared input became ready"),
        marked("player opened").expect("the player opened"),
        marked("guide window read").unwrap(),
    );
    let (read, shown) = (marked("first screen read"), marked("first screen shown"));
    assert!(
        read.is_some_and(|read| shown.is_some_and(|shown| read < shown && shown < window)),
        "the first screen shows before the guide window is read: {:?}",
        MARKS.0.lock().unwrap()
    );
    assert!(
        ready < opened && opened < window,
        "the resumed stream opens before the guide window is read: {:?}",
        MARKS.0.lock().unwrap()
    );
    provider.released.store(true, Ordering::SeqCst);
    // The guide arrives with the resumed channel; it is not opened again.
    app.wait_for_within("text:Resumed Channel", IMPORT).unwrap();
    app.idle(Duration::from_millis(500)).unwrap();
    assert_eq!(iptv_app::player_opens(0), opens + 1, "opened once");
}
