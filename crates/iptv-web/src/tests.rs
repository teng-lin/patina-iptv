//! The app end to end through a fake host: a synthetic playlist and guide
//! are imported over the host protocol, then the remote drives the guide
//! screen, full screen, search and Favorites.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use serde_json::Value;

use crate::host::{Host, install};

const NOW_MS: f64 = 1_800_000_000_000.0;

#[derive(Default)]
struct Fake {
    requests: RefCell<Vec<(u32, Value, bool)>>,
    next: RefCell<Vec<u32>>,
    closed: RefCell<Vec<u32>>,
    events: RefCell<Vec<Value>>,
    kv: RefCell<std::collections::HashMap<String, Vec<u8>>>,
    gets: RefCell<Vec<(u32, String)>>,
    timers: RefCell<Vec<(u32, f64)>>,
    /// Every URL requested, in order.
    urls: RefCell<Vec<String>>,
    /// Added to the wall clock (time passing).
    wall_offset_ms: Cell<f64>,
}

impl Host for Fake {
    fn fetch(&self, id: u32, request: &str, stream: bool) {
        self.requests
            .borrow_mut()
            .push((id, serde_json::from_str(request).unwrap(), stream));
    }
    fn fetch_next(&self, id: u32) {
        self.next.borrow_mut().push(id);
    }
    fn fetch_close(&self, id: u32) {
        self.closed.borrow_mut().push(id);
    }
    fn sleep(&self, id: u32, ms: f64) {
        self.timers.borrow_mut().push((id, ms));
    }
    fn store_get(&self, id: u32, key: &str) {
        self.gets.borrow_mut().push((id, key.to_owned()));
    }
    fn store_put(&self, key: &str, value: &[u8]) {
        let mut kv = self.kv.borrow_mut();
        if value.is_empty() {
            kv.remove(key);
        } else {
            kv.insert(key.to_owned(), value.to_vec());
        }
    }
    fn emit(&self, event: &str) {
        self.events
            .borrow_mut()
            .push(serde_json::from_str(event).unwrap());
    }
    fn monotonic_ms(&self) -> f64 {
        0.0
    }
    fn wall_ms(&self) -> f64 {
        NOW_MS + self.wall_offset_ms.get()
    }
}

fn xmltv_time(unix: i64) -> String {
    // A fixed UTC format; the test needs only hours around now.
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}00 +0000",
        secs / 3600,
        secs % 3600 / 60
    )
}

fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn playlist() -> String {
    "#EXTM3U url-tvg=\"http://guide.example.com/epg.xml\"\n\
     #EXTINF:-1 tvg-id=\"one.ex\" group-title=\"News\",Example One\nhttp://media.example.com/one.ts\n\
     #EXTINF:-1 tvg-id=\"two.ex\" group-title=\"News\",Example Two\nhttp://media.example.com/two.ts\n\
     #EXTINF:-1 tvg-id=\"three.ex\" group-title=\"Sport\",Example Three\nhttp://media.example.com/three.ts\n"
        .to_owned()
}

fn guide() -> String {
    // An hour that has ended, then the one on now and the next.
    guide_of(&["one", "two", "three"], &["Dawn", "Morning", "Noon"])
}

/// A guide for channels `ids`: hour-long programs named `names`, the first
/// an hour before now.
fn guide_of(ids: &[&str], names: &[&str]) -> String {
    let now = (NOW_MS / 1000.0) as i64;
    let hour = now - now.rem_euclid(3600);
    let mut xml = String::from("<tv>");
    for id in ids {
        xml += &format!("<channel id=\"{id}.ex\"><display-name>Example</display-name></channel>");
        for (i, name) in names.iter().enumerate() {
            let start = hour + (i as i64 - 1) * 3600;
            xml += &format!(
                "<programme start=\"{}\" stop=\"{}\" channel=\"{id}.ex\"><title>{id} {name}</title></programme>",
                xmltv_time(start),
                xmltv_time(start + 3600)
            );
        }
    }
    xml + "</tv>"
}

/// How a fake server answers: a status and body, or a network failure.
type Responder = dyn Fn(&str) -> Result<(u16, String), u32>;

fn deliver(fake: &Fake) {
    deliver_with(fake, &|url| {
        Ok((
            200,
            if url.ends_with(".m3u") {
                playlist()
            } else {
                guide()
            },
        ))
    });
}

fn deliver_with(fake: &Fake, respond: &Responder) {
    // Answer every request and chunk read until the import is idle.
    loop {
        let requests: Vec<_> = fake.requests.borrow_mut().drain(..).collect();
        let next: Vec<_> = fake.next.borrow_mut().drain(..).collect();
        let gets: Vec<_> = fake.gets.borrow_mut().drain(..).collect();
        // Retry delays pass at once; the guide's next check (hours) does not.
        let due: Vec<u32> = {
            let mut timers = fake.timers.borrow_mut();
            let due = timers
                .iter()
                .filter(|t| t.1 < 600_000.0)
                .map(|t| t.0)
                .collect();
            timers.retain(|t| t.1 >= 600_000.0);
            due
        };
        if requests.is_empty() && next.is_empty() && gets.is_empty() && due.is_empty() {
            break;
        }
        for id in due {
            crate::exec::deliver_wake(id);
            crate::exec::poll();
        }
        for (id, key) in gets {
            let value = fake.kv.borrow().get(&key).cloned();
            crate::exec::deliver_stored(id, value);
            crate::exec::poll();
        }
        for (id, req, stream) in requests {
            let url = req["url"].as_str().unwrap().to_owned();
            fake.urls.borrow_mut().push(url.clone());
            match respond(&url) {
                Ok((status, body)) => {
                    crate::exec::deliver_status(id, status, Some(body.len() as u64));
                    if stream {
                        STREAMS.with(|s| s.borrow_mut().push((id, body.into_bytes(), false)));
                    } else {
                        crate::exec::deliver_body(id, body.into_bytes());
                    }
                }
                Err(kind) => crate::exec::deliver_error(id, kind, "unreachable".into()),
            }
            crate::exec::poll();
        }
        for id in next {
            let chunk = STREAMS.with(|s| {
                let mut s = s.borrow_mut();
                let entry = s.iter_mut().find(|e| e.0 == id).expect("stream");
                if entry.2 {
                    None
                } else {
                    entry.2 = true;
                    Some(std::mem::take(&mut entry.1))
                }
            });
            crate::exec::deliver_chunk(id, chunk);
            crate::exec::poll();
        }
    }
}

thread_local! {
    static STREAMS: RefCell<Vec<(u32, Vec<u8>, bool)>> = const { RefCell::new(Vec::new()) };
}

fn send(cmd: Value) {
    crate::app::command(&cmd.to_string());
    crate::exec::poll();
}

fn last_view(fake: &Fake) -> Value {
    fake.events
        .borrow()
        .iter()
        .rev()
        .find(|e| e["type"] == "view")
        .cloned()
        .expect("a view")
}

#[test]
fn imports_a_playlist_and_its_guide_then_plays_and_zaps() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    send(serde_json::json!({
        "cmd": "source",
        "source": { "kind": "m3u", "url": "http://lists.example.com/list.m3u" },
    }));
    deliver(&fake);
    assert_eq!(
        crate::exec::pending_tasks(),
        1,
        "the guide loop waits for its next check"
    );

    // The guide screen: All channels in the grid, its first row's live
    // program focused, the columns hidden.
    let view = last_view(&fake);
    assert_eq!(view["ready"], true);
    assert_eq!(view["mode"], "guide");
    assert_eq!(view["guide"], true, "{view}");
    assert_eq!(view["title"], "All channels");
    let grid = &view["grid"];
    assert_eq!(
        (grid["pane"].clone(), grid["columns"].clone()),
        ("guide".into(), false.into())
    );
    assert!(
        view["groups"].is_null(),
        "the groups column is not sent while hidden"
    );
    assert_eq!(grid["rows"]["total"], 3);
    assert_eq!(grid["rows"]["items"][0]["name"], "Example One");
    assert_eq!(grid["focused"]["title"], "one Morning");
    assert_eq!(grid["focused"]["favorite"], false);

    // Nothing plays: Up from the first row has no preview to reach. Moving
    // tunes nothing.
    key("up");
    assert_eq!(last_view(&fake)["grid"]["pane"], "guide");
    assert_eq!(plays(&fake), 0);

    // OK on a live program plays it full screen; Down zaps to the next.
    key("ok");
    assert_eq!(
        last_of(&fake, "play")["url"],
        "http://media.example.com/one.ts"
    );
    assert_eq!(last_view(&fake)["mode"], "full");
    key("down");
    assert_eq!(
        last_of(&fake, "play")["url"],
        "http://media.example.com/two.ts"
    );
    assert_eq!(last_view(&fake)["playing"]["name"], "Example Two");

    // Back returns to the guide on the playing channel's row, at now.
    key("back");
    let view = last_view(&fake);
    assert_eq!(view["mode"], "guide");
    assert_eq!(view["grid"]["rows"]["focus"], 1);
    assert_eq!(view["grid"]["focused"]["title"], "two Morning");
    assert_eq!(view["grid"]["rows"]["items"][1]["playing"], true);

    // OK on what already plays live goes full screen without tuning again.
    let tuned = plays(&fake);
    key("ok");
    assert_eq!(last_view(&fake)["mode"], "full");
    assert_eq!(plays(&fake), tuned);
    key("left");
    assert_eq!(last_view(&fake)["mode"], "guide", "Left returns too");

    // Up from the first row reaches the preview while a channel plays; OK
    // there is full screen, Down returns to the first row.
    key("up");
    key("up");
    assert_eq!(last_view(&fake)["grid"]["pane"], "preview");
    key("down");
    assert_eq!(last_view(&fake)["grid"]["pane"], "guide");
    key("up");
    key("ok");
    assert_eq!(last_view(&fake)["mode"], "full");
    assert_eq!(plays(&fake), tuned);
    key("back");

    // Back on a live program shows the groups column; moving over the
    // groups previews their lists in the grid, from the first row.
    key("back");
    let view = last_view(&fake);
    assert_eq!(view["grid"]["pane"], "groups");
    assert_eq!(view["grid"]["columns"], true);
    assert_eq!(view["groups"]["focus"], 2);
    assert_eq!(view["groups"]["items"][3], "News");
    key("down");
    let view = last_view(&fake);
    assert_eq!(view["title"], "News", "{view}");
    assert_eq!(view["grid"]["rows"]["total"], 2);
    assert_eq!(view["grid"]["rows"]["focus"], 0);
    assert_eq!(view["grid"]["pane"], "groups");

    // Right enters it; Left goes back through the programs, and from the
    // earliest one to the groups, then to the rail's Search.
    key("right");
    assert_eq!(last_view(&fake)["grid"]["pane"], "guide");
    key("left");
    assert_eq!(last_view(&fake)["grid"]["focused"]["title"], "one Dawn");
    key("left");
    assert_eq!(last_view(&fake)["grid"]["pane"], "groups");
    key("left");
    let view = last_view(&fake);
    assert_eq!(
        (view["grid"]["pane"].clone(), view["grid"]["rail"].clone()),
        ("rail".into(), "search".into())
    );

    // Back: groups, then the rail (on TV), where it stays: Back never
    // leaves the app.
    key("right");
    key("back");
    let view = last_view(&fake);
    assert_eq!(
        (view["grid"]["pane"].clone(), view["grid"]["rail"].clone()),
        ("rail".into(), "tv".into())
    );
    key("back");
    key("back");
    assert_eq!(last_view(&fake)["grid"]["pane"], "rail");
    assert!(fake.events.borrow().iter().all(|e| e["type"] != "exit"));
    // OK on TV is the groups column again.
    key("ok");
    assert_eq!(last_view(&fake)["grid"]["pane"], "groups");

    // A held Left (the page's `groups`) opens the groups from anywhere in
    // the grid at once; elsewhere it does nothing.
    key("right");
    key("right");
    assert_eq!(last_view(&fake)["grid"]["pane"], "guide");
    key("groups");
    assert_eq!(last_view(&fake)["grid"]["pane"], "groups");
    key("groups");
    assert_eq!(last_view(&fake)["grid"]["pane"], "groups");
}

#[test]
fn refuses_content_supplied_private_addresses() {
    let request = |url: &str, public_only: bool| {
        let mut r = iptv_providers::HttpRequest::get(url);
        r.public_only = public_only;
        r
    };
    let check = |url: &str, public_only: bool| {
        crate::exec::check_destination(&request(url, public_only)).is_ok()
    };
    assert!(check("http://media.example.com/a.ts", true));
    assert!(!check("http://192.168.1.20/epg.xml", true));
    assert!(!check("http://[::1]/epg.xml", true));
    assert!(!check("http://tvbox.local/epg.xml", true));
    assert!(!check("http://nas/epg.xml", true));
    // A server the user typed may be on the LAN.
    assert!(check("http://192.168.1.20/epg.xml", false));
    assert!(!check("file:///etc/passwd", false));
}

fn xtream(url: &str) -> Result<(u16, String), u32> {
    let body = if url.contains("action=get_live_categories") {
        r#"[{"category_id":"1","category_name":"Demo News"},{"category_id":"2","category_name":"Demo Sport"}]"#.to_owned()
    } else if url.contains("action=get_live_streams") {
        r#"[{"num":1,"name":"Demo One","stream_type":"live","stream_id":101,"category_id":"1","epg_channel_id":"one.ex","stream_icon":"","tv_archive":1,"tv_archive_duration":3},
            {"num":2,"name":"Demo Two","stream_type":"live","stream_id":102,"category_id":"1","epg_channel_id":"two.ex","stream_icon":""},
            {"num":3,"name":"Demo Three","stream_type":"live","stream_id":103,"category_id":"2","epg_channel_id":"three.ex","stream_icon":""}]"#
            .to_owned()
    } else if url.contains("xmltv.php") {
        guide()
    } else if url.contains("action=") {
        "[]".to_owned()
    } else {
        r#"{"user_info":{"auth":1,"status":"Active","allowed_output_formats":["m3u8","ts"]},"server_info":{"timezone":"UTC"}}"#.to_owned()
    };
    Ok((200, body))
}

#[test]
fn an_xtream_account_plays_raw_ts_from_its_live_path() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    send(serde_json::json!({
        "cmd": "source",
        "utcOffsetSecs": 3600,
        "source": { "kind": "xtream", "server": "http://tv.example.com", "username": "demo", "password": "secret" },
    }));
    deliver_with(&fake, &xtream);
    let view = last_view(&fake);
    assert_eq!(view["grid"]["rows"]["total"], 3, "{view}");
    assert_eq!(view["guide"], true, "{view}");

    // Page Down clamps to the last row; OK plays it; Channel Up in full
    // screen zaps to the one before, and Back shows its row.
    key("pagedown");
    assert_eq!(last_view(&fake)["grid"]["rows"]["focus"], 2);
    key("ok");
    assert_eq!(
        last_of(&fake, "play")["url"],
        "http://tv.example.com/live/demo/secret/103.ts"
    );
    key("chup");
    assert_eq!(
        last_of(&fake, "play")["url"],
        "http://tv.example.com/live/demo/secret/102.ts"
    );
    key("back");
    assert_eq!(last_view(&fake)["grid"]["rows"]["focus"], 1);
    key("pageup");
    assert_eq!(last_view(&fake)["grid"]["rows"]["focus"], 0);

    // A failed stream clears the playing channel: Up from the first row no
    // longer reaches the preview.
    send(serde_json::json!({ "cmd": "playFailed" }));
    assert!(last_view(&fake)["playing"].is_null());
    key("up");
    assert_eq!(last_view(&fake)["grid"]["pane"], "guide");

    // Recently watched lists what played, newest first.
    key("back");
    key("up");
    let view = last_view(&fake);
    assert_eq!(view["title"], "Recently watched", "{view}");
    assert_eq!(view["grid"]["rows"]["items"][0]["name"], "Demo Two");

    // Reload imports the same account again.
    send(serde_json::json!({ "cmd": "reload" }));
    deliver_with(&fake, &xtream);
    assert_eq!(last_view(&fake)["title"], "All channels");
}

#[test]
fn failures_are_reported_instead_of_an_empty_list() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    send(
        serde_json::json!({ "cmd": "source", "source": { "kind": "xtream", "server": "http://tv.example.com" } }),
    );
    assert_eq!(
        fake.events.borrow().last().unwrap()["type"],
        "sourceRejected"
    );
    send(serde_json::json!({ "cmd": "source", "source": { "kind": "stalker" } }));
    send(serde_json::json!({ "cmd": "nonsense" }));
    crate::app::command("not json");
    assert!(
        fake.events
            .borrow()
            .iter()
            .all(|e| e["type"] == "notice" || e["type"] == "sourceRejected")
    );
    // Keys before any playlist do nothing, Back included.
    send(serde_json::json!({ "cmd": "key", "key": "down" }));
    send(serde_json::json!({ "cmd": "key", "key": "back" }));
    assert!(fake.events.borrow().iter().all(|e| e["type"] != "exit"));

    send(
        serde_json::json!({ "cmd": "source", "source": { "kind": "m3u", "url": "http://lists.example.com/list.m3u" } }),
    );
    deliver_with(&fake, &|_| Err(1));
    let view = last_view(&fake);
    assert_eq!(view["ready"], false);
    assert_eq!(view["status"]["phase"], "failed", "{view}");

    // The playlist loads but its guide cannot be reached: channels show
    // without guide data.
    send(
        serde_json::json!({ "cmd": "source", "source": { "kind": "m3u", "url": "http://lists.example.com/list.m3u" } }),
    );
    deliver_with(&fake, &|url| {
        if url.ends_with(".m3u") {
            Ok((200, playlist()))
        } else {
            Err(2)
        }
    });
    let view = last_view(&fake);
    assert_eq!(view["grid"]["rows"]["total"], 3, "{view}");
    assert_eq!(view["guide"], false);
    assert!(view["grid"]["focused"]["title"].is_null());
}

fn guide_downloads(fake: &Fake) -> usize {
    fake.urls
        .borrow()
        .iter()
        .filter(|u| u.contains("guide.example.com"))
        .count()
}

#[test]
fn the_stored_guide_and_lists_are_used_at_the_next_launch() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    let m3u = serde_json::json!({ "cmd": "source", "source": { "kind": "m3u", "url": "http://lists.example.com/list.m3u" } });
    send(m3u.clone());
    deliver(&fake);
    assert_eq!(guide_downloads(&fake), 1);
    // Down, OK: the second channel plays (recently watched); back on the
    // guide, Yellow on the third makes it a favorite.
    send(serde_json::json!({ "cmd": "key", "key": "down" }));
    send(serde_json::json!({ "cmd": "key", "key": "ok" }));
    send(serde_json::json!({ "cmd": "key", "key": "back" }));
    send(serde_json::json!({ "cmd": "key", "key": "down" }));
    send(serde_json::json!({ "cmd": "key", "key": "favorite" }));
    let notice = fake
        .events
        .borrow()
        .iter()
        .rev()
        .find(|e| e["type"] == "notice")
        .cloned()
        .unwrap();
    assert_eq!(notice["text"], "Added to Favorites");

    // The next launch: the playlist is fetched, the guide is not.
    send(m3u.clone());
    deliver(&fake);
    assert_eq!(guide_downloads(&fake), 1, "the stored guide is fresh");
    let view = last_view(&fake);
    assert_eq!(view["guide"], true, "{view}");
    assert_eq!(view["grid"]["focused"]["title"], "one Morning");
    send(serde_json::json!({ "cmd": "key", "key": "back" }));
    send(serde_json::json!({ "cmd": "key", "key": "up" }));
    send(serde_json::json!({ "cmd": "key", "key": "up" }));
    let view = last_view(&fake);
    assert_eq!(view["title"], "Favorites", "{view}");
    assert_eq!(view["grid"]["rows"]["items"][0]["name"], "Example Three");
    send(serde_json::json!({ "cmd": "key", "key": "down" }));
    let view = last_view(&fake);
    assert_eq!(view["title"], "Recently watched");
    assert_eq!(view["grid"]["rows"]["items"][0]["name"], "Example Two");

    // "Update the guide now" downloads it again.
    send(serde_json::json!({ "cmd": "refreshGuide" }));
    deliver(&fake);
    assert_eq!(guide_downloads(&fake), 2);
    assert_eq!(last_view(&fake)["guide"], true);

    // Another playlist replaces this one and its stored data.
    let before: Vec<String> = fake.kv.borrow().keys().cloned().collect();
    assert!(before.iter().any(|k| k.starts_with("guide:")), "{before:?}");
    send(
        serde_json::json!({ "cmd": "source", "source": { "kind": "m3u", "url": "http://other.example.com/list.m3u" } }),
    );
    deliver_with(&fake, &|url| {
        Ok((
            200,
            if url.ends_with(".m3u") {
                playlist().replace("guide.example.com", "guide2.example.com")
            } else {
                guide()
            },
        ))
    });
    let after: Vec<String> = fake.kv.borrow().keys().cloned().collect();
    assert!(
        before
            .iter()
            .filter(|k| *k != "current-source")
            .all(|k| !after.contains(k)),
        "{after:?}"
    );
    assert!(after.iter().any(|k| k.starts_with("guide:")));
}

#[test]
fn a_failed_refresh_keeps_the_stored_guide() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    send(
        serde_json::json!({ "cmd": "source", "source": { "kind": "m3u", "url": "http://lists.example.com/list.m3u" } }),
    );
    deliver(&fake);
    let stored = fake
        .kv
        .borrow()
        .iter()
        .find(|(k, _)| k.starts_with("guide:"))
        .map(|(_, v)| v.clone())
        .unwrap();
    send(serde_json::json!({ "cmd": "refreshGuide" }));
    deliver_with(&fake, &|url| {
        if url.ends_with(".m3u") {
            Ok((200, playlist()))
        } else {
            Err(1)
        }
    });
    let view = last_view(&fake);
    assert_eq!(view["guide"], true, "{view}");
    assert_eq!(view["grid"]["focused"]["title"], "one Morning");
    let kept = fake
        .kv
        .borrow()
        .iter()
        .find(|(k, _)| k.starts_with("guide:"))
        .map(|(_, v)| v.clone())
        .unwrap();
    assert_eq!(kept, stored);
    let state = fake
        .kv
        .borrow()
        .iter()
        .find(|(k, _)| k.starts_with("guide-state:"))
        .map(|(_, v)| v.clone())
        .unwrap();
    let state: Value = serde_json::from_slice(&state).unwrap();
    assert_eq!(state["failed"], true);
}

#[test]
fn logos_from_content_must_be_public() {
    // Playlist streams on LAN addresses are refused by the import itself;
    // logos are only filtered here, before the TV's image loader sees them.
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    let lan = "#EXTM3U\n\
        #EXTINF:-1 tvg-logo=\"http://192.168.1.5/logo.png\" group-title=\"News\",Lan Logo\nhttp://media.example.com/one.ts\n\
        #EXTINF:-1 tvg-logo=\"http://img.example.com/two.png\" group-title=\"News\",Public Logo\nhttp://media.example.com/two.ts\n";
    send(
        serde_json::json!({ "cmd": "source", "source": { "kind": "m3u", "url": "http://lists.example.com/list.m3u" } }),
    );
    deliver_with(&fake, &|_| Ok((200, lan.to_owned())));
    let view = last_view(&fake);
    assert!(view["grid"]["rows"]["items"][0]["logo"].is_null(), "{view}");
    assert_eq!(
        view["grid"]["rows"]["items"][1]["logo"],
        "http://img.example.com/two.png"
    );
}

#[test]
fn a_canceled_request_is_aborted_and_its_slot_freed() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    send(
        serde_json::json!({ "cmd": "source", "source": { "kind": "m3u", "url": "http://lists.example.com/list.m3u" } }),
    );
    let first = fake.requests.borrow()[0].0;
    // Another playlist before the first answered: the first request is aborted.
    send(
        serde_json::json!({ "cmd": "source", "source": { "kind": "m3u", "url": "http://other.example.com/list.m3u" } }),
    );
    assert!(
        fake.closed.borrow().contains(&first),
        "{:?}",
        fake.closed.borrow()
    );
    // A late answer to it changes nothing.
    crate::exec::deliver_status(first, 200, None);
    crate::exec::poll();
    deliver(&fake);
    assert_eq!(last_view(&fake)["grid"]["rows"]["total"], 3);
}

#[test]
fn a_redirect_may_not_downgrade_or_leave_public_addresses() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    send(
        serde_json::json!({ "cmd": "source", "source": { "kind": "m3u", "url": "https://lists.example.com/list.m3u" } }),
    );
    let id = fake.requests.borrow_mut().remove(0).0;
    crate::exec::deliver_redirect(id, "http://lists.example.com/list.m3u");
    crate::exec::poll();
    assert!(fake.closed.borrow().contains(&id));
    let view = last_view(&fake);
    assert_eq!(view["status"]["phase"], "failed", "{view}");
}

#[test]
fn a_task_woken_by_a_later_task_of_the_same_poll_runs_in_that_poll() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    let (send, receive) = futures_like::channel();
    let done = Rc::new(std::cell::Cell::new(false));
    let seen = done.clone();
    crate::exec::spawn(async move {
        receive.await;
        seen.set(true);
    });
    crate::exec::spawn(async move { send.send() });
    crate::exec::poll();
    assert!(done.get(), "the waiting task ran once its reply arrived");
    assert_eq!(crate::exec::pending_tasks(), 0);
}

#[test]
fn a_task_spawned_outside_a_poll_asks_the_host_for_one() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    crate::exec::poll();
    let before = fake.timers.borrow().len();
    crate::exec::spawn(async {});
    assert_eq!(fake.timers.borrow().len(), before + 1);
    crate::exec::poll();
    assert_eq!(crate::exec::pending_tasks(), 0);
}

/// A one-shot that registers its waker, as a real channel does.
mod futures_like {
    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};

    #[derive(Default)]
    struct Shared {
        sent: bool,
        waker: Option<Waker>,
    }
    pub struct Sender(Rc<RefCell<Shared>>);
    pub struct Receiver(Rc<RefCell<Shared>>);
    pub fn channel() -> (Sender, Receiver) {
        let shared = Rc::default();
        (Sender(Rc::clone(&shared)), Receiver(shared))
    }
    impl Sender {
        pub fn send(self) {
            let mut shared = self.0.borrow_mut();
            shared.sent = true;
            if let Some(waker) = shared.waker.take() {
                waker.wake();
            }
        }
    }
    impl Future for Receiver {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            let mut shared = self.0.borrow_mut();
            if shared.sent {
                Poll::Ready(())
            } else {
                shared.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

#[test]
fn a_spawn_before_any_host_does_not_silence_later_wakes() {
    // No host on this thread yet: nothing can be asked for.
    crate::exec::spawn(async {});
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    crate::exec::spawn(async {});
    assert_eq!(
        fake.timers.borrow().len(),
        1,
        "the second spawn asks for a poll"
    );
    crate::exec::poll();
    assert_eq!(crate::exec::pending_tasks(), 0);
}

#[test]
fn a_request_wakes_its_task_under_another_executor() {
    use iptv_import::Fetch;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Wake, Waker};
    struct Count(AtomicUsize);
    impl Wake for Count {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    let count = std::sync::Arc::new(Count(AtomicUsize::new(0)));
    let waker = Waker::from(count.clone());
    let mut cx = Context::from_waker(&waker);
    let fetch = crate::exec::HostFetch::new();
    let mut request = std::pin::pin!(fetch.fetch_stream(iptv_providers::HttpRequest::get(
        "https://example.test/list.m3u"
    )));
    assert!(request.as_mut().poll(&mut cx).is_pending());
    let id = fake.requests.borrow()[0].0;
    crate::exec::deliver_status(id, 200, None);
    assert_eq!(
        count.0.load(Ordering::SeqCst),
        1,
        "the delivery woke the task"
    );
    assert!(request.as_mut().poll(&mut cx).is_ready());
}

#[test]
fn a_chosen_address_may_not_redirect_to_a_private_origin() {
    use iptv_import::Fetch;
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    let fetch = crate::exec::HostFetch::new();
    let request = |url: &str| {
        let fetch = fetch.clone();
        let url = url.to_owned();
        async move {
            fetch
                .fetch_stream(iptv_providers::HttpRequest::get(url))
                .await
        }
    };
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let mut elsewhere = std::pin::pin!(request("https://example.test/list.m3u"));
    assert!(elsewhere.as_mut().poll(&mut cx).is_pending());
    let id = fake.requests.borrow().last().unwrap().0;
    crate::exec::deliver_redirect(id, "https://192.168.1.20/list.m3u");
    match elsewhere.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(Err(error)) => {
            assert_eq!(error.kind, iptv_import::FetchErrorKind::Redirect)
        }
        _ => panic!("a private origin after a redirect is refused"),
    }
    // A chosen LAN address keeps its own origin.
    let mut home = std::pin::pin!(request("https://192.168.1.20/list.m3u"));
    assert!(home.as_mut().poll(&mut cx).is_pending());
    let id = fake.requests.borrow().last().unwrap().0;
    crate::exec::deliver_redirect(id, "https://192.168.1.20/other.m3u");
    crate::exec::deliver_status(id, 200, None);
    assert!(matches!(
        home.as_mut().poll(&mut cx),
        std::task::Poll::Ready(Ok(_))
    ));
}

#[test]
fn a_delivery_asks_for_no_extra_poll() {
    use iptv_import::Fetch;
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    let fetch = crate::exec::HostFetch::new();
    crate::exec::spawn(async move {
        let _ = fetch
            .fetch_stream(iptv_providers::HttpRequest::get(
                "https://example.test/list.m3u",
            ))
            .await;
    });
    crate::exec::poll();
    let timers = fake.timers.borrow().len();
    let id = fake.requests.borrow()[0].0;
    // As an export does: deliver, then poll.
    crate::exec::deliver_status(id, 200, None);
    crate::exec::poll();
    assert_eq!(
        fake.timers.borrow().len(),
        timers,
        "the export's own poll suffices"
    );
    assert_eq!(crate::exec::pending_tasks(), 0);
}

fn key(k: &str) {
    send(serde_json::json!({ "cmd": "key", "key": k }));
}

fn last_of(fake: &Fake, kind: &str) -> Value {
    fake.events
        .borrow()
        .iter()
        .rev()
        .find(|e| e["type"] == kind)
        .cloned()
        .unwrap_or(Value::Null)
}

fn plays(fake: &Fake) -> usize {
    fake.events
        .borrow()
        .iter()
        .filter(|e| e["type"] == "play")
        .count()
}

fn m3u_launch(fake: &Rc<Fake>) {
    send(
        serde_json::json!({ "cmd": "source", "source": { "kind": "m3u", "url": "http://lists.example.com/list.m3u" } }),
    );
    deliver(fake);
}

#[test]
fn the_guide_grid_moves_through_channels_and_time_and_plays() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    m3u_launch(&fake);

    let view = last_view(&fake);
    let grid = &view["grid"];
    assert_eq!(grid["minutes"], crate::grid::GRID_MINUTES);
    assert_eq!(grid["focused"]["airing"], "live");
    let row = &grid["rows"]["items"][0];
    let titles: Vec<&str> = row["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"one Morning"), "{titles:?}");
    let start = grid["start"].as_i64().unwrap();
    let now = (NOW_MS / 1000.0) as i64;
    let hour = now - now.rem_euclid(3600);
    assert!(start <= now && now < start + 150 * 60, "now is on screen");
    // Past programs without an archive are dimmed.
    let dawn = row["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["title"] == "one Dawn")
        .cloned();
    if let Some(dawn) = dawn {
        assert_eq!(dawn["airing"], "ended");
    }

    // Down, Right: the next channel's next program. OK on it only says when
    // it starts.
    key("down");
    key("right");
    let focused = last_view(&fake)["grid"]["focused"].clone();
    assert_eq!(focused["title"], "two Noon");
    assert_eq!(focused["airing"], "upcoming");
    key("ok");
    assert_eq!(plays(&fake), 0);
    assert_eq!(last_view(&fake)["mode"], "guide");
    let noon = (hour + 3600).rem_euclid(86_400);
    assert_eq!(
        last_of(&fake, "notice")["text"],
        format!("Starts at {:02}:{:02}", noon / 3600, noon % 3600 / 60)
    );

    // Left twice: the hour that ended, which an M3U without catch-up
    // cannot play.
    key("left");
    key("left");
    let focused = last_view(&fake)["grid"]["focused"].clone();
    assert_eq!(focused["title"], "two Dawn", "{focused}");
    assert_eq!(focused["airing"], "ended");
    assert_eq!(focused["catchup"], false, "an M3U without catch-up");
    key("ok");
    assert_eq!(plays(&fake), 0);
    assert_eq!(last_of(&fake, "notice")["text"], "No longer available");

    // Back returns to now; OK plays the channel live, full screen; Back
    // returns to the guide on its row.
    key("back");
    assert_eq!(last_view(&fake)["grid"]["focused"]["title"], "two Morning");
    key("ok");
    let play = last_of(&fake, "play");
    assert_eq!(play["url"], "http://media.example.com/two.ts");
    assert!(play["archive"].is_null());
    assert_eq!(last_view(&fake)["mode"], "full");
    key("back");
    let view = last_view(&fake);
    assert_eq!(view["mode"], "guide");
    assert_eq!(view["grid"]["focused"]["title"], "two Morning");
    assert_eq!(view["grid"]["rows"]["items"][1]["playing"], true);
    key("chdown");
    assert_eq!(
        last_view(&fake)["grid"]["rows"]["focus"],
        2,
        "a page down clamps"
    );

    // The Guide key from full screen opens the guide on the playing row.
    key("chup");
    key("down");
    key("ok");
    assert_eq!(last_view(&fake)["mode"], "full");
    key("guide");
    let view = last_view(&fake);
    assert_eq!(view["mode"], "guide");
    assert_eq!(view["grid"]["rows"]["focus"], 1);
}

#[test]
fn the_grid_plays_an_archived_program_from_its_start() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    send(serde_json::json!({
        "cmd": "source",
        "source": { "kind": "xtream", "server": "http://tv.example.com", "username": "demo", "password": "secret" },
    }));
    deliver_with(&fake, &xtream);
    // The archive's past is not dimmed.
    let row = last_view(&fake)["grid"]["rows"]["items"][0].clone();
    let cells = row["cells"].as_array().unwrap();
    assert!(cells.iter().any(|c| c["airing"] == "archive"), "{cells:?}");
    key("left");
    let focused = last_view(&fake)["grid"]["focused"].clone();
    assert_eq!(focused["title"], "one Dawn", "{focused}");
    assert_eq!(focused["catchup"], true);
    key("ok");
    let play = last_of(&fake, "play");
    let now = (NOW_MS / 1000.0) as i64;
    let dawn = now - now.rem_euclid(3600) - 3600;
    assert_eq!(play["archive"], dawn);
    let url = play["url"].as_str().unwrap();
    assert!(url.contains("timeshift"), "{url}");
    assert_eq!(last_view(&fake)["mode"], "full");

    // Back: the guide on the channel at now. OK on its live program tunes
    // it live (the archive played).
    key("back");
    let view = last_view(&fake);
    assert_eq!(view["grid"]["focused"]["title"], "one Morning");
    let tuned = plays(&fake);
    key("ok");
    assert_eq!(plays(&fake), tuned + 1);
    assert!(last_of(&fake, "play")["archive"].is_null());
}

#[test]
fn search_finds_channels_at_once_and_programs_once_indexed() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    m3u_launch(&fake);

    // Search opens with the keyboard on the query field.
    key("search");
    let view = last_view(&fake);
    assert_eq!(view["mode"], "search");
    let search = &view["search"];
    assert_eq!(search["focus"], "input");
    assert_eq!(search["keyboard"], true);
    assert_eq!(search["preparing"], true);

    // Channels are found before the program index is built.
    send(serde_json::json!({ "cmd": "searchText", "text": "two" }));
    let search = last_view(&fake)["search"].clone();
    assert_eq!(search["phase"], "results");
    assert_eq!(search["counts"]["channels"], 1);
    assert_eq!(search["counts"]["programs"], 0);
    // The index builds in steps between deliveries, then the query runs
    // again with programs.
    deliver(&fake);
    let metric = last_of(&fake, "metric");
    assert_eq!(metric["name"], "searchIndex");
    assert_eq!(metric["programs"], 9);
    assert!(metric["bytes"].as_u64().unwrap() > 0);
    let search = last_view(&fake)["search"].clone();
    assert_eq!(search["preparing"], false);
    // Two Morning (on now) and Two Noon; Dawn has ended.
    assert_eq!(search["counts"]["programs"], 2, "{search}");

    // Done closes the keyboard and focuses the first result; OK plays it.
    key("done");
    let search = last_view(&fake)["search"].clone();
    assert_eq!(
        (search["keyboard"].clone(), search["focus"].clone()),
        (false.into(), "result".into())
    );
    assert_eq!(search["rows"]["items"][0]["name"], "Example Two");
    key("ok");
    assert_eq!(
        last_of(&fake, "play")["url"],
        "http://media.example.com/two.ts"
    );
    assert_eq!(last_view(&fake)["mode"], "full");
    // Back: the same results, the keyboard still closed.
    key("back");
    let search = last_view(&fake)["search"].clone();
    assert_eq!(search["query"], "two");
    assert_eq!(search["focus"], "result");

    // Programs: Right shows their tab; a program plays its channel.
    key("right");
    let search = last_view(&fake)["search"].clone();
    assert_eq!(search["tab"], "programs");
    let first = &search["rows"]["items"][0];
    assert_eq!(first["title"], "two Morning");
    assert_eq!(first["live"], true);
    key("down");
    key("ok");
    assert_eq!(
        last_of(&fake, "play")["url"],
        "http://media.example.com/two.ts"
    );
    key("back");

    // A query only programs match shows their tab.
    key("up");
    key("up");
    key("up");
    assert_eq!(last_view(&fake)["search"]["focus"], "input");
    key("ok");
    assert_eq!(last_view(&fake)["search"]["keyboard"], true);
    send(serde_json::json!({ "cmd": "searchText", "text": "noon" }));
    let search = last_view(&fake)["search"].clone();
    assert_eq!(search["tab"], "programs", "{search}");
    assert_eq!(search["counts"]["programs"], 3);
    assert_eq!(search["counts"]["channels"], 0);

    // Cancel hides the keyboard; Back then leaves to the guide. What was
    // searched is kept.
    key("cancel");
    assert_eq!(last_view(&fake)["search"]["keyboard"], false);
    key("back");
    assert_eq!(last_view(&fake)["mode"], "guide");
    let lists: Value = fake
        .kv
        .borrow()
        .iter()
        .find(|(k, _)| k.starts_with("lists:"))
        .map(|(_, v)| serde_json::from_slice(v).unwrap())
        .unwrap();
    assert_eq!(lists["searches"], serde_json::json!(["two"]));

    // The guide followed what played. From its groups column, Left is the
    // rail's Search: OK opens it, with the history before a query.
    assert_eq!(last_view(&fake)["grid"]["rows"]["focus"], 1);
    key("back");
    key("left");
    assert_eq!(last_view(&fake)["grid"]["rail"], "search");
    key("ok");
    let search = last_view(&fake)["search"].clone();
    assert_eq!(search["phase"], "idle");
    assert_eq!(search["rows"]["items"][0]["text"], "two");
    assert_eq!(search["preparing"], false, "the index is kept");
}

fn favorite_names(fake: &Fake) -> Vec<String> {
    last_view(fake)["grid"]["rows"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn favorites_are_reordered_in_move_mode_and_keep_their_order() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    m3u_launch(&fake);
    // Blue with no favorites: the empty Favorites group, which cannot be
    // entered, with a hint.
    key("blue");
    let view = last_view(&fake);
    assert_eq!(view["title"], "Favorites");
    assert_eq!(view["grid"]["pane"], "groups");
    assert_eq!(view["groups"]["focus"], 0);
    let hint = "No favorites yet. Yellow adds the focused channel.";
    assert_eq!(last_of(&fake, "notice")["text"], hint);
    fake.events.borrow_mut().clear();
    key("right");
    assert_eq!(last_view(&fake)["grid"]["pane"], "groups");
    assert_eq!(last_of(&fake, "notice")["text"], hint);
    // Back to All channels; Yellow on every row.
    key("down");
    key("down");
    key("right");
    assert_eq!(last_view(&fake)["title"], "All channels");
    for _ in 0..3 {
        key("favorite");
        key("down");
    }
    assert_eq!(last_view(&fake)["grid"]["focused"]["favorite"], true);
    // Blue: the Favorites list in the grid, in the order added.
    key("blue");
    let view = last_view(&fake);
    assert_eq!(view["title"], "Favorites");
    assert_eq!(view["grid"]["pane"], "guide");
    assert_eq!(
        favorite_names(&fake),
        ["Example One", "Example Two", "Example Three"]
    );

    // Blue again: move mode. Down twice moves the first to the end; the
    // focus follows it. Other keys do nothing meanwhile.
    key("blue");
    assert_eq!(last_view(&fake)["reorder"], true);
    key("down");
    key("favorite");
    key("down");
    let view = last_view(&fake);
    assert_eq!(view["grid"]["rows"]["focus"], 2);
    assert_eq!(
        favorite_names(&fake),
        ["Example Two", "Example Three", "Example One"]
    );
    // Not stored until the mode ends.
    let stored = |fake: &Fake| -> Value {
        fake.kv
            .borrow()
            .iter()
            .find(|(k, _)| k.starts_with("lists:"))
            .map(|(_, v)| serde_json::from_slice(v).unwrap())
            .unwrap()
    };
    let before = stored(&fake);
    key("ok");
    assert_eq!(last_view(&fake)["reorder"], false);
    assert_eq!(last_view(&fake)["mode"], "guide", "OK only ended the move");
    let after = stored(&fake);
    assert_ne!(before["favorites"], after["favorites"]);
    assert_eq!(after["favorites"].as_array().unwrap().len(), 3);

    // Left in move mode: to the top; Back ends it too.
    key("blue");
    key("left");
    key("back");
    assert_eq!(
        favorite_names(&fake),
        ["Example One", "Example Two", "Example Three"]
    );
    assert_eq!(last_view(&fake)["grid"]["pane"], "guide");
    key("down");
    key("blue");
    key("right");
    key("blue");
    assert_eq!(
        favorite_names(&fake),
        ["Example One", "Example Three", "Example Two"]
    );
    assert_eq!(last_view(&fake)["grid"]["rows"]["focus"], 2);

    // The next launch keeps the order.
    m3u_launch(&fake);
    key("blue");
    assert_eq!(
        favorite_names(&fake),
        ["Example One", "Example Three", "Example Two"]
    );
}

#[test]
fn lists_stored_before_search_and_reordering_still_load() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    m3u_launch(&fake);
    key("down");
    key("favorite");
    // The record as the previous version wrote it: no searches.
    let (k, mut v): (String, Value) = fake
        .kv
        .borrow()
        .iter()
        .find(|(k, _)| k.starts_with("lists:"))
        .map(|(k, v)| (k.clone(), serde_json::from_slice(v).unwrap()))
        .unwrap();
    v.as_object_mut().unwrap().remove("searches");
    fake.kv.borrow_mut().insert(k, v.to_string().into_bytes());
    m3u_launch(&fake);
    key("blue");
    assert_eq!(favorite_names(&fake), ["Example Two"]);
    key("search");
    assert!(
        last_view(&fake)["search"]["rows"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_play_bar_favorites_the_playing_channel_and_back_never_leaves() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    send(serde_json::json!({
        "cmd": "source",
        "source": { "kind": "xtream", "server": "http://tv.example.com", "username": "demo", "password": "secret" },
    }));
    deliver_with(&fake, &xtream);
    // OK plays full screen; OK there shows the play bar with Favorite
    // focused.
    key("ok");
    assert_eq!(last_view(&fake)["mode"], "full");
    assert!(last_view(&fake)["bar"].is_null());
    key("ok");
    let bar = last_view(&fake)["bar"].clone();
    assert_eq!(bar["focus"], "favorite", "{bar}");
    assert_eq!(bar["favorite"], false);

    // OK adds the playing channel.
    key("ok");
    assert_eq!(last_view(&fake)["bar"]["favorite"], true);
    assert_eq!(last_of(&fake, "notice")["text"], "Added to Favorites");

    // The focus clamps at both ends; Down still zaps under the bar.
    for _ in 0..6 {
        key("right");
    }
    assert_eq!(last_view(&fake)["bar"]["focus"], "search");
    key("left");
    assert_eq!(last_view(&fake)["bar"]["focus"], "guide");
    key("down");
    let view = last_view(&fake);
    assert_eq!(view["playing"]["name"], "Demo Two");
    assert_eq!(view["bar"]["favorite"], false);

    // The page's timeout and Back hide the bar; Back again returns to the
    // guide on the playing row; Back never asks to leave.
    key("barTimeout");
    assert!(last_view(&fake)["bar"].is_null());
    key("ok");
    key("back");
    let view = last_view(&fake);
    assert_eq!(view["mode"], "full");
    assert!(view["bar"].is_null());
    key("back");
    let view = last_view(&fake);
    assert_eq!(view["mode"], "guide");
    assert_eq!(view["grid"]["rows"]["focus"], 1);

    // TV guide on the bar opens the guide on the playing channel, and the
    // bar is gone when full screen returns; Search on it opens search.
    key("ok");
    assert_eq!(last_view(&fake)["mode"], "full");
    key("ok");
    key("right");
    assert_eq!(last_view(&fake)["bar"]["focus"], "guide");
    key("ok");
    let view = last_view(&fake);
    assert_eq!(view["mode"], "guide");
    assert_eq!(view["grid"]["rows"]["focus"], 1);
    key("ok");
    assert_eq!(last_view(&fake)["mode"], "full");
    assert!(last_view(&fake)["bar"].is_null());
    key("ok");
    key("right");
    key("right");
    key("ok");
    assert_eq!(last_view(&fake)["mode"], "search");
    key("cancel");
    key("back");
    assert_eq!(last_view(&fake)["mode"], "guide");
    assert!(fake.events.borrow().iter().all(|e| e["type"] != "exit"));
}

/// Answers the playlist with `playlist` and every guide with `guide`.
fn serve(playlist: String, guide: String) -> impl Fn(&str) -> Result<(u16, String), u32> {
    move |url| {
        Ok((
            200,
            if url.ends_with(".m3u") {
                playlist.clone()
            } else {
                guide.clone()
            },
        ))
    }
}

fn launch_with(fake: &Rc<Fake>, respond: &Responder) {
    send(
        serde_json::json!({ "cmd": "source", "source": { "kind": "m3u", "url": "http://lists.example.com/list.m3u" } }),
    );
    deliver_with(fake, respond);
}

fn focused_title(fake: &Fake) -> Value {
    last_view(fake)["grid"]["focused"]["title"].clone()
}

#[test]
fn a_stream_that_fails_in_full_screen_returns_to_where_it_was_played_from() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    m3u_launch(&fake);
    // Full screen with the play bar; the stream fails: the guide, without
    // the bar (it would take keys over a black screen).
    key("ok");
    key("ok");
    assert!(!last_view(&fake)["bar"].is_null());
    send(serde_json::json!({ "cmd": "playFailed" }));
    let view = last_view(&fake);
    assert_eq!(view["mode"], "guide");
    assert!(view["playing"].is_null());
    assert!(view["bar"].is_null());
    assert_eq!(view["grid"]["pane"], "guide");

    // Played from search: back to search.
    key("search");
    send(serde_json::json!({ "cmd": "searchText", "text": "two" }));
    key("done");
    key("ok");
    assert_eq!(last_view(&fake)["mode"], "full");
    send(serde_json::json!({ "cmd": "playFailed" }));
    let view = last_view(&fake);
    assert_eq!(view["mode"], "search");
    assert!(view["playing"].is_null());
}

#[test]
fn a_channel_played_from_search_zaps_within_its_own_group() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    m3u_launch(&fake);
    // The guide shows Sport (three only).
    key("back");
    key("down");
    key("down");
    assert_eq!(last_view(&fake)["title"], "Sport");
    key("right");
    // Two, from search, is not on it: Down zaps within News (one, two).
    key("search");
    send(serde_json::json!({ "cmd": "searchText", "text": "two" }));
    key("done");
    key("ok");
    assert_eq!(
        last_of(&fake, "play")["url"],
        "http://media.example.com/two.ts"
    );
    key("down");
    assert_eq!(
        last_of(&fake, "play")["url"],
        "http://media.example.com/one.ts"
    );
}

#[test]
fn a_guide_refreshed_in_a_later_hour_keeps_the_focused_program() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    let names = ["Dawn", "Morning", "Noon", "Dusk", "Night"];
    let respond = serve(playlist(), guide_of(&["one", "two", "three"], &names));
    launch_with(&fake, &respond);
    key("right");
    key("right");
    assert_eq!(focused_title(&fake), "one Dusk");
    let start = last_view(&fake)["grid"]["focused"]["start"].clone();
    // An hour and five minutes later the guide is updated: its schedule
    // counts minutes from the new hour.
    fake.wall_offset_ms.set(65.0 * 60_000.0);
    send(serde_json::json!({ "cmd": "view" }));
    assert_eq!(focused_title(&fake), "one Dusk");
    let downloads = guide_downloads(&fake);
    send(serde_json::json!({ "cmd": "refreshGuide" }));
    deliver_with(&fake, &respond);
    assert_eq!(guide_downloads(&fake), downloads + 1, "refreshed");
    let view = last_view(&fake);
    assert_eq!(view["grid"]["focused"]["title"], "one Dusk", "{view}");
    assert_eq!(view["grid"]["focused"]["start"], start);
}

#[test]
fn the_favorites_list_follows_its_members_at_once() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    m3u_launch(&fake);
    for _ in 0..3 {
        key("favorite");
        key("down");
    }
    key("blue");
    key("down");
    assert_eq!(last_view(&fake)["grid"]["focused"]["name"], "Example Two");
    // Two is removed: the row that took its place has the focus.
    key("favorite");
    assert_eq!(favorite_names(&fake), ["Example One", "Example Three"]);
    let view = last_view(&fake);
    assert_eq!(view["grid"]["rows"]["focus"], 1);
    assert_eq!(view["grid"]["focused"]["name"], "Example Three");
    key("favorite");
    // One favorite left: Blue does not start moving it.
    assert_eq!(favorite_names(&fake), ["Example One"]);
    key("blue");
    assert_eq!(last_view(&fake)["reorder"], false);
    // Emptied: the focus goes to the groups column.
    key("favorite");
    let view = last_view(&fake);
    assert_eq!(view["grid"]["rows"]["total"], 0);
    assert_eq!(view["grid"]["pane"], "groups");
    key("blue");
    assert_eq!(last_view(&fake)["reorder"], false);
}

#[test]
fn moving_a_favorite_keeps_a_focus_on_a_past_program() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    m3u_launch(&fake);
    key("favorite");
    key("down");
    key("favorite");
    key("blue");
    key("left");
    assert_eq!(focused_title(&fake), "one Dawn");
    key("blue");
    assert_eq!(last_view(&fake)["reorder"], true);
    // Time passes until Dawn is before the default window; the moved list
    // still reaches back to it.
    fake.wall_offset_ms.set(50.0 * 60_000.0);
    key("down");
    let view = last_view(&fake);
    assert_eq!(favorite_names(&fake), ["Example Two", "Example One"]);
    assert_eq!(view["grid"]["rows"]["focus"], 1);
    assert_eq!(view["grid"]["focused"]["title"], "one Dawn", "{view}");
}

#[test]
fn a_page_keeps_the_time_past_rows_without_listings() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    // Two has no guide.
    let names = ["Dawn", "Morning", "Noon"];
    launch_with(
        &fake,
        &serve(playlist(), guide_of(&["one", "three"], &names)),
    );
    key("left");
    assert_eq!(focused_title(&fake), "one Dawn");
    key("chdown");
    let view = last_view(&fake);
    assert_eq!(view["grid"]["rows"]["focus"], 2);
    assert_eq!(view["grid"]["focused"]["title"], "three Dawn", "{view}");
    key("chup");
    assert_eq!(focused_title(&fake), "one Dawn");
}

#[test]
fn a_row_signature_includes_the_row_number() {
    use crate::grid::Grid;
    use iptv_core::guide::SyntheticSchedule;
    use iptv_core::lists::{ALL, ChannelList};
    let open = |channels: Vec<usize>| {
        Grid::open(
            Rc::new(SyntheticSchedule),
            Rc::new(ChannelList::new(ALL, channels)),
            0,
        )
    };
    let a = open((0..20).collect());
    // Channel 0 on row 8, shown in the first slot after a page down.
    let mut b = open((100..108).chain(0..20).collect());
    b.key("chdown", 0, 10, false);
    let first = |g: &Grid| g.json(None, &|_| None, &|_| false)["rows"]["items"][0].clone();
    let (a, b) = (first(&a), first(&b));
    assert_eq!(
        (a["name"].clone(), b["name"].clone()),
        ("Channel 1".into(), "Channel 1".into())
    );
    assert_eq!(
        (a["number"].clone(), b["number"].clone()),
        (1.into(), 9.into())
    );
    assert_ne!(a["sig"], b["sig"]);
}

#[test]
fn search_keeps_its_history_and_drops_stale_program_results() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    m3u_launch(&fake);
    // Back hides the keyboard first, then leaves.
    key("search");
    assert_eq!(last_view(&fake)["search"]["keyboard"], true);
    key("back");
    let view = last_view(&fake);
    assert_eq!(view["mode"], "search");
    assert_eq!(view["search"]["keyboard"], false);
    key("back");
    assert_eq!(last_view(&fake)["mode"], "guide");

    // A completed query is history at once (leaving the app keeps it).
    key("search");
    send(serde_json::json!({ "cmd": "searchText", "text": "two" }));
    key("done");
    let lists: Value = fake
        .kv
        .borrow()
        .iter()
        .find(|(k, _)| k.starts_with("lists:"))
        .map(|(_, v)| serde_json::from_slice(v).unwrap())
        .unwrap();
    assert_eq!(lists["searches"], serde_json::json!(["two"]));
    assert_eq!(last_view(&fake)["mode"], "search");

    // Program results of a guide that was replaced are not offered while
    // the new index builds.
    deliver(&fake);
    assert_eq!(last_view(&fake)["search"]["counts"]["programs"], 2);
    fake.events.borrow_mut().clear();
    send(serde_json::json!({ "cmd": "refreshGuide" }));
    deliver(&fake);
    let events = fake.events.borrow();
    let preparing: Vec<&Value> = events
        .iter()
        .filter(|e| e["type"] == "view" && e["search"]["preparing"] == true)
        .collect();
    assert!(!preparing.is_empty());
    for v in preparing {
        assert_eq!(v["search"]["counts"]["programs"], 0, "{v}");
    }
    drop(events);
    assert_eq!(last_view(&fake)["search"]["counts"]["programs"], 2);
}

#[test]
fn held_keys_in_the_groups_column_build_the_list_once_settled() {
    let fake = Rc::new(Fake::default());
    install(fake.clone());
    m3u_launch(&fake);
    // Three plays; its number is its row in the list shown.
    key("down");
    key("down");
    key("ok");
    assert_eq!(last_view(&fake)["playing"]["number"], 3);
    key("back");
    key("back");
    assert_eq!(last_view(&fake)["grid"]["pane"], "groups");
    let repeat = |k: &str| send(serde_json::json!({ "cmd": "key", "key": k, "repeat": true }));
    repeat("down");
    let view = last_view(&fake);
    assert_eq!(view["title"], "News");
    assert_eq!(view["pending"], true);
    assert_eq!(view["grid"]["rows"]["total"], 0);
    repeat("down");
    assert_eq!(last_view(&fake)["title"], "Sport");
    send(serde_json::json!({ "cmd": "settle" }));
    let view = last_view(&fake);
    assert_eq!(view["pending"], false);
    assert_eq!(view["grid"]["rows"]["total"], 1);
    assert_eq!(view["playing"]["number"], 1, "three's row in Sport");
    // A key that is not a repeat builds at once.
    repeat("up");
    key("right");
    let view = last_view(&fake);
    assert_eq!(view["title"], "News");
    assert_eq!(view["grid"]["rows"]["total"], 2);
    assert_eq!(view["grid"]["pane"], "guide");
    assert_eq!(view["playing"]["number"], 3, "not in News: its own number");
}
