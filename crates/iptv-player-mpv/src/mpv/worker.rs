//! The thread that talks to mpv. GTK's main thread renders mpv's video, and
//! mpv's render API forbids blocking mpv calls on the render thread (the
//! core may wait for a frame while the call waits for the core: the UI
//! froze up to a second when zapping). So every command, property and event
//! goes through this thread; the main thread only queues [`Cmd`]s and reads
//! [`Note`]s.
//!
//! Live streams play on across a connection the server closes ([`Bridge`]):
//! providers cut live HLS (a playlist reload refused once the redirect's
//! token expired) and MPEG-TS (the connection closed) every minute or so.
//! Once a live entry plays, the same address is appended as a spare entry;
//! mpv's `prefetch-playlist` opens it as soon as the current one has been
//! read to its end, while the buffered rest still plays, and goes on to it
//! without a gap. ffmpeg's `reconnect_streamed`/`reconnect_at_eof` are not
//! used for this: they re-request every HLS playlist and segment served
//! without a length, so live HLS falls behind and stalls.

use std::ffi::{CStr, CString, c_int};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::ffi;
use super::handle::Mpv;

/// mpv's warning level: what the failure clues read.
const LOG_WARN: c_int = 30;

/// A live entry that played shorter than this before it ended is not
/// bridged: its end goes to the app, whose retries back off.
const MIN_BRIDGED: Duration = Duration::from_secs(5);

/// ffmpeg's options for opening a spare entry ([`Bridge::restarted`]).
const SPARE_OPTIONS: &str = "live_start_index=-2";

/// What the main thread asks of mpv.
pub enum Cmd {
    /// Disable automatic handoff for a test of the app's own retry path.
    /// Queue before the first load; other players retain their usual bridge.
    #[cfg(all(test, target_os = "linux"))]
    DisableBridge,
    /// Ends the thread as a bug in it would (a test of the app noticing).
    #[cfg(all(test, target_os = "linux"))]
    Panic,
    /// `loadfile <url> replace`, from the default tracks (video and audio
    /// automatic, no subtitles) with `options` set first
    /// ([`super::map::load_options`]); answered by [`Note::Loading`]. A
    /// `live` stream is bridged across connections the server closes.
    Load {
        public_only: bool,
        url: String,
        live: bool,
        options: Vec<(&'static str, String)>,
    },
    Stop,
    Set(&'static str, String),
    /// The HTTP options for the next load.
    Http {
        source: Option<String>,
        user_agent: String,
        fields: Vec<String>,
    },
    /// Answered by [`Note::Position`] when mpv knows it.
    Position,
    /// Answered by [`Note::Fps`] and [`Note::Channels`], as far as known
    /// (an unchanged value is not observed again for the next file).
    Format,
}

/// What mpv said, as owned values (in mpv's order).
pub enum Note {
    /// The playlist entry a [`Cmd::Load`] made (`None`: refused).
    Loading(Option<i64>),
    TransportError(&'static str),
    Started(i64),
    Ended {
        entry: i64,
        reason: c_int,
        error: c_int,
    },
    /// The current live entry was read to its end (the server closed it)
    /// and mpv plays on with this spare entry of the same address, already
    /// open: not an end.
    Bridged(i64),
    /// Playback (re)started; with the audio output in use.
    Restarted {
        audio_out: Option<String>,
    },
    Buffering(bool),
    Width(i64),
    Height(i64),
    Aspect(f64),
    /// A warning or error (read for failure clues, never kept or logged),
    /// or an output format line (logged).
    Log {
        warn: bool,
        text: String,
    },
    /// `time-pos` in seconds.
    Position(f64),
    /// `track-list` changed: mpv's JSON of it.
    Tracks(String),
    /// `container-fps`.
    Fps(f64),
    /// `audio-params/channel-count`.
    Channels(i64),
}

pub struct Worker {
    mpv: Arc<Mpv>,
    queue: Arc<Mutex<Vec<Cmd>>>,
    quit: Arc<AtomicBool>,
    /// The `on_load` hook guards loads: added once per handle, and given
    /// up for good when a refused address could not be replaced
    /// ([`Bridge::guarded`]); a replacement thread inherits it.
    guarded: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Disconnected once the thread has ended, whatever ended it: a panic
    /// or mpv shutting down. `notify` runs once more after that.
    pub notes: Receiver<Note>,
}

impl Worker {
    /// Starts the thread; `notify` runs (on it) after notes were sent, and
    /// when it ends.
    pub fn spawn(mpv: Arc<Mpv>, notify: impl Fn() + Send + 'static) -> Worker {
        // SAFETY: a live initialized handle and static NUL-terminated hook name.
        let guarded = unsafe { ffi::mpv_hook_add(mpv.0, 0, c"on_load".as_ptr(), 0) } >= 0;
        Self::start(mpv, Arc::new(AtomicBool::new(guarded)), notify)
    }

    /// A new thread for this worker's handle, after its own ended. The
    /// handle keeps its hook and properties; commands queued for the old
    /// thread are gone.
    pub fn replacement(&self, notify: impl Fn() + Send + 'static) -> Worker {
        Self::start(self.mpv.clone(), self.guarded.clone(), notify)
    }

    fn start(
        mpv: Arc<Mpv>,
        guarded: Arc<AtomicBool>,
        notify: impl Fn() + Send + 'static,
    ) -> Worker {
        let (tx, notes) = channel();
        let queue = Arc::new(Mutex::new(Vec::new()));
        let quit = Arc::new(AtomicBool::new(false));
        let policy = super::POLICY.get();
        let thread = {
            let (mpv, queue, quit, guarded) =
                (mpv.clone(), queue.clone(), quit.clone(), guarded.clone());
            std::thread::Builder::new()
                .name("iptv-mpv".into())
                .spawn(move || {
                    /// Tells the main loop once the thread ends, after
                    /// `tx` is gone, so its drain sees the disconnection.
                    struct Ended<F: Fn()>(F);
                    impl<F: Fn()> Drop for Ended<F> {
                        fn drop(&mut self) {
                            (self.0)();
                        }
                    }
                    let ended = Ended(notify);
                    // Dropped before `ended` (reverse order), also when
                    // `run` unwinds.
                    let tx = tx;
                    let mut bridge = Bridge::new();
                    bridge.policy = policy;
                    bridge.guarded = guarded.load(Ordering::Acquire);
                    run(&mpv, &queue, &quit, &tx, &ended.0, &mut bridge, &guarded);
                })
                .map_err(|e| log::error!("iptv mpv: no control thread ({e})"))
                .ok()
        };
        Worker {
            mpv,
            queue,
            quit,
            guarded,
            thread,
            notes,
        }
    }

    pub fn send(&self, cmd: Cmd) {
        if let Ok(mut q) = self.queue.lock() {
            q.push(cmd);
        }
        self.mpv.wakeup();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Release);
        self.mpv.wakeup();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(
    mpv: &Mpv,
    queue: &Mutex<Vec<Cmd>>,
    quit: &AtomicBool,
    tx: &Sender<Note>,
    notify: &impl Fn(),
    bridge: &mut Bridge,
    guarded: &AtomicBool,
) {
    loop {
        if quit.load(Ordering::Acquire) {
            return;
        }
        let cmds = queue.lock().map(|mut q| std::mem::take(&mut *q));
        let mut sent = false;
        for cmd in cmds.unwrap_or_default() {
            for note in execute(mpv, cmd, bridge) {
                sent |= tx.send(note).is_ok();
            }
        }
        // Waits for an event or a wake-up (`send`, `drop`), then takes
        // whatever else is queued.
        let mut timeout = -1.0;
        loop {
            // SAFETY: a live handle; the event is valid until the next wait,
            // and only this thread waits.
            let ev = unsafe { &*ffi::mpv_wait_event(mpv.0, timeout) };
            timeout = 0.0;
            match ev.event_id {
                ffi::MPV_EVENT_NONE => break,
                ffi::MPV_EVENT_SHUTDOWN => return,
                ffi::MPV_EVENT_HOOK if !ev.data.is_null() => {
                    let path = mpv.get(c"stream-open-filename").unwrap_or_default();
                    let allowed = bridge
                        .transport
                        .as_ref()
                        .is_some_and(|relay| relay.owns_url(&path));
                    // Worker-only unit fixtures synthesize tones without IO.
                    let allowed = allowed || (cfg!(test) && path.starts_with("av://lavfi:"));
                    if !allowed {
                        let denied = mpv.try_set(
                            "stream-open-filename",
                            "iptv-blocked://unsupported-reference",
                        );
                        if !denied {
                            // Never continue an original path after failed
                            // substitution. Keep the hook blocked until handle
                            // teardown and refuse subsequent source loads.
                            bridge.guarded = false;
                            guarded.store(false, Ordering::Release);
                            sent |= tx
                                .send(Note::TransportError(
                                    "ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED",
                                ))
                                .is_ok();
                            continue;
                        }
                        if !bridge.retired(&path) {
                            sent |= tx
                                .send(Note::TransportError(
                                    "ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED",
                                ))
                                .is_ok();
                        }
                    }
                    // SAFETY: MPV_EVENT_HOOK carries this struct until next wait;
                    // continuing it exactly once releases the blocked load.
                    unsafe {
                        let hook = &*(ev.data as *const ffi::mpv_event_hook);
                        ffi::mpv_hook_continue(mpv.0, hook.id);
                    }
                }
                _ => {
                    if ev.event_id == ffi::MPV_EVENT_END_FILE
                        && let Some(code) = bridge
                            .transport
                            .as_ref()
                            .and_then(|transport| transport.take_error_code())
                    {
                        sent |= tx.send(Note::TransportError(code)).is_ok();
                    }
                    if let Some(note) = bridge.event(mpv, ev).or_else(|| convert(mpv, ev)) {
                        sent |= tx.send(note).is_ok();
                    }
                }
            }
        }
        if sent {
            notify();
        }
    }
}

/// Runs `cmd`; what it answers.
fn execute(mpv: &Mpv, cmd: Cmd, bridge: &mut Bridge) -> Vec<Note> {
    match cmd {
        #[cfg(all(test, target_os = "linux"))]
        Cmd::DisableBridge => {
            bridge.enabled = false;
            Vec::new()
        }
        #[cfg(all(test, target_os = "linux"))]
        Cmd::Panic => std::panic::resume_unwind(Box::new("synthetic control thread failure")),
        Cmd::Load {
            url,
            live,
            options,
            public_only,
        } => {
            if !bridge.guarded {
                return vec![Note::TransportError(
                    "ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED",
                )];
            }
            // Track choices are options: they would carry over to this file.
            mpv.set("vid", "auto");
            mpv.set("aid", "auto");
            mpv.set("sid", "no");
            // Also clears the bridge's spare options.
            for (name, value) in &options {
                mpv.set(name, value);
            }
            bridge.retire_transport();
            let url = if url
                .get(..7)
                .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
                || url
                    .get(..8)
                    .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
            {
                let Ok(transport) = iptv_net::playback::PlaybackRelay::start_with_policy(
                    &url,
                    &bridge.headers,
                    (bridge.policy)(public_only),
                ) else {
                    mpv.command(&["stop"]);
                    return vec![Note::Loading(None)];
                };
                let local = transport.url().to_owned();
                bridge.transport = Some(transport);
                local
            } else {
                url
            };
            let ok = mpv.command(&["loadfile", &url, "replace"]);
            // The new entry is the playlist's last (`replace` may keep the
            // stopping one until it ends).
            let id = ok.then(|| last_entry(mpv)).flatten();
            bridge.load(url, live, id);
            vec![Note::Loading(id)]
        }
        Cmd::Stop => {
            mpv.command(&["stop"]);
            bridge.retire_transport();
            bridge.load(String::new(), false, None);
            Vec::new()
        }
        Cmd::Set(name, value) => {
            mpv.set(name, &value);
            Vec::new()
        }
        Cmd::Http {
            source,
            user_agent,
            fields,
        } => {
            mpv.set("user-agent", &user_agent);
            mpv.command(&["change-list", "http-header-fields", "clr", ""]);
            if bridge
                .transport
                .as_ref()
                .is_some_and(|transport| Some(transport.source_url()) != source.as_deref())
            {
                bridge.retire_transport();
            }
            bridge.headers = fields
                .iter()
                .filter_map(|field| {
                    field
                        .split_once(':')
                        .map(|(name, value)| (name.to_owned(), value.trim().to_owned()))
                })
                .collect();
            bridge.headers.push(("User-Agent".into(), user_agent));
            if let Some(transport) = &bridge.transport {
                let _ = transport.update_headers(&bridge.headers);
            }
            Vec::new()
        }
        Cmd::Position => mpv
            .get_f64(c"time-pos")
            .map(Note::Position)
            .into_iter()
            .collect(),
        Cmd::Format => {
            let fps = mpv.get_f64(c"container-fps").map(Note::Fps);
            let channels = mpv
                .get_i64(c"audio-params/channel-count")
                .map(Note::Channels);
            fps.into_iter().chain(channels).collect()
        }
    }
}

/// The id of the playlist's last entry.
fn last_entry(mpv: &Mpv) -> Option<i64> {
    let n = mpv.get_i64(c"playlist-count")?;
    let name = CString::new(format!("playlist/{}/id", n - 1)).ok()?;
    mpv.get_i64(&name)
}

/// Removes the playlist entry `id`, if it is still there.
fn remove_entry(mpv: &Mpv, id: i64) {
    let n = mpv.get_i64(c"playlist-count").unwrap_or(0);
    let index = (0..n).find(|i| {
        CString::new(format!("playlist/{i}/id"))
            .ok()
            .and_then(|name| mpv.get_i64(&name))
            == Some(id)
    });
    if let Some(i) = index {
        mpv.command(&["playlist-remove", &i.to_string()]);
    }
}

/// The spare entry of a live stream (see the module's docs). Off with
/// `IPTV_RS_LIVE_BRIDGE=0`: every closed connection is then an end the app
/// reconnects.
struct Bridge {
    policy: super::Policy,
    guarded: bool,
    enabled: bool,
    headers: Vec<(String, String)>,
    transport: Option<iptv_net::playback::PlaybackRelay>,
    retired_prefixes: std::collections::VecDeque<String>,
    /// The live stream's address (`None`: nothing live is loaded).
    url: Option<String>,
    /// The entry that plays it, and its spare.
    current: Option<i64>,
    spare: Option<i64>,
    /// The entry mpv started last.
    started: Option<i64>,
    /// When the current entry started playing.
    since: Option<Instant>,
}

impl Bridge {
    fn retire_transport(&mut self) {
        if let Some(relay) = self.transport.take() {
            if self.retired_prefixes.len() == 8 {
                self.retired_prefixes.pop_front();
            }
            self.retired_prefixes.push_back(relay.prefix().to_owned());
        }
    }
    fn retired(&self, url: &str) -> bool {
        self.retired_prefixes
            .iter()
            .any(|prefix| url.starts_with(prefix))
    }

    fn new() -> Bridge {
        let off = std::env::var("IPTV_RS_LIVE_BRIDGE").is_ok_and(|v| v.trim() == "0");
        if off {
            log::info!("iptv mpv: live bridging off");
        }
        Bridge {
            policy: super::default_policy,
            guarded: true,
            enabled: !off,
            headers: Vec::new(),
            transport: None,
            retired_prefixes: std::collections::VecDeque::new(),
            url: None,
            current: None,
            spare: None,
            started: None,
            since: None,
        }
    }

    /// A load replaced the playlist (or a stop cleared it).
    fn load(&mut self, url: String, live: bool, entry: Option<i64>) {
        self.url = (live && self.enabled && entry.is_some()).then_some(url);
        self.current = entry;
        self.spare = None;
        self.since = None;
    }

    /// Bridging takes these events first; `Some` replaces their note.
    fn event(&mut self, mpv: &Mpv, ev: &ffi::mpv_event) -> Option<Note> {
        match ev.event_id {
            ffi::MPV_EVENT_START_FILE => {
                // SAFETY: the data type mpv documents for the event.
                // SAFETY: mpv supplies the documented event payload until the next wait_event call.
                let s = unsafe { &*(ev.data as *const ffi::mpv_event_start_file) };
                self.started = Some(s.playlist_entry_id);
                None
            }
            ffi::MPV_EVENT_PLAYBACK_RESTART => {
                self.restarted(mpv);
                None
            }
            ffi::MPV_EVENT_END_FILE => {
                // SAFETY: as above.
                // SAFETY: mpv supplies the documented event payload until the next wait_event call.
                let e = unsafe { &*(ev.data as *const ffi::mpv_event_end_file) };
                self.ended(mpv, e.playlist_entry_id, e.reason)
            }
            _ => None,
        }
    }

    /// The current live entry plays: it gets a spare.
    fn restarted(&mut self, mpv: &Mpv) {
        let Some(url) = &self.url else { return };
        if self.current.is_none() || self.started != self.current {
            return;
        }
        self.since.get_or_insert_with(Instant::now);
        if self.spare.is_some() {
            return;
        }
        // The spare of a live HLS stream starts at the newest segment: what
        // the current entry has buffered reaches about there (ffmpeg's
        // default, three segments back, would replay those seconds). A
        // prefetch opens with the global options, not the entry's own: set
        // them now that the current entry is open ([`Cmd::Load`] clears them).
        mpv.set("demuxer-lavf-o", SPARE_OPTIONS);
        if mpv.command(&["loadfile", url, "append"]) {
            self.spare = last_entry(mpv);
        }
    }

    /// The current entry ended: read to its end after it played a while,
    /// it hands over to its spare; any other end drops the spare (mpv
    /// stops instead of going on to it) and is reported as before.
    fn ended(&mut self, mpv: &Mpv, entry: i64, reason: c_int) -> Option<Note> {
        if self.current != Some(entry) {
            return None;
        }
        let spare = self.spare.take()?;
        let played = self.since.take().map(|t| t.elapsed());
        if reason == ffi::MPV_END_FILE_REASON_EOF && played.is_some_and(|p| p >= MIN_BRIDGED) {
            self.current = Some(spare);
            return Some(Note::Bridged(spare));
        }
        remove_entry(mpv, spare);
        None
    }
}

fn convert(mpv: &Mpv, ev: &ffi::mpv_event) -> Option<Note> {
    // SAFETY (each cast): the data type mpv documents for the event id.
    match ev.event_id {
        ffi::MPV_EVENT_LOG_MESSAGE => {
            // SAFETY: mpv supplies the documented event payload until the next wait_event call.
            let m = unsafe { &*(ev.data as *const ffi::mpv_event_log_message) };
            if m.text.is_null() {
                return None;
            }
            // SAFETY: mpv supplies a nul-terminated string; null was rejected above.
            let text = unsafe { CStr::from_ptr(m.text) }.to_string_lossy();
            let warn = m.log_level <= LOG_WARN;
            let format = text.starts_with("AO:") || text.starts_with("VO:");
            (warn || format).then(|| Note::Log {
                warn,
                text: text.into_owned(),
            })
        }
        ffi::MPV_EVENT_START_FILE => {
            // SAFETY: mpv supplies the documented event payload until the next wait_event call.
            let s = unsafe { &*(ev.data as *const ffi::mpv_event_start_file) };
            Some(Note::Started(s.playlist_entry_id))
        }
        ffi::MPV_EVENT_END_FILE => {
            // SAFETY: mpv supplies the documented event payload until the next wait_event call.
            let e = unsafe { &*(ev.data as *const ffi::mpv_event_end_file) };
            Some(Note::Ended {
                entry: e.playlist_entry_id,
                reason: e.reason,
                error: e.error,
            })
        }
        ffi::MPV_EVENT_PLAYBACK_RESTART => Some(Note::Restarted {
            audio_out: mpv.get(c"current-ao"),
        }),
        ffi::MPV_EVENT_PROPERTY_CHANGE if ev.reply_userdata == 5 => {
            mpv.get(c"track-list").map(Note::Tracks)
        }
        ffi::MPV_EVENT_PROPERTY_CHANGE => {
            // SAFETY: mpv supplies the documented event payload until the next wait_event call.
            let p = unsafe { &*(ev.data as *const ffi::mpv_event_property) };
            if p.data.is_null() {
                return None; // unavailable (no file, no video)
            }
            match (ev.reply_userdata, p.format) {
                (1, ffi::MPV_FORMAT_FLAG) => {
                    // SAFETY: the matched mpv format identifies this value type; null was rejected above.
                    Some(Note::Buffering(unsafe { *(p.data as *const c_int) } != 0))
                }
                // SAFETY: the matched mpv format identifies this value type; null was rejected above.
                (2, ffi::MPV_FORMAT_INT64) => Some(Note::Width(unsafe { *(p.data as *const i64) })),
                (3, ffi::MPV_FORMAT_INT64) => {
                    // SAFETY: the matched mpv format identifies this value type; null was rejected above.
                    Some(Note::Height(unsafe { *(p.data as *const i64) }))
                }
                (4, ffi::MPV_FORMAT_DOUBLE) => {
                    // SAFETY: the matched mpv format identifies this value type; null was rejected above.
                    Some(Note::Aspect(unsafe { *(p.data as *const f64) }))
                }
                // SAFETY: the matched mpv format identifies this value type; null was rejected above.
                (6, ffi::MPV_FORMAT_DOUBLE) => Some(Note::Fps(unsafe { *(p.data as *const f64) })),
                (7, ffi::MPV_FORMAT_INT64) => {
                    // SAFETY: the matched mpv format identifies this value type; null was rejected above.
                    Some(Note::Channels(unsafe { *(p.data as *const i64) }))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::sync::mpsc::RecvTimeoutError;

    use super::*;

    /// An instance that plays no sound.
    fn mpv() -> Arc<Mpv> {
        let mpv = Mpv::new().expect("libmpv");
        mpv.set("ao", "null");
        Arc::new(mpv)
    }

    /// A tone of `secs` seconds, made by mpv itself (no network, no video:
    /// the worker needs no render context for it).
    fn tone(secs: f32) -> String {
        format!("av://lavfi:sine=frequency=440:sample_rate=48000:duration={secs}")
    }

    /// How long a test waits for mpv at most: long, for a machine under
    /// heavy load (nothing waits for it unless something is wrong).
    const PATIENCE: Duration = Duration::from_secs(60);

    /// Waits until `done`, at most [`PATIENCE`].
    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn load(url: String, live: bool) -> Cmd {
        Cmd::Load {
            public_only: true,
            url,
            live,
            options: super::super::map::load_options(live, false),
        }
    }

    /// The notes until one matches `until` (at most [`PATIENCE`]), that
    /// one last.
    fn notes_until(worker: &Worker, until: impl Fn(&Note) -> bool) -> Vec<Note> {
        let deadline = Instant::now() + PATIENCE;
        let mut notes = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match worker.notes.recv_timeout(left) {
                Ok(note) => {
                    let done = until(&note);
                    notes.push(note);
                    if done {
                        return notes;
                    }
                }
                Err(RecvTimeoutError::Timeout) => panic!("timed out; {} notes", notes.len()),
                Err(e) => panic!("{e}"),
            }
        }
    }

    /// The entry the load made. (Notes of mpv's own may come first: the
    /// observed properties' first values, log lines.)
    fn loaded(notes: &[Note]) -> Option<i64> {
        notes.iter().find_map(|n| match n {
            Note::Loading(entry) => Some(*entry),
            _ => None,
        })?
    }

    fn ended(n: &Note) -> bool {
        matches!(n, Note::Ended { .. })
    }

    #[test]
    fn native_reference_load_is_blocked_before_network_io() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mpv = mpv();
        let worker = Worker::spawn(mpv.clone(), || {});
        // A decoder-selected playlist entry bypasses Cmd::Load, just as this
        // direct command does. The on_load hook must refuse it before open.
        assert!(mpv.command(&[
            "loadfile",
            &format!("http://{}/private", listener.local_addr().unwrap()),
            "replace"
        ]));
        let notes = notes_until(&worker, ended);
        assert!(notes.iter().any(|note| matches!(
            note,
            Note::TransportError("ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED")
        )));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn unquoted_hls_key_cannot_reach_a_second_loopback_server() {
        use std::io::{Read, Write};
        let origin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin_address = origin.local_addr().unwrap();
        // Keep this unique source origin reserved through the counter assertion.
        let _origin_lifetime = origin.try_clone().unwrap();
        origin.set_nonblocking(true).unwrap();
        let forbidden = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        forbidden.set_nonblocking(true).unwrap();
        let playlist = format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:1\n\
             #EXT-X-KEY:METHOD=AES-128,URI=http://{blocked}/synthetic.key\n\
             #EXTINF:1,\nhttp://{blocked}/segment.ts\n#EXT-X-ENDLIST\n",
            blocked = forbidden.local_addr().unwrap()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stop_server = stop.clone();
        let seen = Arc::new(AtomicBool::new(false));
        let seen_server = seen.clone();
        let server = std::thread::spawn(move || {
            while !stop_server.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = origin.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let _ = stream.read(&mut [0; 4096]);
                seen_server.store(true, Ordering::Release);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                    playlist.len(),
                    playlist
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        fn actual_policy(public_only: bool) -> iptv_net::playback::PlaybackPolicy {
            iptv_net::playback::PlaybackPolicy {
                public_only,
                ..Default::default()
            }
        }
        let previous = super::super::POLICY.replace(actual_policy);
        let mpv = mpv();
        // Exercise FFmpeg's nested HLS loads, not mpv's plain-playlist fallback.
        mpv.set("demuxer", "lavf");
        mpv.set("demuxer-lavf-format", "hls");
        let worker = Worker::spawn(mpv.clone(), || {});
        super::super::POLICY.set(previous);
        worker.send(Cmd::Load {
            public_only: false,
            url: format!("http://{origin_address}/list.m3u8"),
            live: false,
            options: super::super::map::load_options(false, false),
        });
        let mut notes = notes_until(&worker, ended);
        wait_until("player to finish refusing the playlist", || {
            mpv.get(c"idle-active").as_deref() == Some("yes")
        });
        notes.extend(worker.notes.try_iter());
        stop.store(true, Ordering::Release);
        server.join().unwrap();
        assert!(
            seen.load(Ordering::Acquire),
            "the player fetched the synthetic playlist"
        );
        assert!(
            iptv_net::playback::PlaybackRelay::key_requests_for(&format!(
                "http://{origin_address}/list.m3u8"
            )) >= 1,
            "the player requested the encryption key through the relay"
        );
        assert!(
            !notes
                .iter()
                .any(|note| matches!(note, Note::Restarted { .. }))
        );
        assert!(
            notes
                .iter()
                .any(|note| matches!(note, Note::TransportError("ERROR_CODE_IO_UNSPECIFIED"))),
            "the nested key reached the relay and was refused by its address policy"
        );
        assert_eq!(
            forbidden.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(notes.iter().any(|note| matches!(
            note,
            Note::Ended {
                reason: ffi::MPV_END_FILE_REASON_ERROR,
                ..
            }
        )));
    }

    #[test]
    fn changing_source_retires_transport_before_new_identity_is_applied() {
        let mut bridge = Bridge::new();
        bridge.transport = Some(
            iptv_net::playback::PlaybackRelay::start_with_policy(
                "http://localhost:1/old",
                &[],
                crate::mpv::default_policy(false),
            )
            .unwrap(),
        );
        let retired = bridge.transport.as_ref().unwrap().url().to_owned();
        let mpv = mpv();
        execute(
            &mpv,
            Cmd::Http {
                source: Some("http://localhost:2/new".into()),
                user_agent: "SyntheticAgent".into(),
                fields: vec!["Authorization: synthetic-new-identity".into()],
            },
            &mut bridge,
        );
        assert!(bridge.transport.is_none());
        assert!(
            bridge.retired(&retired),
            "a pending hook from the retired source is denied silently"
        );
        assert!(
            bridge
                .headers
                .iter()
                .any(|(name, value)| name == "Authorization" && value == "synthetic-new-identity")
        );
    }

    /// A file's life as notes, in mpv's order: the entry the load made,
    /// its start, the playback start with the audio output, its end; the
    /// main loop is woken for them.
    #[test]
    fn a_load_reports_its_entry_start_and_end() {
        let mpv = mpv();
        let (woke_tx, woke) = channel();
        let worker = Worker::spawn(mpv.clone(), move || {
            let _ = woke_tx.send(());
        });
        worker.send(Cmd::Http {
            source: None,
            user_agent: "SyntheticAgent/3.0".into(),
            fields: vec!["X-One: 1".into(), "X-Two: 2".into()],
        });
        // Paused from the start: it cannot end before it was asked about.
        let Cmd::Load { url, options, .. } = load(tone(1.0), false) else {
            unreachable!()
        };
        let mut options = options;
        options.push(("pause", "yes".into()));
        worker.send(Cmd::Load {
            public_only: true,
            url,
            live: false,
            options,
        });
        let notes = notes_until(&worker, |n| matches!(n, Note::Restarted { .. }));
        let Some(entry) = loaded(&notes) else {
            panic!("the load's entry");
        };
        assert!(
            notes
                .iter()
                .any(|n| matches!(n, Note::Started(e) if *e == entry))
        );
        let Some(Note::Restarted { audio_out }) = notes.last() else {
            unreachable!()
        };
        assert_eq!(audio_out.as_deref(), Some("null"));
        assert!(woke.recv_timeout(PATIENCE).is_ok());
        assert_eq!(
            mpv.get(c"user-agent").as_deref(),
            Some("SyntheticAgent/3.0")
        );
        assert_eq!(mpv.get(c"http-header-fields").as_deref(), Some(""));

        // What plays, while it plays.
        worker.send(Cmd::Position);
        let notes = notes_until(&worker, |n| matches!(n, Note::Position(_)));
        assert!(matches!(notes.last(), Some(Note::Position(s)) if *s >= 0.0));
        worker.send(Cmd::Format);
        let notes = notes_until(&worker, |n| matches!(n, Note::Channels(_)));
        assert!(matches!(notes.last(), Some(Note::Channels(1))));

        // Played to its end.
        worker.send(Cmd::Set("pause", "no".into()));
        let notes = notes_until(&worker, ended);
        let Some(Note::Ended {
            entry: e,
            reason,
            error,
        }) = notes.last()
        else {
            unreachable!()
        };
        assert_eq!(
            (*e, *reason, *error),
            (entry, ffi::MPV_END_FILE_REASON_EOF, 0)
        );
        // Nothing playing: nothing to report.
        worker.send(Cmd::Position);
        worker.send(Cmd::Format);
        worker.send(Cmd::Set("volume", "30".into()));
        wait_until("the volume", || mpv.get_f64(c"volume") == Some(30.0));
        assert!(
            worker
                .notes
                .try_iter()
                .all(|n| !matches!(n, Note::Position(_) | Note::Fps(_))),
            "nothing playing, nothing to report"
        );
    }

    /// A refused load answers no entry; a stop ends what plays.
    #[test]
    fn a_refused_load_and_a_stop() {
        let mpv = mpv();
        let worker = Worker::spawn(mpv.clone(), || {});
        worker.send(load("synthetic\0url".into(), false));
        let notes = notes_until(&worker, |n| matches!(n, Note::Loading(_)));
        assert!(matches!(notes.last(), Some(Note::Loading(None))));

        worker.send(load(tone(30.0), true));
        notes_until(&worker, |n| matches!(n, Note::Restarted { .. }));
        // The live entry got its spare.
        assert_eq!(mpv.get_i64(c"playlist-count"), Some(2));
        worker.send(Cmd::Stop);
        let notes = notes_until(&worker, ended);
        let Some(Note::Ended { reason, .. }) = notes.last() else {
            unreachable!()
        };
        assert_ne!(*reason, ffi::MPV_END_FILE_REASON_EOF);
    }

    /// A live entry that ended before it played [`MIN_BRIDGED`] is an end;
    /// its spare goes, so mpv stops instead of playing it.
    #[test]
    fn a_short_live_entry_ends_and_drops_its_spare() {
        let mpv = mpv();
        let worker = Worker::spawn(mpv.clone(), || {});
        worker.send(load(tone(0.4), true));
        let notes = notes_until(&worker, ended);
        let Some(entry) = loaded(&notes) else {
            panic!("the load's entry");
        };
        assert!(
            matches!(notes.last(), Some(Note::Ended { entry: e, reason: 0, .. }) if *e == entry)
        );
        assert!(!notes.iter().any(|n| matches!(n, Note::Bridged(_))));
        // The worker removed the spare (the ended entry stays listed).
        wait_until("the spare to go", || {
            mpv.get_i64(c"playlist-count") == Some(1)
        });
    }

    /// mpv shutting down ends the thread.
    #[test]
    fn the_thread_ends_with_mpv() {
        let mpv = mpv();
        let worker = Worker::spawn(mpv.clone(), || {});
        assert!(mpv.command(&["quit"]));
        let thread = worker.thread.as_ref().unwrap();
        wait_until("the thread to end", || thread.is_finished());
    }

    /// Off (`IPTV_RS_LIVE_BRIDGE=0`), or with a refused load, nothing is
    /// bridged; only the current entry's end with a spare is the bridge's.
    #[test]
    fn the_bridge_keeps_to_its_entry() {
        let mpv = mpv();
        let mut bridge = Bridge::new();
        bridge.enabled = false;
        bridge.load("http://stream.example.test/live.ts".into(), true, Some(1));
        assert_eq!(bridge.url, None);
        bridge.enabled = true;
        bridge.load("http://stream.example.test/live.ts".into(), true, None);
        assert_eq!(bridge.url, None, "refused");
        bridge.load("http://stream.example.test/live.ts".into(), true, Some(1));
        assert!(bridge.url.is_some());
        // Not the current entry, or no spare: not the bridge's.
        assert!(
            bridge
                .ended(&mpv, 2, ffi::MPV_END_FILE_REASON_EOF)
                .is_none()
        );
        assert!(
            bridge
                .ended(&mpv, 1, ffi::MPV_END_FILE_REASON_EOF)
                .is_none()
        );
        // Restarts of another entry make no spare.
        bridge.started = Some(3);
        bridge.restarted(&mpv);
        assert_eq!((bridge.spare, bridge.since), (None, None));
        // A restart of the current entry that has its spare (a rebuffer):
        // timed from then, no second spare.
        bridge.started = Some(1);
        bridge.spare = Some(9);
        bridge.restarted(&mpv);
        assert!(bridge.since.is_some());
        assert_eq!(mpv.get_i64(c"playlist-count"), Some(0));
        // A spare and long enough: the bridge.
        bridge.since = Instant::now().checked_sub(MIN_BRIDGED);
        assert!(matches!(
            bridge.ended(&mpv, 1, ffi::MPV_END_FILE_REASON_EOF),
            Some(Note::Bridged(9))
        ));
        assert_eq!(bridge.current, Some(9));
        // An error end drops the spare (not listed: nothing to remove).
        bridge.spare = Some(10);
        bridge.since = Instant::now().checked_sub(MIN_BRIDGED);
        assert!(
            bridge
                .ended(&mpv, 9, ffi::MPV_END_FILE_REASON_ERROR)
                .is_none()
        );
        assert_eq!(bridge.spare, None);
    }

    fn event(id: c_int, reply: u64, data: *mut c_void) -> ffi::mpv_event {
        ffi::mpv_event {
            event_id: id,
            error: 0,
            reply_userdata: reply,
            data,
        }
    }

    fn property(mpv: &Mpv, reply: u64, format: c_int, data: *mut c_void) -> Option<Note> {
        let mut p = ffi::mpv_event_property {
            name: c"synthetic".as_ptr(),
            format,
            data,
        };
        convert(
            mpv,
            &event(
                ffi::MPV_EVENT_PROPERTY_CHANGE,
                reply,
                (&mut p as *mut ffi::mpv_event_property).cast(),
            ),
        )
    }

    /// mpv's events as notes: observed properties by their ids, warnings
    /// and output formats from the log, nothing else.
    #[test]
    fn events_become_notes() {
        let mpv = mpv();
        let (mut flag, mut int, mut double): (c_int, i64, f64) = (1, 640, 1.5);
        let flag_p = (&mut flag as *mut c_int).cast();
        let int_p = (&mut int as *mut i64).cast();
        let double_p = (&mut double as *mut f64).cast();
        use ffi::{MPV_FORMAT_DOUBLE as D, MPV_FORMAT_FLAG as F, MPV_FORMAT_INT64 as I};
        assert!(matches!(
            property(&mpv, 1, F, flag_p),
            Some(Note::Buffering(true))
        ));
        assert!(matches!(
            property(&mpv, 2, I, int_p),
            Some(Note::Width(640))
        ));
        assert!(matches!(
            property(&mpv, 3, I, int_p),
            Some(Note::Height(640))
        ));
        assert!(matches!(property(&mpv, 4, D, double_p), Some(Note::Aspect(a)) if a == 1.5));
        assert!(matches!(property(&mpv, 6, D, double_p), Some(Note::Fps(f)) if f == 1.5));
        assert!(matches!(
            property(&mpv, 7, I, int_p),
            Some(Note::Channels(640))
        ));
        assert!(
            matches!(property(&mpv, 5, 0, std::ptr::null_mut()), Some(Note::Tracks(t)) if t == "[]")
        );
        // Unavailable, or not what was observed.
        assert!(property(&mpv, 2, I, std::ptr::null_mut()).is_none());
        assert!(property(&mpv, 2, D, double_p).is_none());
        assert!(property(&mpv, 8, I, int_p).is_none());

        let log = |level: c_int, text: Option<&CStr>| {
            let mut m = ffi::mpv_event_log_message {
                prefix: c"cplayer".as_ptr(),
                level: c"info".as_ptr(),
                text: text.map_or(std::ptr::null(), CStr::as_ptr),
                log_level: level,
            };
            convert(
                &mpv,
                &event(
                    ffi::MPV_EVENT_LOG_MESSAGE,
                    0,
                    (&mut m as *mut ffi::mpv_event_log_message).cast(),
                ),
            )
        };
        assert!(log(40, None).is_none());
        assert!(log(40, Some(c"Playing: synthetic\n")).is_none());
        assert!(matches!(
            log(40, Some(c"AO: [null] 48000Hz\n")),
            Some(Note::Log { warn: false, text }) if text.starts_with("AO:")
        ));
        assert!(matches!(
            log(20, Some(c"HTTP error 404\n")),
            Some(Note::Log { warn: true, .. })
        ));
        assert!(convert(&mpv, &event(99, 0, std::ptr::null_mut())).is_none());
    }
}
