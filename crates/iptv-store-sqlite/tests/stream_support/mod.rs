//! A guide download without a network: an in-memory [`Fetch`] serving
//! synthetic XMLTV (held, or generated as it is read), a single-threaded
//! `block_on`, and builders for synthetic playlists and guides. Hosts use
//! reserved example domains.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::future::Future;

use iptv_epg::{UnixTime, format_xmltv_time};
use iptv_import::{
    BodyStream, CancelToken, Fetch, FetchError, FetchErrorKind, MemoryBody, NoProgress,
    StreamingResponse, import_m3u_text,
};
use iptv_providers::model::Channel;
use iptv_providers::{HttpRequest, HttpResponse};

pub const HOUR: i64 = 3600;
pub const BASE: &str = "http://epg.example.com/";

/// Runs a future whose I/O is all in memory (never pending for long).
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let mut fut = std::pin::pin!(fut);
    loop {
        if let std::task::Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

/// A body served from memory, generated chunk by chunk, or cut short.
pub enum Body {
    Memory(MemoryBody),
    Generated(Generated),
}

impl BodyStream for Body {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, FetchError> {
        match self {
            Body::Memory(b) => b.next_chunk().await,
            Body::Generated(g) => Ok(g.chunk()),
        }
    }
}

/// What a path serves.
#[derive(Clone)]
pub enum Serve {
    Bytes(Vec<u8>),
    /// The bytes, then a dropped connection.
    Truncated(Vec<u8>),
    Generated(GenShape),
}

/// Serves `BASE` + name; anything else is a 404.
#[derive(Default)]
pub struct Feeds {
    pub paths: HashMap<String, Serve>,
    pub chunk: usize,
}

impl Feeds {
    pub fn new() -> Self {
        Self {
            paths: HashMap::new(),
            chunk: 16 * 1024 + 7,
        }
    }

    pub fn serve(mut self, name: &str, what: Serve) -> Self {
        self.paths.insert(format!("{BASE}{name}"), what);
        self
    }
}

impl Fetch for Feeds {
    type Body = Body;

    fn fetch(&self, _: HttpRequest) -> impl Future<Output = Result<HttpResponse, FetchError>> {
        std::future::ready(Err(FetchError::new(FetchErrorKind::Connect, "not served")))
    }

    fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<StreamingResponse<Body>, FetchError>> {
        let response = |status, body| StreamingResponse {
            status,
            content_length: None,
            body,
        };
        let r = match self.paths.get(&request.url) {
            Some(Serve::Bytes(b)) => response(200, Body::Memory(MemoryBody::new(b, self.chunk))),
            Some(Serve::Truncated(b)) => {
                let cut = FetchError::new(FetchErrorKind::Body, "connection reset");
                let body = MemoryBody::new(b, self.chunk).failing_at_end(cut);
                response(200, Body::Memory(body))
            }
            Some(Serve::Generated(shape)) => response(200, Body::Generated(Generated::new(*shape))),
            None => response(404, Body::Memory(MemoryBody::default())),
        };
        std::future::ready(Ok(r))
    }
}

/// One element of a hand-written guide.
pub enum El<'a> {
    Channel(&'a str, &'a [&'a str]),
    Program(Prog<'a>),
}

#[derive(Clone, Copy, Default)]
pub struct Prog<'a> {
    pub channel: &'a str,
    pub start: UnixTime,
    pub stop: Option<UnixTime>,
    pub title: &'a str,
    pub sub_title: Option<&'a str>,
    pub desc: Option<&'a str>,
    pub episode: Option<&'a str>,
    pub category: Option<&'a str>,
}

pub fn prog<'a>(
    channel: &'a str,
    start: UnixTime,
    stop: Option<UnixTime>,
    title: &'a str,
) -> Prog<'a> {
    Prog {
        channel,
        start,
        stop,
        title,
        ..Prog::default()
    }
}

/// XMLTV with the elements in this order.
pub fn xmltv(elements: &[El<'_>]) -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tv>\n");
    for el in elements {
        match el {
            El::Channel(id, names) => {
                let _ = write!(s, "<channel id=\"{id}\">");
                for n in names.iter() {
                    let _ = write!(s, "<display-name>{n}</display-name>");
                }
                s.push_str("</channel>\n");
            }
            El::Program(p) => {
                let _ = write!(
                    s,
                    "<programme start=\"{}\" channel=\"{}\"",
                    format_xmltv_time(p.start),
                    p.channel
                );
                if let Some(stop) = p.stop {
                    let _ = write!(s, " stop=\"{}\"", format_xmltv_time(stop));
                }
                let _ = write!(s, "><title>{}</title>", p.title);
                if let Some(t) = p.sub_title {
                    let _ = write!(s, "<sub-title>{t}</sub-title>");
                }
                if let Some(t) = p.desc {
                    let _ = write!(s, "<desc>{t}</desc>");
                }
                if let Some(t) = p.category {
                    let _ = write!(s, "<category>{t}</category>");
                }
                if let Some(t) = p.episode {
                    let _ = write!(s, "<episode-num system=\"onscreen\">{t}</episode-num>");
                }
                s.push_str("</programme>\n");
            }
        }
    }
    s.push_str("</tv>\n");
    s
}

/// A playlist channel: (tvg-id, tvg-name, name).
pub type PlaylistEntry<'a> = (Option<&'a str>, Option<&'a str>, &'a str);

/// The channels of an M3U playlist with these entries.
pub fn playlist(entries: &[PlaylistEntry<'_>]) -> Vec<Channel> {
    let mut s = String::from("#EXTM3U\n");
    for (i, (tvg_id, tvg_name, name)) in entries.iter().enumerate() {
        s.push_str("#EXTINF:-1");
        if let Some(id) = tvg_id {
            let _ = write!(s, " tvg-id=\"{id}\"");
        }
        if let Some(n) = tvg_name {
            let _ = write!(s, " tvg-name=\"{n}\"");
        }
        let _ = writeln!(s, " group-title=\"Synthetic\",{name}");
        let _ = writeln!(s, "http://streams.example.com/live/{i}.ts");
    }
    import_m3u_text(s.as_bytes(), &mut NoProgress, &CancelToken::new())
        .unwrap()
        .channels
}

/// A generated guide: `channels` channels (`gen{n}.example`, "Generated
/// n"), hourly programmes with descriptions over `days` days from `from`.
#[derive(Clone, Copy, Debug)]
pub struct GenShape {
    pub channels: usize,
    pub days: i64,
    pub from: UnixTime,
}

/// Writes a [`GenShape`] guide as it is read (memory: one chunk).
pub struct Generated {
    shape: GenShape,
    /// Next channel element, then next programme channel.
    channel: usize,
    programs: bool,
    done: bool,
}

impl Generated {
    pub fn new(shape: GenShape) -> Self {
        Self {
            shape,
            channel: 0,
            programs: false,
            done: false,
        }
    }

    /// The next chunk (about 64 KiB), `None` at the end.
    pub fn chunk(&mut self) -> Option<Vec<u8>> {
        if self.done {
            return None;
        }
        let mut s = String::with_capacity(70 * 1024);
        if self.channel == 0 && !self.programs {
            s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tv>\n");
        }
        while s.len() < 64 * 1024 {
            let n = self.channel;
            if !self.programs {
                let _ = writeln!(
                    s,
                    "<channel id=\"gen{n}.example\"><display-name>Generated {n}</display-name></channel>"
                );
            } else {
                for h in 0..self.shape.days * 24 {
                    let t = self.shape.from + h * HOUR;
                    let _ = writeln!(
                        s,
                        "<programme start=\"{}\" stop=\"{}\" channel=\"gen{n}.example\">\
                         <title>Show {n}-{h}</title>\
                         <desc>Invented episode {h} of show {n}, with a description of ordinary \
                         length.</desc></programme>",
                        format_xmltv_time(t),
                        format_xmltv_time(t + HOUR)
                    );
                }
            }
            self.channel += 1;
            if self.channel == self.shape.channels {
                self.channel = 0;
                if self.programs {
                    s.push_str("</tv>\n");
                    self.done = true;
                    break;
                }
                self.programs = true;
            }
        }
        Some(s.into_bytes())
    }
}

/// The playlist matching a [`GenShape`] guide by tvg-id.
pub fn generated_playlist(channels: usize) -> Vec<Channel> {
    let ids: Vec<String> = (0..channels).map(|n| format!("gen{n}.example")).collect();
    let names: Vec<String> = (0..channels).map(|n| format!("Generated {n}")).collect();
    let entries: Vec<PlaylistEntry<'_>> = ids
        .iter()
        .zip(&names)
        .map(|(id, name)| (Some(id.as_str()), None, name.as_str()))
        .collect();
    playlist(&entries)
}
