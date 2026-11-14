//! The data layer, headless on a manual clock: stored playlists as the
//! playlists screen and the guide settings work on them (`data.rs`,
//! `data/library.rs`, `data/stored.rs`), and a cold start that resumes the
//! last channel once the startup refresh brought it (`player.rs`).
//!
//! What it checks: a second playlist added from the playlists screen
//! opens the combined browser; showing another one brings its stored channels
//! and guide; a background refresh changes only the store; a refresh that
//! moved the playing channel keeps it, one that removed it stops it;
//! "Refresh all"; rename; a User-Agent tried and saved, then one whose
//! check fails and is not saved; a failed refresh keeps the channels and
//! says why; a guide source switched off drops the guide, switched on
//! downloads it again; "Update the guide now", its failure, "Delete
//! stored guide data"; a cold start whose last channel (in the file of
//! older builds) arrives with the startup refresh; deleting a playlist in
//! the background, the one on screen (the newest other shows), the last
//! one (an empty, recoverable combined library).
//!
//! Synthetic data only: playlists and guides written to the temporary
//! home and "picked" from files.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iptv_epg::format_xmltv_time;
use iptv_providers::model::SourceConfig;
use iptv_store_sqlite::{SourceId, Store};
use nsh_core::picker::PickedFile;
use nsh_core::time::Clock;
use nsh_core::{ImageData, Size, ViewKind, element::Root, prelude::*};
use nsh_engine::EngineConfig;
use nsh_testing::Headless;

pub(super) const IMPORT: Duration = Duration::from_secs(20);

pub(super) fn step(r: Result<(), String>) {
    r.unwrap_or_else(|e| panic!("{e}"));
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Hour-long programmes from a day back to two days ahead for `channels`,
/// "<prefix> Now" on air now.
pub(super) fn guide(prefix: &str, channels: &[&str]) -> String {
    let now = unix_now();
    let hour = now - now.rem_euclid(3600);
    let mut xml = String::from("<?xml version=\"1.0\"?><tv>");
    for c in channels {
        xml.push_str(&format!(
            "<channel id=\"{c}\"><display-name>{c}</display-name></channel>"
        ));
    }
    for c in channels {
        let mut t = hour - 86_400;
        while t < hour + 2 * 86_400 {
            let title = if t == hour { "Now" } else { "Other" };
            xml.push_str(&format!(
                "<programme channel=\"{c}\" start=\"{}\" stop=\"{}\"><title>{prefix} {title}</title><desc>A synthetic programme.</desc></programme>",
                format_xmltv_time(t),
                format_xmltv_time(t + 3600)
            ));
            t += 3600;
        }
    }
    xml + "</tv>"
}

/// A local server answering every request with `body` (404 while it is
/// `None`); its guide link.
fn serve(body: Arc<Mutex<Option<String>>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let link = format!("http://{}/guide.xml", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            // The request, up to the end of its headers.
            let mut request = Vec::new();
            let mut buf = [0; 1024];
            while !request.ends_with(b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let reply = match body.lock().unwrap().clone() {
                Some(xml) => format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{xml}",
                    xml.len()
                ),
                None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_owned(),
            };
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    link
}

/// An M3U playlist of `channels` (tvg-id, name), its guide at `guide`.
pub(super) fn playlist(guide: Option<&Path>, channels: &[(&str, &str)]) -> String {
    let mut m3u = match guide {
        Some(g) => format!("#EXTM3U url-tvg=\"{}\"\n", g.display()),
        None => "#EXTM3U\n".to_owned(),
    };
    for (id, name) in channels {
        m3u.push_str(&format!(
            "#EXTINF:-1 tvg-id=\"{id}\" group-title=\"Synthetic\",{name}\nhttp://stream.example.test/{id}.m3u8\n"
        ));
    }
    m3u
}

/// The app's copy of picked playlist file `name`.
pub(super) fn copy_of(data: &Path, name: &str) -> PathBuf {
    std::fs::read_dir(data.join("playlists"))
        .unwrap()
        .map(|d| d.unwrap().path().join(name))
        .find(|p| p.exists())
        .unwrap_or_else(|| panic!("no copy of {name}"))
}

/// Replace synthetic picked-document content through its encrypted storage API.
pub(super) fn replace_copy(
    data: &Path,
    path: impl AsRef<Path>,
    bytes: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    let path = path.as_ref();
    let temporary = path.with_extension("replacement");
    store(data)
        .write_private_file(&temporary, &mut bytes.as_ref())
        .map_err(|_| std::io::Error::other("synthetic document write failed"))?;
    std::fs::rename(temporary, path)
}

pub(super) fn start() -> Headless {
    let size = Size::new(px(1280.0), px(720.0));
    let mut config = EngineConfig {
        viewport: size,
        clock: Clock::manual(SystemTime::now()),
        ..EngineConfig::new(Root::of::<iptv_app::App>())
    };
    config.environment.window = size;
    Headless::start(config).unwrap()
}

/// The groups, then the rail beside them, then one of its items.
pub(super) fn rail(app: &mut Headless, item: &str) {
    step(app.press("text:/^☰  /"));
    step(app.press_key("left"));
    step(app.press_key("left"));
    step(app.wait_for(&format!("text:{item} >> nth=0")));
    step(app.press(&format!("text:{item} >> nth=0")));
}

/// On the playlists screen: playlist `name`'s actions, then `act`.
pub(super) fn act(app: &mut Headless, name: &str, act: &str) {
    step(app.press(&format!("text:{name}")));
    step(app.wait_for(&format!("text:{act}")));
    step(app.press(&format!("text:{act}")));
}

/// Types `text` into the text sheet on screen in place of its text.
pub(super) fn enter_text(app: &mut Headless, text: &str) {
    let input = app
        .screen()
        .layout_nodes()
        .into_iter()
        .rev()
        .find(|n| n.kind == ViewKind::Input && !n.hidden)
        .expect("a text field")
        .id;
    let input = format!("view:{input}");
    step(app.clear(&input));
    step(app.type_text(&input, text));
    step(app.submit(&input));
}

/// Adds the next picked file through the wizard (open), then watches it.
fn add(app: &mut Headless, name: &str) {
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for(&format!("text:{name}")));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
}

/// Closes the screens over the guide, the top one first.
pub(super) fn to_guide(app: &mut Headless) {
    for _ in 0..4 {
        let open = app.screen().count("text:Close").unwrap();
        if open > 0 {
            step(app.press(&format!("text:Close >> nth={}", open - 1)));
        } else if app.screen().matches("text:TV guide data").unwrap() {
            // The settings screen: Back to its pages, then closed.
            step(app.back());
        } else {
            return;
        }
    }
    panic!("screens stay open");
}

/// Moves the clock until `done` holds (work off the UI thread takes real
/// time): `limit` of real time at most.
pub(super) fn until(
    app: &mut Headless,
    what: &str,
    limit: Duration,
    done: impl Fn(&Headless) -> bool,
) {
    let start = std::time::Instant::now();
    while !done(app) {
        assert!(start.elapsed() < limit, "{what} never happened");
        step(app.run_for(Duration::from_millis(100)));
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub(super) fn store(data: &Path) -> Store {
    crate::isolated::open_database(data.join("iptv.sqlite")).unwrap()
}

/// Whether the store notes `source`'s last guide download as failed.
fn guide_failed(data: &Path, source: SourceId) -> bool {
    store(data)
        .epg_guide(source)
        .unwrap()
        .is_some_and(|g| g.last_error.is_some())
}

/// On the playlists screen: waits until playlist `name`'s refresh is done
/// (its actions offer it again), then closes its actions.
pub(super) fn refreshed(app: &mut Headless, name: &str) {
    step(app.press(&format!("text:{name}")));
    step(app.wait_for_within("text:Refresh now", IMPORT));
    step(app.back());
}

#[test]
fn stored_playlists_switch_refresh_and_go() {
    let Some(home) = isolated!() else { return };
    // Log lines run as in the app (into the test's captured output).
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Info)
        .try_init();
    // These journeys exercise the two-step preview (OK tunes, OK again goes
    // full screen) and immediate zaps; dedicated personalization tests
    // cover the default close/delay policy.
    let mut settings = iptv_settings::Settings::new();
    for key in ["guide.stay_on_guide", "guide.stay_on_list"] {
        settings.set(key, iptv_settings::Value::Bool(true)).unwrap();
    }
    settings
        .set("guide.switch_delay", iptv_settings::Value::ms(0))
        .unwrap();
    let data = home.data_dir();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    let guide_a = home.path().join("guide-a.xml");
    std::fs::write(&guide_a, guide("Alpha", &["a1", "a2"])).unwrap();
    let guide_b = home.path().join("guide-b.xml");
    std::fs::write(&guide_b, guide("Beta", &["b1"])).unwrap();
    // The third playlist's guide, typed as a link: a local mock server.
    let guide_c: Arc<Mutex<Option<String>>> = Arc::default();
    let guide_c_link = serve(guide_c.clone());
    let alpha = |channels: &[(&str, &str)]| playlist(Some(&guide_a), channels);
    let beta = |channels: &[(&str, &str)]| playlist(Some(&guide_b), channels);
    let first = alpha(&[("a1", "Alpha One"), ("a2", "Alpha Two")]);
    let picks = Arc::new(Mutex::new(vec![
        ("third list.m3u", playlist(None, &[("c1", "Gamma One")])),
        ("second.m3u", beta(&[("b1", "Beta One")])),
        ("first.m3u", first),
    ]));
    let on_pick = move |app: &mut Headless| {
        let picks = picks.clone();
        app.on_pick(move |_| {
            let (name, body) = picks.lock().unwrap().pop().expect("a file to pick");
            vec![PickedFile::new(
                name,
                "audio/x-mpegurl",
                ImageData::new(body.into_bytes()),
            )]
        });
    };

    let mut app = start();
    on_pick(&mut app);

    // The first playlist, with its guide; Alpha Two plays in the preview.
    step(app.wait_for("text:No playlists yet"));
    step(app.press("text:Add playlist"));
    add(&mut app, "first.m3u");
    step(app.wait_for_within("text:Alpha Now", IMPORT));
    step(app.press("text:/^(★ )?Alpha Two$/"));
    assert!(iptv_app::player_has_source(0), "Alpha Two plays");
    // A favourite of the first playlist's: full screen, the card's
    // Favourite, back to the guide.
    step(app.press("text:/^(★ )?Alpha Two$/"));
    step(app.press_key("enter"));
    step(app.wait_for("id:player-actions"));
    for _ in 0..4 {
        step(app.press_key("right"));
    }
    step(app.press_key("enter"));
    step(app.wait_for("text:★ Favourite"));
    step(app.back());
    step(app.back());
    step(app.wait_for("text:★ Alpha Two"));

    // A second playlist from the playlists screen opens both sources;
    // explicit single-source browsing still stops the previous playback.
    rail(&mut app, "Playlists");
    step(app.wait_for("text:first"));
    step(app.press("text:+ Add playlist >> nth=1"));
    add(&mut app, "second.m3u");
    step(app.wait_for_within("text:Beta One · second", IMPORT));
    step(app.wait_for("text:★ Alpha Two · first"));
    // Both sources are visible. Explicit single-source actions remain
    // available for this journey's organization/background-refresh checks.
    rail(&mut app, "Playlists");
    act(&mut app, "second", "Show this playlist");
    step(app.wait_for_within("text:Beta One", IMPORT));
    assert!(!iptv_app::player_has_source(0), "playback stopped");
    assert!(
        !app.screen().matches("text:/^★/").unwrap(),
        "its favourites went"
    );
    assert!(
        data.join("last_channel-1").exists(),
        "the first's last channel"
    );

    // Showing the first again: its stored channels and guide.
    rail(&mut app, "Playlists");
    act(&mut app, "first", "Show this playlist");
    step(app.wait_for_within("text:Alpha Now", IMPORT));
    step(app.wait_for_gone("text:Beta One", IMPORT));

    // The second refreshes in the background: four channels in the store,
    // the guide on screen as it was.
    // (The one on screen: its action only goes back to the guide.)
    replace_copy(
        &data,
        copy_of(&data, "second.m3u"),
        beta(&[
            ("b1", "Beta One"),
            ("b2", "Beta Two"),
            ("b3", "Beta Three"),
            ("b4", "Beta Four"),
        ]),
    )
    .unwrap();
    rail(&mut app, "Playlists");
    act(&mut app, "first", "On screen now: back to the guide");
    step(app.wait_for_gone(
        "text:Tap a playlist for what you can do with it",
        Duration::from_secs(1),
    ));
    rail(&mut app, "Playlists");
    act(&mut app, "second", "Refresh now");
    step(app.wait_for_within("text:M3U playlist · 4 channels", IMPORT));
    to_guide(&mut app);
    assert!(app.screen().matches("text:Alpha Now").unwrap());

    // Alpha Two plays; a refresh that moves it down a row keeps it playing.
    step(app.press("text:/^(★ )?Alpha Two$/"));
    assert!(iptv_app::player_has_source(0));
    let first_copy = copy_of(&data, "first.m3u");
    replace_copy(
        &data,
        &first_copy,
        alpha(&[
            ("a0", "Alpha Zero"),
            ("a1", "Alpha One"),
            ("a2", "Alpha Two"),
        ]),
    )
    .unwrap();
    rail(&mut app, "Playlists");
    // Keep publication pending while checking the in-progress action. A local
    // import can otherwise finish before the driver observes that brief state.
    let held = super::store_latency::hold_writer(&home);
    act(&mut app, "first", "Refresh now");
    // Asked again while it runs: it runs once.
    act(&mut app, "first", "Refreshing…");
    held.execute_batch("ROLLBACK").unwrap();
    drop(held);
    step(app.wait_for_within("text:M3U playlist · 3 channels", IMPORT));
    to_guide(&mut app);
    step(app.wait_for("text:Alpha Zero"));
    assert!(iptv_app::player_has_source(0), "still playing");
    // One that leaves fewer rows than its row was: found again by key, on
    // its new row, it plays on (regression: the new guide dropped it).
    replace_copy(
        &data,
        &first_copy,
        alpha(&[("a1", "Alpha One"), ("a2", "Alpha Two")]),
    )
    .unwrap();
    rail(&mut app, "Playlists");
    act(&mut app, "first", "Refresh now");
    step(app.wait_for_within("text:M3U playlist · 2 channels", IMPORT));
    to_guide(&mut app);
    step(app.wait_for_gone("text:Alpha Zero", IMPORT));
    step(app.run_for(Duration::from_secs(1)));
    assert!(iptv_app::player_has_source(0), "Alpha Two plays on");

    // "Refresh all" with Alpha Two gone from the first: playback stops.
    replace_copy(
        &data,
        &first_copy,
        alpha(&[("a0", "Alpha Zero"), ("a1", "Alpha One")]),
    )
    .unwrap();
    rail(&mut app, "Playlists");
    step(app.press("text:Refresh all playlists"));
    step(app.wait_for_within("text:M3U playlist · 2 channels", IMPORT));
    until(&mut app, "playback stopped", IMPORT, |_| {
        !iptv_app::player_has_source(0)
    });

    // Rename the second.
    act(&mut app, "second", "Rename");
    enter_text(&mut app, "Renamed list");
    step(app.wait_for("text:Renamed list"));

    // A User-Agent for the first: checked by importing with it, then saved.
    let first_id = store(&data).sources().unwrap()[0].id;
    let agent = |data: &Path| match store(data).source(first_id).unwrap().unwrap().source.config {
        SourceConfig::M3u(c) => c.user_agent,
        _ => None,
    };
    act(&mut app, "first", "User-Agent");
    enter_text(&mut app, "SyntheticAgent/1.0");
    until(&mut app, "the agent saved", IMPORT, |_| {
        agent(&data).as_deref() == Some("SyntheticAgent/1.0")
    });
    // With the file broken, another one fails its check: not saved.
    replace_copy(&data, &first_copy, "not a playlist").unwrap();
    act(&mut app, "first", "User-Agent");
    enter_text(&mut app, "OtherAgent/2.0");
    step(app.wait_for_within("text:/^New details not saved/", IMPORT));
    assert_eq!(agent(&data).as_deref(), Some("SyntheticAgent/1.0"));
    // A refresh fails too: the channels stay, the row says why; so does
    // one of an empty playlist.
    act(&mut app, "first", "Refresh now");
    step(app.wait_for_within("text:/^Refresh failed/", IMPORT));
    replace_copy(&data, &first_copy, "#EXTM3U\n").unwrap();
    act(&mut app, "first", "Refresh now");
    step(app.wait_for_within("text:/nothing to watch in it/", IMPORT));
    // The playlist back, its guide's file gone: the refresh keeps the
    // stored guide and notes the failure.
    replace_copy(
        &data,
        &first_copy,
        alpha(&[("a0", "Alpha Zero"), ("a1", "Alpha One")]),
    )
    .unwrap();
    std::fs::rename(&guide_a, home.path().join("away.xml")).unwrap();
    act(&mut app, "first", "Refresh now");
    until(&mut app, "the guide's failure noted", IMPORT, |_| {
        guide_failed(&data, first_id)
    });
    std::fs::rename(home.path().join("away.xml"), &guide_a).unwrap();

    // Its guide source switched off: once the edits settle the guide is
    // checked and goes; a refresh keeps it gone, and "Update the guide
    // now" has nothing to download.
    let provider = "text:/The provider's guide$/";
    let switch = |app: &mut Headless, playlist: &str, to: &str| {
        act(app, playlist, "Guide sources");
        step(app.wait_for(provider));
        step(app.press(provider));
        step(app.wait_for(&format!("text:{to}")));
        step(app.press(&format!("text:{to}")));
        to_guide(app);
    };
    switch(&mut app, "first", "Switch off");
    step(app.run_for(Duration::from_secs(3)));
    step(app.wait_for_gone("text:Alpha Now", IMPORT));
    rail(&mut app, "Playlists");
    act(&mut app, "first", "Refresh now");
    refreshed(&mut app, "first");
    to_guide(&mut app);
    assert!(!app.screen().matches("text:Alpha Now").unwrap());
    rail(&mut app, "Settings");
    step(app.press("text:TV guide data"));
    step(app.wait_for("text:Update the guide now"));
    step(app.press("text:Update the guide now"));
    step(app.wait_for("text:This playlist has no guide source switched on"));
    to_guide(&mut app);
    // Another playlist's guide sources change: its guide is looked at
    // when it is shown, not now.
    rail(&mut app, "Playlists");
    switch(&mut app, "Renamed list", "Switch off");
    // Switched on again, the first's guide is downloaded again.
    rail(&mut app, "Playlists");
    act(&mut app, "first", "Guide sources");
    step(app.wait_for(provider));
    step(app.press(provider));
    step(app.wait_for("text:Switch on"));
    step(app.press("text:Switch on"));
    to_guide(&mut app);
    step(app.run_for(Duration::from_secs(3)));
    step(app.wait_for_within("text:Alpha Now", IMPORT));

    // Settings: "Update the guide now" downloads it again.
    std::fs::write(&guide_a, guide("Fresh", &["a1", "a2"])).unwrap();
    rail(&mut app, "Settings");
    step(app.press("text:TV guide data"));
    step(app.wait_for("text:Update the guide now"));
    step(app.press("text:Update the guide now"));
    step(app.wait_for_within("text:Fresh Now", IMPORT));
    // Without its file the update fails; the stored guide stays.
    std::fs::remove_file(&guide_a).unwrap();
    step(app.press("text:Update the guide now"));
    step(app.wait_for_within("text:/^Failed /", IMPORT));
    assert!(app.screen().matches("text:Fresh Now").unwrap());
    // "Delete stored guide data": the channels stay without programmes.
    step(app.press("text:Delete stored guide data"));
    // Its confirmation's button.
    step(app.wait_for("text:Delete stored guide data?"));
    step(app.press("text:Delete stored guide data >> nth=1"));
    step(app.wait_for_within("text:Guide data deleted", IMPORT));
    step(app.wait_for_gone("text:Fresh Now", IMPORT));
    to_guide(&mut app);
    // The next periodic check (hours later) finds no guide stored and
    // downloads it, which fails without its file: noted in the store.
    step(app.fast_forward(Duration::from_secs(7 * 3600)));
    until(&mut app, "the timer's failure noted", IMPORT, |_| {
        guide_failed(&data, first_id)
    });
    // With the file back, "Update the guide now" brings it, and the checks
    // start again.
    std::fs::write(&guide_a, guide("Again", &["a1", "a2"])).unwrap();
    rail(&mut app, "Settings");
    step(app.press("text:TV guide data"));
    step(app.wait_for("text:Update the guide now"));
    step(app.press("text:Update the guide now"));
    step(app.wait_for_within("text:Again Now", IMPORT));
    to_guide(&mut app);
    drop(app);

    // A cold start: the last channel, kept in the file older builds used,
    // is not stored yet; the startup refresh brings it and it plays.
    std::fs::write(&guide_a, guide("Alpha", &["a1", "a9"])).unwrap();
    let nine = alpha(&[("a1", "Alpha One"), ("a9", "Alpha Nine")]);
    replace_copy(&data, &first_copy, &nine).unwrap();
    let key = iptv_providers::m3u::parse_m3u(&nine).channels.items[1]
        .id
        .as_str()
        .to_owned();
    let _ = std::fs::remove_file(data.join(format!("last_channel-{}", first_id.0)));
    std::fs::write(data.join("last_channel"), key).unwrap();
    let mut app = start();
    on_pick(&mut app);
    step(app.wait_for_within("id:player-banner >> text:/Alpha Nine · first$/", IMPORT));
    assert!(iptv_app::player_has_source(0), "Alpha Nine plays");
    drop(app);

    // A saved combined key resumes the owning source's channel, with its
    // banner up full screen when the cached catalogue arrives.
    let one = iptv_providers::m3u::parse_m3u(&nine).channels.items[0]
        .id
        .as_str()
        .to_owned();
    std::fs::write(data.join(format!("last_channel-{}", first_id.0)), &one).unwrap();
    std::fs::write(
        data.join("last_channel_combined"),
        format!("source:{}:{one}", first_id.0),
    )
    .unwrap();
    let mut app = start();
    step(app.wait_for_within("id:player-banner >> text:/Alpha One · first$/", IMPORT));
    assert!(iptv_app::player_has_source(0), "Alpha One plays");
    drop(app);

    // Startup playback disabled: the combined guide, nothing playing.
    let _ = std::fs::remove_file(data.join(format!("last_channel-{}", first_id.0)));
    let _ = std::fs::remove_file(data.join("last_channel"));
    settings
        .set(
            "general.resume_last_channel",
            iptv_settings::Value::Bool(false),
        )
        .unwrap();
    std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
    let mut app = start();
    on_pick(&mut app);
    step(app.wait_for_within("text:Alpha Now", IMPORT));
    step(app.run_for(Duration::from_secs(1)));
    assert!(!iptv_app::player_has_source(0), "nothing plays");

    // A third playlist, shown; then deleting: the second in the
    // background, the third on screen (the first shows), the last one.
    // Its guide typed in the form must load: first a link that does not,
    // then one that does.
    rail(&mut app, "Playlists");
    step(app.press("text:+ Add playlist >> nth=1"));
    step(app.wait_for("text:Playlist file"));
    step(app.press("text:Playlist file"));
    step(app.wait_for("text:Choose a file…"));
    step(app.press("text:Choose a file…"));
    step(app.wait_for("text:/^third.list\\.m3u$/"));
    step(app.press("text:Guide link"));
    enter_text(&mut app, &guide_c_link);
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:/^The guide link did not load/", IMPORT));
    *guide_c.lock().unwrap() = Some(guide("Gamma", &["c1"]));
    step(app.press("text:Add playlist"));
    step(app.wait_for_within("text:Playlist added", IMPORT));
    step(app.press("text:Watch"));
    step(app.wait_for_within("text:Gamma Now", IMPORT));
    rail(&mut app, "Playlists");
    act(&mut app, "Renamed list", "Delete");
    step(app.press("text:Delete"));
    step(app.wait_for_gone("text:Renamed list", Duration::from_secs(2)));
    act(&mut app, "third_list", "Delete");
    step(app.press("text:Delete"));
    step(app.wait_for_within("text:Alpha One · first", IMPORT));
    act(&mut app, "first", "Delete");
    step(app.press("text:Delete"));
    step(app.wait_for_within(
        "text:M3U link or file, Xtream Codes login, Stalker portal",
        IMPORT,
    ));
    step(app.wait_for_gone("id:guide-channel-name", IMPORT));
    assert_eq!(app.screen().count("id:guide-channel-name").unwrap(), 0);
    assert!(store(&data).sources().unwrap().is_empty());
    assert!(!data.join(format!("last_channel-{}", first_id.0)).exists());
    drop(app);

    // A store that cannot be opened: the first-run wizard all the same.
    for f in ["iptv.sqlite", "iptv.sqlite-wal", "iptv.sqlite-shm"] {
        let _ = std::fs::remove_file(data.join(f));
    }
    std::fs::create_dir_all(data.join("iptv.sqlite")).unwrap();
    let mut app = start();
    step(app.wait_for_within("text:No playlists yet", IMPORT));
}
