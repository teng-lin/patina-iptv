//! Synthetic playlists, guides and simulated provider backends for tests.
//!
//! Std only (plus flate2 for gzip) so `iptv-net`'s tests can include this
//! file too. Everything here is invented: reserved `.example` domains,
//! local addresses, made-up names.
#![allow(dead_code)]

use std::cell::Cell;
use std::fmt::Write as _;
use std::io::Write as _;

/// The synthetic "now" for every test (2027-01-15T08:00:00Z).
pub const NOW: i64 = 1_800_000_000;

pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

/// An M3U with `n` channels in `groups` groups. Every 10th channel has
/// catch-up; `epg` goes into `url-tvg`.
pub fn m3u(n: usize, groups: usize, epg: Option<&str>, stream_base: &str) -> String {
    let mut s = String::from("#EXTM3U");
    if let Some(e) = epg {
        write!(s, " url-tvg=\"{e}\"").unwrap();
    }
    s.push('\n');
    for i in 0..n {
        let catchup = if i % 10 == 0 {
            format!(
                " catchup=\"default\" catchup-days=\"3\" catchup-source=\"{stream_base}/archive/ch{}/index.m3u8?utc={{utc}}&duration={{duration}}\"",
                i % 3
            )
        } else {
            String::new()
        };
        writeln!(
            s,
            "#EXTINF:-1 tvg-id=\"ch{i}.example\" tvg-name=\"Synthetic {i}\" tvg-logo=\"http://img.example.com/{i}.png\" group-title=\"Group {}\"{catchup},Synthetic {i}",
            i % groups.max(1)
        )
        .unwrap();
        writeln!(s, "{stream_base}/ch{}/index.m3u8?n={i}", i % 3).unwrap();
    }
    s
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn xmltv_time(t: i64) -> String {
    // Civil-from-days (Howard Hinnant), UTC.
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}{:02}{:02}{:02} +0000",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// XMLTV for `channels` (id, display name) with `slot`-second programmes
/// covering `[from, to)`.
pub fn xmltv(channels: &[(String, String)], from: i64, to: i64, slot: i64) -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tv>\n");
    for (id, name) in channels {
        writeln!(
            s,
            "<channel id=\"{}\"><display-name>{}</display-name></channel>",
            xml_escape(id),
            xml_escape(name)
        )
        .unwrap();
    }
    for (c, (id, _)) in channels.iter().enumerate() {
        let mut t = from;
        let mut k = 0;
        while t < to {
            writeln!(
                s,
                "<programme start=\"{}\" stop=\"{}\" channel=\"{}\"><title>Show {c}-{k}</title><desc>Invented episode {k}.</desc></programme>",
                xmltv_time(t),
                xmltv_time(t + slot),
                xml_escape(id)
            )
            .unwrap();
            t += slot;
            k += 1;
        }
    }
    s.push_str("</tv>\n");
    s
}

/// `(tvg-id, name)` pairs matching [`m3u`]'s channels.
pub fn m3u_epg_channels(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|i| (format!("ch{i}.example"), format!("Synthetic {i}")))
        .collect()
}

// ---- simulated backends ---------------------------------------------------

/// A request as the simulators see it.
#[derive(Clone, Debug, Default)]
pub struct SimRequest {
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
}

impl SimRequest {
    /// From a full URL (`http://host/path?query`) or a request target.
    pub fn parse(target: &str, headers: Vec<(String, String)>) -> Self {
        let rest = match target.find("://") {
            Some(i) => {
                let after = &target[i + 3..];
                after.find('/').map_or("/", |j| &after[j..])
            }
            None => target,
        };
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let query = query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let (k, v) = p.split_once('=').unwrap_or((p, ""));
                (decode(k), decode(v))
            })
            .collect();
        Self {
            path: path.to_owned(),
            query,
            headers,
        }
    }

    pub fn param(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl SimResponse {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            body: body.into(),
        }
    }

    pub fn status(status: u16) -> Self {
        Self {
            status,
            body: Vec::new(),
        }
    }
}

pub const XT_USER: &str = "demo-user";
pub const XT_PASS: &str = "synthetic-pass";

/// A simulated Xtream panel (`player_api.php`).
#[derive(Clone, Debug)]
pub struct XtreamSim {
    pub channels: usize,
    pub categories: usize,
    pub vod: usize,
    pub series: usize,
    /// `user_info.status`.
    pub status: &'static str,
    pub exp_date: i64,
    /// Answer `get_live_streams` with an `auth: 0` account object.
    pub reject_lists: bool,
    pub fail_categories: Option<u16>,
    pub fail_vod: Option<u16>,
    /// HTTP status for the account probes (`None` = answer).
    pub fail_account: Option<u16>,
}

impl Default for XtreamSim {
    fn default() -> Self {
        Self {
            channels: 50,
            categories: 5,
            vod: 20,
            series: 10,
            status: "Active",
            exp_date: NOW + 30 * 86_400,
            reject_lists: false,
            fail_categories: None,
            fail_vod: None,
            fail_account: None,
        }
    }
}

impl XtreamSim {
    pub fn handle(&self, req: &SimRequest) -> SimResponse {
        if !req.path.ends_with("/player_api.php") {
            return SimResponse::status(404);
        }
        if req.param("username") != Some(XT_USER) || req.param("password") != Some(XT_PASS) {
            return SimResponse::ok(r#"{"user_info":{"auth":0}}"#);
        }
        let cats = |prefix: &str| {
            let items: Vec<String> = (0..self.categories)
                .map(|c| {
                    format!(r#"{{"category_id":"{prefix}{c}","category_name":"{prefix} group {c}","parent_id":0}}"#)
                })
                .collect();
            SimResponse::ok(format!("[{}]", items.join(",")))
        };
        match req.param("action") {
            None | Some("get_account_info" | "get_profile") => match self.fail_account {
                Some(s) => SimResponse::status(s),
                None => SimResponse::ok(format!(
                    "{{\"user_info\":{{\"username\":\"{XT_USER}\",\"password\":\"{XT_PASS}\",\
                     \"auth\":1,\"status\":\"{}\",\"exp_date\":\"{}\",\"is_trial\":\"0\",\
                     \"active_cons\":\"0\",\"max_connections\":\"1\",\
                     \"allowed_output_formats\":[\"m3u8\",\"ts\"]}},\
                     \"server_info\":{{\"url\":\"127.0.0.1\",\"port\":\"80\",\
                     \"server_protocol\":\"http\",\"timezone\":\"UTC\",\
                     \"timestamp_now\":{NOW},\"time_now\":\"2027-01-15 08:00:00\"}}}}",
                    self.status, self.exp_date
                )),
            },
            Some("get_live_categories") => match self.fail_categories {
                Some(s) => SimResponse::status(s),
                None => cats("live"),
            },
            Some("get_vod_categories") => cats("vod"),
            Some("get_series_categories") => cats("series"),
            Some("get_live_streams") if self.reject_lists => {
                SimResponse::ok(r#"{"user_info":{"auth":0}}"#)
            }
            Some("get_live_streams") => {
                let items: Vec<String> = (1..=self.channels)
                    .map(|i| {
                        format!(
                            "{{\"num\":{i},\"name\":\"Synthetic {i}\",\"stream_type\":\"live\",\
                             \"stream_id\":{i},\"stream_icon\":\"http://img.example.com/{i}.png\",\
                             \"epg_channel_id\":\"ch{i}.example\",\"category_id\":\"live{}\",\
                             \"tv_archive\":{},\"tv_archive_duration\":3}}",
                            i % self.categories.max(1),
                            u8::from(i % 10 == 0)
                        )
                    })
                    .collect();
                SimResponse::ok(format!("[{}]", items.join(",")))
            }
            Some("get_vod_streams") => match self.fail_vod {
                Some(s) => SimResponse::status(s),
                None => {
                    let items: Vec<String> = (1..=self.vod)
                        .map(|i| {
                            format!(
                                "{{\"num\":{i},\"name\":\"Film {i}\",\"stream_type\":\"movie\",\
                                 \"stream_id\":{},\"category_id\":\"vod{}\",\
                                 \"container_extension\":\"mp4\",\"rating\":\"7.5\"}}",
                                1000 + i,
                                i % self.categories.max(1)
                            )
                        })
                        .collect();
                    SimResponse::ok(format!("[{}]", items.join(",")))
                }
            },
            Some("get_series") => {
                let items: Vec<String> = (1..=self.series)
                    .map(|i| format!(r#"{{"num":{i},"name":"Serial {i}","series_id":{},"category_id":"series{}"}}"#, 2000 + i, i % self.categories.max(1)))
                    .collect();
                SimResponse::ok(format!("[{}]", items.join(",")))
            }
            Some(_) => SimResponse::ok("[]"),
        }
    }
}

pub const ST_TOKEN: &str = "tok-synthetic-1";

/// A simulated Stalker portal. `api_path` is the working endpoint; other
/// paths answer 404.
#[derive(Debug)]
pub struct StalkerSim {
    pub api_path: &'static str,
    /// Token-free panel: data without a handshake.
    pub simple: bool,
    pub channels: usize,
    pub genres: usize,
    pub page_size: usize,
    /// `get_profile` status (0 ok, 1 blocked, 2 login required).
    pub profile_status: u8,
    /// Reject the token once on this page (then accept a new handshake).
    pub expire_on_page: Option<u32>,
    /// Pages that answer HTTP 500.
    pub failing_pages: Vec<u32>,
    pub vod: usize,
    /// Reject the token once on the first `create_link`.
    pub expire_on_link: bool,
    /// Simulation state (leave at the default).
    pub expired: Cell<bool>,
    pub handshakes: Cell<u32>,
}

impl Default for StalkerSim {
    fn default() -> Self {
        Self {
            api_path: "/stalker_portal/server/load.php",
            simple: false,
            channels: 35,
            genres: 4,
            page_size: 14,
            profile_status: 0,
            expire_on_page: None,
            failing_pages: Vec::new(),
            vod: 0,
            expire_on_link: false,
            expired: Cell::new(false),
            handshakes: Cell::new(0),
        }
    }
}

impl StalkerSim {
    pub fn handshakes(&self) -> u32 {
        self.handshakes.get()
    }

    fn token(&self) -> String {
        format!("{ST_TOKEN}-{}", self.handshakes.get())
    }

    pub fn handle(&self, req: &SimRequest) -> SimResponse {
        if req.path != self.api_path {
            return SimResponse::status(404);
        }
        let action = req.param("action").unwrap_or("");
        let kind = req.param("type").unwrap_or("");
        if !self.simple {
            if kind == "stb" && action == "handshake" {
                self.handshakes.set(self.handshakes.get() + 1);
                self.expired.set(false);
                return SimResponse::ok(format!(
                    r#"{{"js":{{"token":"{}","random":"r4nd0m"}}}}"#,
                    self.token()
                ));
            }
            let bearer = format!("Bearer {}", self.token());
            if req.header("Authorization") != Some(bearer.as_str()) || self.expired.get() {
                return SimResponse::ok("Authorization failed.");
            }
            if kind == "stb" && action == "get_profile" {
                return SimResponse::ok(match self.profile_status {
                    0 => r#"{"js":{"status":0,"watchdog_timeout":60,"fname":"Synthetic"}}"#
                        .to_owned(),
                    1 => r#"{"js":{"status":1,"msg":"","block_msg":"Synthetic account disabled"}}"#
                        .to_owned(),
                    _ => r#"{"js":{"status":2,"msg":"Login required"}}"#.to_owned(),
                });
            }
        }
        match (kind, action) {
            ("itv", "get_genres") => {
                let mut items = vec![r#"{"id":"*","title":"All"}"#.to_owned()];
                items.extend(
                    (1..=self.genres)
                        .map(|g| format!(r#"{{"id":"{g}","title":"Genre {g}","censored":0}}"#)),
                );
                SimResponse::ok(format!(r#"{{"js":[{}]}}"#, items.join(",")))
            }
            ("itv", "get_ordered_list") => {
                let page: u32 = req.param("p").and_then(|p| p.parse().ok()).unwrap_or(1);
                if self.failing_pages.contains(&page) {
                    return SimResponse::status(500);
                }
                if self.expire_on_page == Some(page)
                    && !self.expired.get()
                    && self.handshakes.get() < 2
                {
                    self.expired.set(true);
                    return SimResponse::ok("Authorization failed.");
                }
                let start = (page as usize - 1) * self.page_size;
                let rows: Vec<String> = (start..(start + self.page_size).min(self.channels))
                    .map(|i| {
                        format!(
                            "{{\"id\":\"{}\",\"name\":\"Portal {i}\",\"number\":\"{}\",\
                             \"cmd\":\"ffmpeg http://127.0.0.1/ch/{i}.m3u8\",\"tv_genre_id\":\"{}\",\
                             \"xmltv_id\":\"ch{i}.example\",\"use_http_tmp_link\":\"0\",\
                             \"use_load_balancing\":\"0\"}}",
                            100 + i,
                            i + 1,
                            i % self.genres.max(1) + 1
                        )
                    })
                    .collect();
                SimResponse::ok(format!(
                    r#"{{"js":{{"total_items":"{}","max_page_items":"{}","data":[{}]}}}}"#,
                    self.channels,
                    self.page_size,
                    rows.join(",")
                ))
            }
            ("itv", "create_link") => {
                if self.expire_on_link && !self.expired.get() && self.handshakes.get() < 2 {
                    self.expired.set(true);
                    return SimResponse::ok("Authorization failed.");
                }
                let cmd = req.param("cmd").unwrap_or("");
                let id = cmd
                    .rsplit('/')
                    .next()
                    .and_then(|f| f.split('.').next())
                    .unwrap_or("0");
                SimResponse::ok(format!(
                    r#"{{"js":{{"cmd":"ffmpeg http://127.0.0.1:8091/live/tmp-{id}.m3u8?play_token=t{}"}}}}"#,
                    self.handshakes.get()
                ))
            }
            ("vod", "get_categories") => {
                SimResponse::ok(r#"{"js":[{"id":"*","title":"All"},{"id":"7","title":"Films"}]}"#)
            }
            ("vod", "get_ordered_list") => {
                let page: usize = req.param("p").and_then(|p| p.parse().ok()).unwrap_or(1);
                let start = (page - 1) * self.page_size;
                let rows: Vec<String> = (start..(start + self.page_size).min(self.vod))
                    .map(|i| format!(r#"{{"id":"{}","name":"Film {i}","cmd":"/media/{i}.mpg","category_id":"7","is_series":"0"}}"#, 500 + i))
                    .collect();
                SimResponse::ok(format!(
                    r#"{{"js":{{"total_items":"{}","max_page_items":"{}","data":[{}]}}}}"#,
                    self.vod,
                    self.page_size,
                    rows.join(",")
                ))
            }
            _ => SimResponse::ok(r#"{"js":[]}"#),
        }
    }
}
