//! The periodic guide check of the playlist on screen survives guide work
//! that ends without scheduling it (`data.rs`, `data/stored.rs`).
//!
//! Work on a playlist cancels its guide timer and normally ends by setting
//! the next one. What it checks: a failed refresh over a cancelled guide
//! download, a refresh whose guide download "Delete stored guide data"
//! revoked, "Update the guide now" revoked the same way, and a failed
//! "Add playlist" (which ends the timers of the playlist that stays on
//! screen) each leave a check that later really downloads the guide; a
//! revoked job's check keeps the time the job asked for, so deleted guide
//! data is not fetched again at once; a check that comes due while a
//! refresh still runs waits for it; and with automatic guide updates off
//! failed work arms no check.
//!
//! Synthetic data only: a playlist "picked" from a file in the temporary
//! home or linked from a local mock server, its guide served by another;
//! both can hold their replies back.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use iptv_store_sqlite::SourceId;
use nsh_core::ImageData;
use nsh_core::picker::PickedFile;
use nsh_testing::Headless;

use super::cov_data::{
    IMPORT, act, copy_of, enter_text, guide, playlist, rail, refreshed, replace_copy, start, step,
    store, to_guide, until,
};

/// Longer than the retry a check waits (`MIN_WAIT`, one minute).
const RETRY: Duration = Duration::from_secs(61);
/// Longer than any check waits (`MAX_WAIT`, six hours).
const LATER: Duration = Duration::from_secs(7 * 3600);

#[derive(Default)]
struct Served {
    body: String,
    held: bool,
    requests: usize,
}

/// A local server of one document whose replies can be held back, as a
/// slow one's.
#[derive(Clone)]
struct Server {
    link: String,
    state: Arc<(Mutex<Served>, Condvar)>,
}

impl Server {
    fn start(name: &str, kind: &'static str, body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let link = format!("http://{}/{name}", listener.local_addr().unwrap());
        let state = Arc::new((
            Mutex::new(Served {
                body,
                ..Served::default()
            }),
            Condvar::new(),
        ));
        let shared = state.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let shared = shared.clone();
                // A thread each: a held reply does not stop later requests.
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut buf = [0; 1024];
                    while !request.ends_with(b"\r\n\r\n") {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    }
                    let (lock, released) = &*shared;
                    let mut served = lock.lock().unwrap();
                    served.requests += 1;
                    while served.held {
                        served = released.wait(served).unwrap();
                    }
                    let body = served.body.clone();
                    drop(served);
                    let _ = stream.write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    );
                });
            }
        });
        Self { link, state }
    }

    /// Requests from now on wait for [`Self::serve`].
    fn hold(&self) {
        self.state.0.lock().unwrap().held = true;
    }

    /// Answers the held requests and later ones with `body`.
    fn serve(&self, body: String) {
        let mut served = self.state.0.lock().unwrap();
        served.body = body;
        served.held = false;
        self.state.1.notify_all();
    }

    fn requests(&self) -> usize {
        self.state.0.lock().unwrap().requests
    }
}

/// The app with one playlist on screen, its guide typed as the server's
/// link and stored.
struct Scenario {
    app: Headless,
    /// The guide's server.
    server: Server,
    /// The playlist's server, when it is linked rather than a picked file.
    playlist: Option<Server>,
    data: PathBuf,
    source: SourceId,
}

impl Scenario {
    /// `automatic`: whether the guide is updated on a timer
    /// (`epg.update_interval`); `linked`: the playlist comes from a server
    /// instead of a picked file.
    fn start(home: &crate::isolated::Home, automatic: bool, linked: bool) -> Self {
        let _ = env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Info)
            .try_init();
        let mut settings = iptv_settings::Settings::new();
        settings
            .set(
                "epg.update_on_playlist_change",
                iptv_settings::Value::Bool(true),
            )
            .unwrap();
        if !automatic {
            settings
                .set("epg.update_interval", iptv_settings::Value::ms(0))
                .unwrap();
        }
        let data = home.data_dir();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("settings.json"), settings.to_json_string()).unwrap();
        let server = Server::start(
            "guide.xml",
            "application/xml",
            guide("Alpha", &["a1", "a2"]),
        );
        let playlist = linked.then(|| Server::start("playlist.m3u", "audio/x-mpegurl", channels()));
        let mut app = start();
        app.on_pick(|_| {
            vec![PickedFile::new(
                "first.m3u",
                "audio/x-mpegurl",
                ImageData::new(channels().into_bytes()),
            )]
        });
        step(app.wait_for("text:No playlists yet"));
        step(app.press("text:Add playlist"));
        step(app.wait_for("text:Playlist file"));
        match &playlist {
            Some(playlist) => {
                let placeholder = "text:http://…/playlist.m3u";
                step(app.press("text:Playlist link"));
                step(app.wait_for(placeholder));
                step(app.press(placeholder));
                enter_text(&mut app, &playlist.link);
                step(app.press("text:Optional: how the playlist is listed"));
                enter_text(&mut app, "first");
            }
            None => {
                step(app.press("text:Playlist file"));
                step(app.wait_for("text:Choose a file…"));
                step(app.press("text:Choose a file…"));
                step(app.wait_for("text:first.m3u"));
            }
        }
        step(app.press("text:Guide link"));
        enter_text(&mut app, &server.link);
        step(app.press("text:Add playlist"));
        step(app.wait_for_within("text:Playlist added", IMPORT));
        step(app.press("text:Watch"));
        step(app.wait_for_within("text:Alpha Now", IMPORT));
        let source = store(&data).sources().unwrap()[0].id;
        let mut scenario = Self {
            app,
            server,
            playlist,
            data,
            source,
        };
        if automatic {
            scenario.until_pending("the import set the guide timer");
        } else {
            step(scenario.app.run_for(Duration::from_secs(1)));
            assert!(!scenario.pending(), "no timer without automatic updates");
        }
        scenario
    }

    /// Whether the playlist has a guide timer or a guide read under way.
    fn pending(&self) -> bool {
        iptv_app::guide_check_pending(self.source)
    }

    fn until_pending(&mut self, what: &str) {
        let source = self.source;
        until(&mut self.app, what, IMPORT, |_| {
            iptv_app::guide_check_pending(source)
        });
    }

    /// Waits for the guide request after the first `asked` to arrive.
    fn until_asked(&mut self, asked: usize) {
        let server = self.server.clone();
        until(&mut self.app, "the guide was asked for", IMPORT, |_| {
            server.requests() > asked
        });
    }

    /// From the guide: "Delete stored guide data", confirmed.
    fn delete_guide_data(&mut self) {
        rail(&mut self.app, "Settings");
        step(self.app.press("text:TV guide data"));
        step(self.app.wait_for("text:Delete stored guide data"));
        step(self.app.press("text:Delete stored guide data"));
        step(self.app.wait_for("text:Delete stored guide data?"));
        step(self.app.press("text:Delete stored guide data >> nth=1"));
        step(self.app.wait_for_within("text:Guide data deleted", IMPORT));
        to_guide(&mut self.app);
        step(self.app.wait_for_gone("text:/ Now$/", IMPORT));
    }

    /// From the guide: the playlists screen's "Refresh now".
    fn refresh(&mut self) {
        rail(&mut self.app, "Playlists");
        act(&mut self.app, "first", "Refresh now");
    }

    /// The playlist no longer parses: its refreshes fail (a linked one's
    /// held refresh among them).
    fn break_playlist(&self) {
        match &self.playlist {
            Some(playlist) => playlist.serve("not a playlist".to_owned()),
            None => {
                let copy = copy_of(&self.data, "first.m3u");
                replace_copy(&self.data, copy, "not a playlist").unwrap();
            }
        }
    }

    /// The server answers with `prefix`'s guide; a check that comes due
    /// within `wait` downloads it and the guide shows it.
    fn check_downloads(&mut self, prefix: &str, wait: Duration) {
        self.server.serve(guide(prefix, &["a1", "a2"]));
        to_guide(&mut self.app);
        step(self.app.fast_forward(wait));
        step(
            self.app
                .wait_for_within(&format!("text:{prefix} Now"), IMPORT),
        );
        self.until_pending("the check set the next one");
    }
}

fn channels() -> String {
    playlist(None, &[("a1", "Alpha One"), ("a2", "Alpha Two")])
}

/// A refresh replaces one whose guide download is still running, then
/// fails with no guide stored: neither sets the next check itself.
#[test]
fn a_failed_refresh_over_a_cancelled_guide_download_keeps_the_guide_timer() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, true, false);
    s.delete_guide_data();
    assert!(s.pending(), "deleting the guide keeps its timer");
    // The first refresh: its channels are in, its guide download waits.
    s.server.hold();
    let asked = s.server.requests();
    s.refresh();
    s.until_asked(asked);
    assert!(
        !s.pending(),
        "the refresh owns the guide: no timer meanwhile"
    );
    // The second fails before any guide work.
    s.break_playlist();
    act(&mut s.app, "first", "Refresh now");
    step(s.app.wait_for_within("text:/^Refresh failed/", IMPORT));
    s.until_pending("the failed refresh kept a guide check");
    s.check_downloads("Fresh", RETRY);
}

/// "Update the guide now", asked for during a refresh and revoked by
/// "Delete stored guide data", keeps a check; it comes due while the
/// refresh still runs and waits for it instead of ending unseen.
#[test]
fn a_guide_check_due_during_a_refresh_waits_for_it() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, true, true);
    // The refresh waits for its playlist.
    let playlist = s.playlist.clone().expect("a linked playlist");
    playlist.hold();
    let asked = playlist.requests();
    s.refresh();
    until(&mut s.app, "the playlist was asked for", IMPORT, |_| {
        playlist.requests() > asked
    });
    to_guide(&mut s.app);
    // The update waits for the guide, then loses it to the deletion.
    s.server.hold();
    let asked = s.server.requests();
    rail(&mut s.app, "Settings");
    step(s.app.press("text:TV guide data"));
    step(s.app.wait_for("text:Update the guide now"));
    step(s.app.press("text:Update the guide now"));
    s.until_asked(asked);
    assert!(
        !s.pending(),
        "the update owns the guide: no timer meanwhile"
    );
    step(s.app.press("text:Delete stored guide data"));
    step(s.app.wait_for("text:Delete stored guide data?"));
    step(s.app.press("text:Delete stored guide data >> nth=1"));
    step(s.app.wait_for_within("text:Guide data deleted", IMPORT));
    s.until_pending("the revoked update kept a guide check");
    // The check the update asked for, hours away, comes due.
    step(s.app.fast_forward(LATER));
    assert!(s.pending(), "due while the refresh runs: it waits again");
    // The refresh fails with no guide stored: it sets no check itself.
    s.break_playlist();
    to_guide(&mut s.app);
    rail(&mut s.app, "Playlists");
    step(s.app.wait_for_within("text:/^Refresh failed/", IMPORT));
    assert!(s.pending(), "the failed refresh left the waiting check");
    s.check_downloads("Again", RETRY);
}

/// "Delete stored guide data" revokes a refresh's guide download: the
/// refresh ends well, yet nothing else would set the next check. The one
/// kept is the refresh's own, hours away: the deleted guide stays deleted
/// until then.
#[test]
fn deleting_guide_data_during_a_refresh_keeps_the_guide_timer() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, true, false);
    s.server.hold();
    let asked = s.server.requests();
    s.refresh();
    s.until_asked(asked);
    // Its channels are on screen (the stored guide with them) before the
    // deletion empties the guide.
    refreshed(&mut s.app, "first");
    assert!(
        !s.pending(),
        "the refresh owns the guide: no timer meanwhile"
    );
    to_guide(&mut s.app);
    s.delete_guide_data();
    s.until_pending("the revoked refresh kept a guide check");
    s.server.serve(guide("Soon", &["a1", "a2"]));
    let asked = s.server.requests();
    step(s.app.fast_forward(RETRY));
    step(s.app.run_for(Duration::from_secs(1)));
    assert_eq!(s.server.requests(), asked, "not fetched again at once");
    assert!(s.pending());
    s.check_downloads("Later", LATER);
}

/// A wizard's Add starts a generation, which ends the timers of the
/// playlist on screen; when it fails, that playlist stays and is checked.
#[test]
fn a_failed_add_keeps_the_guide_timer_of_the_playlist_on_screen() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, true, false);
    // Nothing stored, so the check that is kept has a guide to download.
    s.delete_guide_data();
    assert!(s.pending(), "deleting the guide keeps its timer");
    let second = Server::start("second.m3u", "audio/x-mpegurl", String::new());
    second.hold();
    rail(&mut s.app, "Playlists");
    step(s.app.press("text:+ Add playlist >> nth=1"));
    let placeholder = "text:http://…/playlist.m3u";
    step(s.app.wait_for("text:Playlist link"));
    step(s.app.press("text:Playlist link"));
    step(s.app.wait_for(placeholder));
    step(s.app.press(placeholder));
    enter_text(&mut s.app, &second.link);
    step(s.app.press("text:Add playlist"));
    let asked = second.clone();
    until(&mut s.app, "the new playlist was asked for", IMPORT, |_| {
        asked.requests() > 0
    });
    assert!(!s.pending(), "the Add owns the guide: no timer meanwhile");
    second.serve("not a playlist".to_owned());
    s.until_pending("the failed Add kept the shown playlist a guide check");
    // Out of the wizard (its form, then its first page).
    for _ in 0..3 {
        let screen = s.app.screen();
        if !screen.matches(placeholder).unwrap()
            && !screen.matches("text:Playlist link").unwrap()
            && !screen.matches(&format!("text:{}", second.link)).unwrap()
        {
            break;
        }
        step(s.app.back());
    }
    s.check_downloads("Fresh", RETRY);
}

/// With automatic guide updates off, failed and replaced work arms no
/// check: the guide is downloaded on request only.
#[test]
fn manual_guide_updates_arm_no_check_after_failed_work() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, false, false);
    s.delete_guide_data();
    s.server.hold();
    let asked = s.server.requests();
    s.refresh();
    s.until_asked(asked);
    s.break_playlist();
    act(&mut s.app, "first", "Refresh now");
    step(s.app.wait_for_within("text:/^Refresh failed/", IMPORT));
    s.server.serve(guide("Fresh", &["a1", "a2"]));
    let asked = s.server.requests();
    to_guide(&mut s.app);
    step(s.app.fast_forward(RETRY));
    step(s.app.run_for(Duration::from_secs(1)));
    assert!(!s.pending(), "no check without automatic updates");
    assert_eq!(s.server.requests(), asked, "and no download");
    assert!(!s.app.screen().matches("text:Fresh Now").unwrap());
}
