//! What the guide sources tests share: a local HTTP server (fixed bodies by
//! path on 127.0.0.1, 404 for anything else), XMLTV of synthetic
//! channels, and a driver whose steps fail with what is on screen.
//! Synthetic data only.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nsh_core::ViewKind;
use nsh_core::geometry::{Point, Points};
use nsh_engine::EngineConfig;
use nsh_testing::Headless;

/// The app, driven by steps that panic with the texts on screen (those
/// not on it at [`T::rebase`]).
pub struct T {
    pub app: Headless,
    base: HashSet<String>,
    /// Report failures instead of panicking (exploring).
    pub soft: bool,
}

#[allow(dead_code)]
impl T {
    pub fn new(config: EngineConfig) -> Self {
        // The app's warnings, shown with a failure.
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
            .is_test(true)
            .try_init();
        Self {
            app: Headless::start(config).unwrap(),
            base: HashSet::new(),
            soft: false,
        }
    }

    /// What is on screen now is left out of failure reports.
    pub fn rebase(&mut self) {
        self.base = self.texts().into_iter().collect();
    }

    pub fn texts(&self) -> Vec<String> {
        self.app
            .screen()
            .texts()
            .into_iter()
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// The texts not on screen at the last [`T::rebase`].
    pub fn new_texts(&self) -> Vec<String> {
        self.texts()
            .into_iter()
            .filter(|t| !self.base.contains(t))
            .collect()
    }

    pub fn dump(&mut self, at: &str) {
        let _ = self.app.wait(Duration::from_millis(400));
        eprintln!("--- {at}: {:?}", self.new_texts());
    }

    pub fn ok(&mut self, what: &str, r: Result<(), String>) {
        if let Err(e) = r {
            let first = e.split("; on screen").next().unwrap_or(&e).to_owned();
            let msg = format!("{what}: {first}\n  new on screen: {:?}", self.new_texts());
            if self.soft {
                eprintln!("!!! {msg}");
            } else {
                panic!("{msg}");
            }
        }
    }

    /// Timeout scaled by `IPTV_TEST_WAIT_SCALE`.
    pub fn wait_timeout(secs: u64) -> Duration {
        Duration::from_secs(secs.saturating_mul(crate::isolated::wait_scale() as u64))
    }

    /// Presses `sel` once it is on screen. A step is done when the app
    /// rendered nothing for a moment, which a loaded machine can reach
    /// before the screen the last step opened is drawn: the press waits
    /// for its target instead of failing on the old screen.
    pub fn press(&mut self, sel: &str) {
        let r = self.app.wait_for_within(sel, Self::wait_timeout(10)).and_then(|()| self.app.press(sel));
        self.ok(&format!("press {sel}"), r);
    }

    pub fn wait(&mut self, sel: &str, secs: u64) {
        let r = self.app.wait_for_within(sel, Self::wait_timeout(secs));
        self.ok(&format!("wait {sel}"), r);
    }

    /// Presses `sel` and waits for the sheet it opens (`title`).
    pub fn open_sheet(&mut self, sel: &str, title: &str) {
        self.press(sel);
        self.wait(title, 2);
    }

    pub fn gone(&mut self, sel: &str, secs: u64) {
        let r = self.app.wait_for_gone(sel, Self::wait_timeout(secs));
        self.ok(&format!("gone {sel}"), r);
    }

    /// Waits until `sel` matches `n` views (within `secs`).
    pub fn count(&mut self, sel: &str, n: usize, secs: u64) {
        let start = std::time::Instant::now();
        let timeout = Self::wait_timeout(secs);
        loop {
            self.app.catch_up();
            let found = self.app.screen().count(sel).unwrap();
            if found == n {
                return;
            }
            if start.elapsed() > timeout {
                self.ok(sel, Err(format!("{found} views, not {n}")));
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Whether `sel` is on screen now.
    pub fn shows(&self, sel: &str) -> bool {
        self.app.screen().matches(sel).unwrap()
    }

    pub fn key(&mut self, k: &str) {
        let r = self.app.press_key(k);
        self.ok(&format!("key {k}"), r);
    }

    /// The text input mounted last (the open sheet's), as a selector.
    fn last_input(&self) -> String {
        let id = self
            .app
            .screen()
            .layout_nodes()
            .into_iter()
            .filter(|n| n.kind == ViewKind::Input && !n.hidden)
            .map(|n| n.id)
            .max();
        match id {
            Some(id) => format!("view:{id}"),
            None if self.soft => "id:no-input".to_owned(),
            None => panic!("no input; new on screen: {:?}", self.new_texts()),
        }
    }

    /// Types `text` in place of what the open sheet's field holds, then
    /// Return.
    pub fn enter(&mut self, text: &str) {
        self.replace(text);
        let input = self.last_input();
        let r = self.app.submit(&input);
        self.ok("submit", r);
    }

    /// Return in the open sheet's field.
    pub fn submit(&mut self) {
        let input = self.last_input();
        let r = self.app.submit(&input);
        self.ok("submit", r);
    }

    /// Types `text` in place of what the open sheet's field holds.
    pub fn replace(&mut self, text: &str) {
        let input = self.last_input();
        let r = self.app.clear(&input);
        self.ok("clear", r);
        if !text.is_empty() {
            let r = self.app.type_text(&input, text);
            self.ok("type", r);
        }
    }

    /// What the open sheet's field holds.
    pub fn field(&self) -> String {
        let input = self.last_input();
        self.app
            .screen()
            .input_text(&input)
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_owned()
    }

    /// A tap beside the open sheet holding `text` (on its dimmed
    /// backdrop, near the window's top left corner).
    pub fn press_beside(&mut self, text: &str) {
        let nodes = self.app.screen().layout_nodes();
        let parent: HashMap<_, _> = nodes.iter().map(|n| (n.id, n.parent)).collect();
        let inside = nodes
            .iter()
            .find(|n| n.text.as_ref().is_some_and(|t| t.text.text.trim() == text))
            .map(|n| n.id)
            .expect("the sheet's text");
        // Up from the text: the first ancestor with a full-height sibling
        // mounted before it (the backdrop).
        let mut at = inside;
        let backdrop = loop {
            let Some(&up) = parent.get(&at) else {
                panic!("no backdrop around {text}");
            };
            let dim = nodes.iter().find(|n| {
                n.parent == up
                    && n.id < at
                    && n.bounds.origin.y.0 <= 1.0
                    && n.bounds.size.height.0 > 300.0
            });
            if let Some(dim) = dim {
                break dim.id;
            }
            at = up;
        };
        let r = self.app.press_at(
            &format!("view:{backdrop}"),
            Point::new(Points(12.0), Points(12.0)),
        );
        self.ok(&format!("press beside {text}"), r);
    }
}

/// A playlist picked from a file.
pub fn picked(text: String) -> Vec<nsh_core::picker::PickedFile> {
    vec![nsh_core::picker::PickedFile::new(
        "synthetic.m3u",
        "audio/x-mpegurl",
        nsh_core::ImageData::new(text.into_bytes()),
    )]
}

/// The server's paths and bodies; the bodies can change while it runs.
#[derive(Clone)]
pub struct Server {
    pub base: String,
    bodies: Arc<Mutex<HashMap<String, String>>>,
}

impl Server {
    /// Serves `bodies` (path → body) from a free port until the test ends.
    pub fn start(bodies: &[(&str, String)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let bodies: HashMap<String, String> = bodies
            .iter()
            .map(|(p, b)| ((*p).to_owned(), b.clone()))
            .collect();
        let bodies = Arc::new(Mutex::new(bodies));
        let served = bodies.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let served = served.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(&stream);
                    let mut first = String::new();
                    if reader.read_line(&mut first).is_err() {
                        return;
                    }
                    // The headers, up to the blank line.
                    let mut line = String::new();
                    while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                        line.clear();
                    }
                    let path = first.split_whitespace().nth(1).unwrap_or("/");
                    let path = path.split('?').next().unwrap_or(path);
                    let body = served.lock().unwrap().get(path).cloned();
                    let (status, body) = match body {
                        Some(b) => ("200 OK", b),
                        None => ("404 Not Found", String::new()),
                    };
                    let mut out = &stream;
                    let _ = write!(
                        out,
                        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = out.flush();
                    // Closed gracefully: the client reads the whole answer
                    // (a reset could drop it).
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let _ = std::io::copy(&mut &stream, &mut std::io::sink());
                });
            }
        });
        Self { base, bodies }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Serves `body` at `path` from now on.
    #[allow(dead_code)]
    pub fn set(&self, path: &str, body: String) {
        self.bodies.lock().unwrap().insert(path.to_owned(), body);
    }
}

/// An XMLTV guide: per channel (id, display name), one programme `title`
/// on now (an hour around `now`).
pub fn guide(now: i64, channels: &[(&str, &str, &str)]) -> String {
    use iptv_epg::format_xmltv_time;
    let mut xml = String::from("<?xml version=\"1.0\"?><tv>");
    for (id, name, _) in channels {
        xml.push_str(&format!(
            "<channel id=\"{id}\"><display-name>{name}</display-name></channel>"
        ));
    }
    for (id, _, title) in channels {
        xml.push_str(&format!(
            "<programme channel=\"{id}\" start=\"{}\" stop=\"{}\"><title>{title}</title></programme>",
            format_xmltv_time(now - 1800),
            format_xmltv_time(now + 1800)
        ));
    }
    xml + "</tv>"
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
