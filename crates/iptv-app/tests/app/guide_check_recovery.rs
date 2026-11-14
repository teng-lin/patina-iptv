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
//! Work the refresh workers refuse (their queue is full) is among it: a
//! timer's check comes back quietly a minute later, an update asked for
//! says so and keeps a check. With guide updates on request only, nothing
//! is made up, yet the check the refresh policy had set itself (the
//! back-off after a failed download) survives work that cancelled it and
//! then could not run.
//!
//! Guide actions and the work they interrupt, in the order that matters:
//! a refresh that reports its channels after "Delete stored guide data"
//! does not show the deleted programmes again; neither does a timer's
//! download that was under way; and a channel's guide chosen while "Update
//! the guide now" runs is downloaded by an update of its own, not taken
//! for the one it cancelled.
//!
//! Synthetic data only: a playlist "picked" from a file in the temporary
//! home or linked from a local mock server, its guide served by another;
//! both can hold their replies back.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::headless::Headless;
use iptv_store_sqlite::SourceId;
use nsh_core::ImageData;
use nsh_core::picker::PickedFile;

use super::cov_data::{
    IMPORT, act, copy_of, enter_text, guide, playlist, rail, refreshed, replace_copy, start, step,
    store, to_guide, until,
};

/// Longer than the retry a check waits (`MIN_WAIT`, one minute).
const RETRY: Duration = Duration::from_secs(61);
/// Longer than any check waits (`MAX_WAIT`, six hours).
const LATER: Duration = Duration::from_secs(7 * 3600);
/// Just past the longest wait: a check set meanwhile is not due yet.
const LONGEST: Duration = Duration::from_secs(6 * 3600 + 1);
/// Just past the wait after a failed download (the policy's back-off)
/// and of a check that is still due: an hour each.
const BACKOFF: Duration = Duration::from_secs(3601);
const BUSY: &str = "text:Too many playlists are updating. Try again shortly.";

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

    /// From the guide: Settings, its TV guide data page.
    fn guide_settings(&mut self) {
        rail(&mut self.app, "Settings");
        step(self.app.press("text:TV guide data"));
        step(self.app.wait_for("text:Update the guide now"));
    }

    /// From the guide: the settings' "Update the guide now".
    fn update_now(&mut self) {
        self.guide_settings();
        step(self.app.press("text:Update the guide now"));
    }

    /// From the guide: the first channel's "Choose guide data…", `row`.
    fn choose_guide(&mut self, row: &str) {
        // The groups, then back to the channels beside them.
        step(self.app.press("text:/^☰  /"));
        step(self.app.press_key("right"));
        step(self.app.press_key("m"));
        step(self.app.wait_for("text:Choose guide data…"));
        step(self.app.press("text:Choose guide data…"));
        step(self.app.wait_for("text:/^Guide data for /"));
        step(self.app.press(&format!("text:{row}")));
    }

    /// From the guide: the wizard's "Add playlist" with `link`, submitted.
    fn add_playlist(&mut self, link: &str) {
        rail(&mut self.app, "Playlists");
        step(self.app.press("text:+ Add playlist >> nth=1"));
        let placeholder = "text:http://…/playlist.m3u";
        step(self.app.wait_for("text:Playlist link"));
        step(self.app.press("text:Playlist link"));
        step(self.app.wait_for(placeholder));
        step(self.app.press(placeholder));
        enter_text(&mut self.app, link);
        step(self.app.press("text:Add playlist"));
    }

    /// Out of the wizard after a failed Add of `link` (its form, then its
    /// first page), and back to the guide.
    fn leave_wizard(&mut self, link: &str) {
        for _ in 0..3 {
            let screen = self.app.screen();
            if !screen.matches("text:http://…/playlist.m3u").unwrap()
                && !screen.matches("text:Playlist link").unwrap()
                && !screen.matches(&format!("text:{link}")).unwrap()
            {
                break;
            }
            step(self.app.back());
        }
        to_guide(&mut self.app);
    }

    /// What the store notes of the playlist's guide.
    fn stored_guide(&self) -> Option<iptv_store_sqlite::StoredGuide> {
        store(&self.data).epg_guide(self.source).unwrap()
    }

    fn shows(&self, selector: &str) -> bool {
        self.app.screen().matches(selector).unwrap()
    }

    /// Gives work off the UI thread a second of real time to arrive, for
    /// what must not happen.
    fn settle(&mut self) {
        for _ in 0..40 {
            step(self.app.run_for(Duration::from_millis(50)));
            std::thread::sleep(Duration::from_millis(25));
        }
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

/// A timer's check the workers refuse comes back a minute later, without
/// a message each time.
#[test]
fn a_refused_guide_check_comes_back_quietly() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, true, false);
    // Nothing stored, so the check has a guide to download.
    s.delete_guide_data();
    let asked = s.server.requests();
    iptv_app::refuse_work(1);
    step(s.app.fast_forward(LONGEST));
    step(s.app.run_for(Duration::from_secs(1)));
    assert!(s.pending(), "the refused check waits");
    assert_eq!(s.server.requests(), asked, "refused: nothing downloaded");
    assert!(!s.shows(BUSY), "a background check says nothing");
    s.check_downloads("Fresh", RETRY);
}

/// "Update the guide now" that the workers refuse says so, and the timer
/// it cancelled is replaced by a check.
#[test]
fn a_refused_update_says_so_and_keeps_the_guide_timer() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, true, false);
    s.delete_guide_data();
    let asked = s.server.requests();
    iptv_app::refuse_work(1);
    s.update_now();
    step(s.app.wait_for_within(BUSY, IMPORT));
    s.until_pending("the refused update kept a guide check");
    assert_eq!(s.server.requests(), asked, "refused: nothing downloaded");
    s.check_downloads("Fresh", RETRY);
}

/// With guide updates on request only, the refresh policy still sets a
/// check itself after a failed download: its back-off, whatever the
/// interval. An update the workers refuse cancels that check and cannot
/// answer for it: the check stays, at its own time, and nothing else is
/// made up once it ran.
#[test]
fn manual_guide_updates_keep_the_backoff_that_refused_work_cancelled() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, false, false);
    s.delete_guide_data();
    // The download fails: the policy backs off.
    s.server.serve("not a guide".to_owned());
    s.update_now();
    s.until_pending("the failed download set its back-off");
    to_guide(&mut s.app);
    iptv_app::refuse_work(1);
    s.update_now();
    step(s.app.wait_for_within(BUSY, IMPORT));
    s.until_pending("the refused update left the back-off");
    // So does a refresh the workers refuse, which also starts a new
    // generation of timers.
    to_guide(&mut s.app);
    iptv_app::refuse_work(1);
    s.refresh();
    step(
        s.app
            .wait_for_within("text:/^Refresh failed: too many/", IMPORT),
    );
    s.until_pending("the refused refresh left the back-off");
    // Not sooner than the policy asked for.
    s.server.serve(guide("Fresh", &["a1", "a2"]));
    let asked = s.server.requests();
    to_guide(&mut s.app);
    step(s.app.fast_forward(RETRY));
    step(s.app.run_for(Duration::from_secs(1)));
    assert_eq!(s.server.requests(), asked, "the back-off is an hour");
    assert!(s.pending());
    // The hour itself goes by the wall clock, which the test cannot move:
    // the noted failure is deleted instead (with the stored guide data),
    // so that the check finds the guide never fetched when it comes due.
    s.delete_guide_data();
    assert!(s.pending(), "deleting the guide data keeps the check");
    step(s.app.fast_forward(BACKOFF));
    step(s.app.wait_for_within("text:Fresh Now", IMPORT));
    let source = s.source;
    until(&mut s.app, "the check answered", IMPORT, |_| {
        !iptv_app::guide_check_pending(source)
    });
    step(s.app.fast_forward(LATER));
    step(s.app.run_for(Duration::from_secs(1)));
    assert!(!s.pending(), "no periodic check with updates on request");
}

/// A refresh reads its stored guide, then reports its channels with it.
/// "Delete stored guide data" in between revokes the refresh: its report
/// still shows the channels, with the guide as stored now, not the
/// programmes that were just deleted.
#[test]
fn a_refresh_reporting_after_the_guide_was_deleted_does_not_show_it_again() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, true, false);
    // The refresh has read the guide and waits to report.
    iptv_app::hold_reports(true);
    s.refresh();
    until(&mut s.app, "the refresh read its guide", IMPORT, |_| {
        iptv_app::reports_waiting() > 0
    });
    to_guide(&mut s.app);
    assert!(s.shows("text:Alpha Now"), "the stored guide is on screen");
    s.delete_guide_data();
    let asked = s.server.requests();
    iptv_app::hold_reports(false);
    rail(&mut s.app, "Playlists");
    refreshed(&mut s.app, "first");
    to_guide(&mut s.app);
    s.settle();
    assert!(
        !s.shows("text:/ Now$/"),
        "the deleted programmes came back with the refresh's report"
    );
    assert!(
        store(&s.data).epg_guide(s.source).unwrap().is_none(),
        "and nothing was stored again"
    );
    assert_eq!(s.server.requests(), asked, "its guide download was revoked");
    s.until_pending("the revoked refresh kept a guide check");
}

/// A timer's check is downloading the guide when "Delete stored guide
/// data" is confirmed: the download is stopped instead of storing and
/// showing the guide again a moment later, and the check stays owed.
#[test]
fn deleting_guide_data_stops_a_guide_check_that_is_downloading() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, true, false);
    // Changed storage settings: the next check downloads the guide again.
    s.guide_settings();
    step(s.app.press("text:Keep programme descriptions"));
    to_guide(&mut s.app);
    s.server.hold();
    let asked = s.server.requests();
    step(s.app.fast_forward(LONGEST));
    s.until_asked(asked);
    s.delete_guide_data();
    s.server.serve(guide("Again", &["a1", "a2"]));
    s.settle();
    assert!(
        !s.shows("text:/ Now$/"),
        "the stopped download showed the guide again"
    );
    assert!(
        store(&s.data).epg_guide(s.source).unwrap().is_none(),
        "and nothing was stored again"
    );
    assert!(s.pending(), "the stopped check stays owed");
    let asked = s.server.requests();
    step(s.app.fast_forward(RETRY));
    step(s.app.run_for(Duration::from_secs(1)));
    assert_eq!(s.server.requests(), asked, "not fetched again at once");
    s.check_downloads("Later", BACKOFF);
}

/// A channel's guide is chosen while "Update the guide now" downloads.
/// The choice cancels that update, which read the store before it; the
/// download that applies the choice is an update of its own, started once
/// the cancelled one has unwound, not hours later at the next check.
#[test]
fn a_guide_chosen_during_an_update_is_downloaded_after_it() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, true, false);
    // A choice to go back from: its channel shows no programmes.
    s.choose_guide("No guide data");
    step(s.app.wait_for_within("text:Guide data changed", IMPORT));
    step(s.app.wait_for_gone("text:/^Guide data for /", IMPORT));
    // The update waits for the guide.
    s.server.hold();
    let asked = s.server.requests();
    s.update_now();
    s.until_asked(asked);
    to_guide(&mut s.app);
    // Automatic again needs the guide's full channel lists: a download.
    s.choose_guide("Automatic");
    step(
        s.app
            .wait_for_within("text:Saved; the guide is downloading to show it", IMPORT),
    );
    s.until_asked(asked + 1);
    s.server.serve(guide("Chosen", &["a1", "a2"]));
    step(s.app.wait_for_gone("text:/^Guide data for /", IMPORT));
    // Both channels show it: the one that had no guide data again too.
    let shown = |app: &Headless| app.screen().count("text:Chosen Now").unwrap() >= 2;
    until(
        &mut s.app,
        "both channels show the new guide",
        IMPORT,
        shown,
    );
    s.until_pending("the update set the next check");
}

/// "Update the guide now" finds nothing to download (its guide sources
/// went meanwhile): it says so and ends, instead of staying "Updating…"
/// with every later request joining it.
#[test]
fn an_update_without_guide_sources_ends() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, true, false);
    // Gone from the store while the app still lists them.
    let mut direct = store(&s.data);
    direct.set_guide_sources(s.source, &[]).unwrap();
    direct.delete_epg(s.source).unwrap();
    drop(direct);
    s.update_now();
    step(
        s.app
            .wait_for_within("text:No guide sources are configured", IMPORT),
    );
    step(s.app.wait_for_gone("text:Updating…", IMPORT));
    s.until_pending("the update that could not run kept a guide check");
}

/// With guide updates on request only: an update fails while an Add runs,
/// which has started a new generation of timers, so the back-off the
/// update asks for cannot be set. The Add fails and leaves the playlist on
/// screen: that back-off is what it is owed.
#[test]
fn manual_guide_updates_keep_a_backoff_asked_for_during_a_failed_add() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, false, false);
    s.delete_guide_data();
    // The update waits for the guide; the Add for its playlist.
    s.server.hold();
    let asked = s.server.requests();
    s.update_now();
    s.until_asked(asked);
    to_guide(&mut s.app);
    let second = Server::start("second.m3u", "audio/x-mpegurl", String::new());
    second.hold();
    s.add_playlist(&second.link);
    let adding = second.clone();
    until(&mut s.app, "the new playlist was asked for", IMPORT, |_| {
        adding.requests() > 0
    });
    assert!(!s.pending(), "no timer so far");
    // The update fails under the Add's generation: its back-off waits.
    s.server.serve("not a guide".to_owned());
    let (data, source) = (s.data.clone(), s.source);
    until(&mut s.app, "the failed download was noted", IMPORT, |_| {
        let noted = store(&data).epg_guide(source).unwrap();
        noted.is_some_and(|guide| guide.last_error.is_some())
    });
    s.settle();
    assert!(!s.pending(), "not set under the Add's generation");
    second.serve("not a playlist".to_owned());
    s.until_pending("the failed Add left the back-off the update asked for");
    s.leave_wizard(&second.link);
    // A timer, not the read of the stored guide that follows a failed Add.
    s.settle();
    assert!(s.pending(), "the back-off stays");
}

/// With guide updates on request only: a back-off is set; an update
/// cancels it, and an Add begins a new generation while it downloads. The
/// update succeeds: the policy asks for no check any more. The Add fails:
/// the playlist left on screen gets no check back, and shows the guide
/// the update stored, which could not be shown under the Add's generation.
#[test]
fn manual_guide_updates_keep_no_check_the_update_answered_during_a_failed_add() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, false, false);
    s.delete_guide_data();
    s.server.serve("not a guide".to_owned());
    s.update_now();
    s.until_pending("the failed download set its back-off");
    to_guide(&mut s.app);
    s.server.hold();
    let asked = s.server.requests();
    s.update_now();
    s.until_asked(asked);
    to_guide(&mut s.app);
    let second = Server::start("second.m3u", "audio/x-mpegurl", String::new());
    second.hold();
    s.add_playlist(&second.link);
    let adding = second.clone();
    until(&mut s.app, "the new playlist was asked for", IMPORT, |_| {
        adding.requests() > 0
    });
    s.server.serve(guide("Fresh", &["a1", "a2"]));
    let (data, source) = (s.data.clone(), s.source);
    until(&mut s.app, "the guide was stored", IMPORT, |_| {
        let noted = store(&data).epg_guide(source).unwrap();
        noted.is_some_and(|guide| guide.last_error.is_none() && guide.replaced_at.is_some())
    });
    s.settle();
    second.serve("not a playlist".to_owned());
    s.leave_wizard(&second.link);
    step(s.app.wait_for_within("text:Fresh Now", IMPORT));
    s.settle();
    assert!(!s.pending(), "the update answered: no check is owed");
    assert!(
        s.stored_guide()
            .is_some_and(|guide| guide.last_error.is_none())
    );
}

/// With guide updates on request only: a back-off is set; a refresh of the
/// playlist cancels it, and an Add begins a new generation while the
/// refresh waits for its playlist. The refresh then fails without a word
/// about the guide (its playlist is empty), under a generation that can
/// set no timer any more. The Add fails too: the playlist left on screen
/// still has its back-off.
#[test]
fn manual_guide_updates_keep_the_backoff_across_a_failed_refresh_and_a_failed_add() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, false, true);
    s.delete_guide_data();
    s.server.serve("not a guide".to_owned());
    s.update_now();
    s.until_pending("the failed download set its back-off");
    to_guide(&mut s.app);
    // The refresh waits for its playlist; the Add for its own.
    let playlist = s.playlist.clone().expect("a linked playlist");
    playlist.hold();
    let asked = playlist.requests();
    s.refresh();
    until(&mut s.app, "the playlist was asked for", IMPORT, |_| {
        playlist.requests() > asked
    });
    to_guide(&mut s.app);
    let second = Server::start("second.m3u", "audio/x-mpegurl", String::new());
    second.hold();
    s.add_playlist(&second.link);
    let adding = second.clone();
    until(&mut s.app, "the new playlist was asked for", IMPORT, |_| {
        adding.requests() > 0
    });
    // The refresh fails under the Add's generation.
    playlist.serve("#EXTM3U\n".to_owned());
    let (data, source) = (s.data.clone(), s.source);
    until(&mut s.app, "the failed refresh was noted", IMPORT, |_| {
        let sources = store(&data).sources().unwrap();
        let noted = sources.iter().find(|stored| stored.id == source);
        noted.is_some_and(|stored| stored.import.last_error.is_some())
    });
    s.settle();
    assert!(!s.pending(), "not set under the Add's generation");
    second.serve("not a playlist".to_owned());
    s.until_pending("the failed Add left the playlist its back-off");
    s.leave_wizard(&second.link);
    s.settle();
    assert!(s.pending(), "the back-off stays");
}

/// With guide updates on request only: a back-off is set; an update
/// cancels it and is still reading its guide sources (the store's writer
/// is busy) when an Add begins a new generation and fails at once. The
/// update is then refused by the workers: it ends without an answer under
/// its older generation, where no timer can be set. Its recovery must
/// leave what the playlist is owed to the failed Add's, which waits for it.
#[test]
fn manual_guide_updates_keep_the_backoff_when_work_ends_under_an_older_generation() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, false, false);
    s.delete_guide_data();
    s.server.serve("not a guide".to_owned());
    s.update_now();
    s.until_pending("the failed download set its back-off");
    let asked = s.server.requests();
    // The update waits for the store; the workers refuse the Add, then it.
    iptv_app::occupy_writer(Duration::from_secs(3));
    iptv_app::refuse_work(2);
    step(s.app.press("text:Update the guide now"));
    to_guide(&mut s.app);
    let second = Server::start("second.m3u", "audio/x-mpegurl", String::new());
    s.add_playlist(&second.link);
    step(s.app.wait_for_within(
        "text:/^Too many playlists are updating. Try again when/",
        IMPORT,
    ));
    assert!(!s.pending(), "the update owns the guide meanwhile");
    s.until_pending("the failed Add left the playlist its back-off");
    s.leave_wizard(&second.link);
    s.settle();
    assert!(s.pending(), "the back-off stays");
    assert_eq!(s.server.requests(), asked, "refused: nothing downloaded");
    assert_eq!(
        second.requests(),
        0,
        "refused: the playlist was not asked for"
    );
}

/// After a failed Add the app reads the stored guide of the playlist left
/// on screen again by itself. When the workers refuse that read, nobody
/// is told: no update was asked for, and the playlist keeps its check.
#[test]
fn a_refused_restore_after_a_failed_add_says_nothing() {
    let Some(home) = isolated!() else { return };
    let mut s = Scenario::start(&home, true, false);
    s.delete_guide_data();
    let second = Server::start("second.m3u", "audio/x-mpegurl", String::new());
    second.hold();
    s.add_playlist(&second.link);
    let adding = second.clone();
    until(&mut s.app, "the new playlist was asked for", IMPORT, |_| {
        adding.requests() > 0
    });
    // The next work the app asks of its workers is that read.
    iptv_app::refuse_work(1);
    second.serve("not a playlist".to_owned());
    s.until_pending("the failed Add kept the shown playlist a guide check");
    s.settle();
    assert!(!s.shows(BUSY), "a read nobody asked for says nothing");
    assert!(s.pending());
    s.leave_wizard(&second.link);
    s.check_downloads("Fresh", RETRY);
}
