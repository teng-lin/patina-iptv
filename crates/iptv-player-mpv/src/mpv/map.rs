//! What the Android host's props and events mean in mpv's terms: picture
//! sizes as mpv options, HTTP headers as mpv's, and mpv's failures as the
//! Media3 error-code names `iptv_core::playback::ErrorKind` classifies.
//! Pure functions, tested without mpv.

/// How mpv shows a picture-size mode (`pictureSize` prop): the same
/// rectangle `PictureFrame.pictureSize()` gives the Android surface, clipped
/// by the view.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    /// mpv `keepaspect`: `false` stretches to the view.
    pub keepaspect: bool,
    /// mpv `panscan`: 1 crops to fill the view (cover).
    pub panscan: f64,
    /// mpv `video-aspect-override` (`"no"`: the video's own).
    pub aspect: &'static str,
    /// mpv `video-zoom` (log2 of the scale of the fitted picture).
    pub zoom: f64,
}

/// mpv's options for picture-size `mode` with video of display aspect
/// `video_aspect` (16:9 until known, as on Android).
pub fn picture(mode: &str, video_aspect: Option<f64>) -> Picture {
    let va = video_aspect
        .filter(|a| a.is_finite() && *a > 0.0)
        .unwrap_or(16.0 / 9.0);
    let fit = Picture {
        keepaspect: true,
        panscan: 0.0,
        aspect: "no",
        zoom: 0.0,
    };
    match mode {
        "full" => Picture {
            keepaspect: false,
            ..fit
        },
        "zoom" => Picture {
            panscan: 1.0,
            ..fit
        },
        // A wide film letterboxed in the video: zoom until it fills the frame.
        "cinema" => Picture {
            zoom: (2.35 / va).max(1.0).log2(),
            ..fit
        },
        "reduced" => Picture {
            zoom: 0.85_f64.log2(),
            ..fit
        },
        "4:3" => Picture {
            aspect: "4:3",
            ..fit
        },
        "16:9" => Picture {
            aspect: "16:9",
            ..fit
        },
        "2.35:1" => Picture {
            aspect: "2.35:1",
            ..fit
        },
        _ => fit,
    }
}

/// The HTTP options of one stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Http {
    /// mpv `user-agent`: the source's, else the player's own
    /// ([`super::DEFAULT_UA`]).
    pub user_agent: String,
    /// mpv `http-header-fields` entries (`Name: value`), User-Agent aside.
    pub fields: Vec<String>,
}

/// Cleans stream headers for the origin-scoped playback transport. These
/// fields are never installed as mpv's global HTTP headers. CR/LF/NUL values
/// that could split a request are dropped. A source that names no
/// `User-Agent`, or a blank one, gets the player's own: the app puts its
/// setting in the source's headers, so there is nothing else to fall back to.
pub fn http(headers: &[(String, String)]) -> Http {
    let clean = |s: &str| !s.contains(['\r', '\n', '\0']);
    let mut user_agent = None;
    let mut fields = Vec::new();
    for (name, value) in headers {
        let name = name.trim();
        if name.is_empty() || name.contains(':') || !clean(name) || !clean(value) {
            continue;
        }
        if name.eq_ignore_ascii_case("user-agent") {
            user_agent = Some(value.clone()).filter(|v| !v.trim().is_empty());
        } else {
            fields.push(format!("{name}: {value}"));
        }
    }
    Http {
        user_agent: user_agent.unwrap_or_else(|| super::DEFAULT_UA.to_owned()),
        fields,
    }
}

/// Whether a reopen keeps the picture's last frame instead of the black
/// shutter: the app reconnects the stream that played (`reconnect`; a
/// portal may hand out a new address for it), or only the token changed
/// (a retry of the same url). A new channel, catch-up offset or stop goes
/// black until the new stream plays.
pub fn keeps_frame(old: Option<&str>, new: Option<&str>, reconnect: bool) -> bool {
    match (old, new) {
        (Some(old), Some(new)) => reconnect || old == new,
        _ => false,
    }
}

/// Whether a failed stream keeps its last frame: a live one does (the app
/// reconnects it and shows "Reconnecting…" or its card over the frame);
/// catch-up and anything else go black.
pub fn keeps_frame_on_error(live: bool) -> bool {
    live
}

/// How much a live stream's cache reads ahead, s. A live stream cannot
/// read past its live edge, so this is a cap, not a wait.
pub const LIVE_CACHE_SECS: u32 = 20;

/// mpv's options for one load (set as properties before `loadfile`).
/// The cache is on for every stream, and playback starts as soon as the
/// first frame decodes (`cache-pause-initial=no`: zapping waits for
/// nothing more). A live stream keeps up to [`LIVE_CACHE_SECS`] ahead and
/// after running dry waits for 2 s of it (not 1), so a stream with
/// hiccups settles a little further behind live instead of stuttering. A
/// reopen of a live stream (`reconnect`) starts three HLS segments back
/// from the newest, as a first open does, rather than at the live edge;
/// every load clears the spare's options of the bridge
/// ([`super::worker`]).
pub fn load_options(live: bool, reconnect: bool) -> Vec<(&'static str, String)> {
    let (cache_secs, pause_wait) = if live {
        (LIVE_CACHE_SECS.to_string(), "2")
    } else {
        // mpv's defaults: catch-up reads ahead as far as memory allows.
        ("1000".to_owned(), "1")
    };
    let lavf = if live && reconnect {
        "live_start_index=-3"
    } else {
        ""
    };
    vec![
        ("cache", "yes".to_owned()),
        ("cache-pause-initial", "no".to_owned()),
        ("cache-secs", cache_secs),
        ("cache-pause-wait", pause_wait.to_owned()),
        ("demuxer-lavf-o", lavf.to_owned()),
    ]
}

/// Whether mpv may open `url`: network streams only (never local files,
/// FFmpeg's pseudo-protocols or mpv's own `av://`, `lavfi://`...).
pub fn playable(url: &str) -> bool {
    const SCHEMES: &[&str] = &["http", "https"];
    url.split_once("://")
        .is_some_and(|(scheme, _)| SCHEMES.iter().any(|s| s.eq_ignore_ascii_case(scheme)))
}

/// mpv's `vid`/`aid` for a `videoTrack`/`audioTrack` prop: `auto`, or a
/// track id (mpv's are numbers).
pub fn track_choice(value: Option<&str>) -> String {
    match value.map(str::trim) {
        Some(id) if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) => id.to_owned(),
        _ => "auto".to_owned(),
    }
}

/// mpv's `sid` for a `textTrack` prop: `no` (off), or a track id.
pub fn text_choice(value: Option<&str>) -> String {
    match track_choice(value) {
        id if id == "auto" => "no".to_owned(),
        id => id,
    }
}

/// The `tracks` event for mpv's `track-list` (JSON): its video, audio and
/// subtitle tracks (no cover art), and what plays; `video_auto`: the
/// viewer left the video to mpv. `None` when the JSON is not a list.
pub fn tracks(track_list: &str, video_auto: bool) -> Option<serde_json::Value> {
    use serde_json::{Value, json};
    let list: Vec<Value> = serde_json::from_str(track_list).ok()?;
    let (mut video, mut audio, mut text) = (Vec::new(), Vec::new(), Vec::new());
    let (mut sv, mut sa, mut st) = (None, None, None);
    let num = |v: &Value| v.as_f64().filter(|n| n.is_finite() && *n > 0.0);
    for t in &list {
        let Some(id) = t["id"].as_i64() else { continue };
        let id = id.to_string();
        let selected = t["selected"].as_bool() == Some(true);
        let label = t["title"].as_str().unwrap_or("");
        let language = t["lang"].as_str().unwrap_or("");
        let at = match t["type"].as_str() {
            Some("video") if t["image"].as_bool() != Some(true) => {
                video.push(json!({
                    "id": id,
                    "label": label,
                    "width": num(&t["demux-w"]),
                    "height": num(&t["demux-h"]),
                    "fps": num(&t["demux-fps"]),
                    "bitrate": num(&t["hls-bitrate"]).or_else(|| num(&t["demux-bitrate"])),
                }));
                &mut sv
            }
            Some("audio") => {
                audio.push(json!({
                    "id": id,
                    "label": label,
                    "language": language,
                    "channels": num(&t["demux-channel-count"])
                        .or_else(|| num(&t["audio-channels"])),
                    "codec": t["codec"].as_str().unwrap_or(""),
                }));
                &mut sa
            }
            Some("sub") => {
                text.push(json!({ "id": id, "label": label, "language": language }));
                &mut st
            }
            _ => continue,
        };
        if selected {
            *at = Some(id);
        }
    }
    let video_selected = match (video_auto, sv) {
        (false, Some(id)) => id,
        _ => "auto".to_owned(),
    };
    Some(json!({
        "video": video,
        "audio": audio,
        "text": text,
        "selected": {
            "video": video_selected,
            "audio": sa,
            "text": st.unwrap_or_else(|| "off".to_owned()),
        },
    }))
}

/// What mpv's log said about the current file's failure (its messages carry
/// no codes the API reports: FFmpeg's HTTP status and socket errors come
/// only as text). Holds no text, so no address can leak from it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clues {
    /// The relay's own verdict on this load. It is authoritative: the relay
    /// made the upstream request, while the log only shows what the relay
    /// answered the player with (a 415 or 502 standing in for the cause).
    transport: Option<&'static str>,
    http: Option<u16>,
    network: Option<&'static str>,
    format: bool,
}

pub const HTTP_STATUS: &str = "ERROR_CODE_IO_BAD_HTTP_STATUS";
pub const CONNECTION_FAILED: &str = "ERROR_CODE_IO_NETWORK_CONNECTION_FAILED";
pub const CONNECTION_TIMEOUT: &str = "ERROR_CODE_IO_NETWORK_CONNECTION_TIMEOUT";
pub const CONTAINER_UNSUPPORTED: &str = "ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED";
pub const FORMAT_UNSUPPORTED: &str = "ERROR_CODE_DECODING_FORMAT_UNSUPPORTED";
pub const DECODER_INIT_FAILED: &str = "ERROR_CODE_DECODER_INIT_FAILED";
pub const IO_UNSPECIFIED: &str = "ERROR_CODE_IO_UNSPECIFIED";
pub const UNSPECIFIED: &str = "ERROR_CODE_UNSPECIFIED";
/// Not one of Media3's: an address mpv is not allowed to open
/// ([`playable`]); classified as a format failure (no retry).
pub const UNSUPPORTED_SCHEME: &str = "ERROR_CODE_UNSUPPORTED_SCHEME";

/// The 3-digit status after `marker` in `text`.
fn status_after(text: &str, marker: &str) -> Option<u16> {
    let rest = &text[text.find(marker)? + marker.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (digits.len() == 3).then(|| digits.parse().ok()).flatten()
}

impl Clues {
    /// Records the relay's error for this load. It wins over whatever the
    /// log says, whichever arrives first: the log line for the relay's
    /// stand-in status can be delivered after the end-of-file event that
    /// reports the relay's error. A new load starts from fresh clues
    /// (`Player::open`), so this cannot outlive the load it describes.
    pub fn transport_error(&mut self, code: &'static str) {
        self.transport = Some(code);
    }

    /// The load went on to its next entry (a live stream handed over to
    /// its spare): what the relay said about the connection that closed
    /// does not describe a later failure of the new one. The relay reports
    /// again if the new connection fails.
    pub fn handed_over(&mut self) {
        self.transport = None;
    }

    /// Reads one log message.
    pub fn hear(&mut self, text: &str) {
        if let Some(status) =
            status_after(text, "HTTP error ").or_else(|| status_after(text, "Server returned "))
            && status >= 400
        {
            self.http = Some(status);
            return;
        }
        let t = text.to_ascii_lowercase();
        if t.contains("timed out") {
            self.network = Some(CONNECTION_TIMEOUT);
        } else if [
            "connection refused",
            "failed to resolve",
            "network is unreachable",
            "host is unreachable",
            "no route to host",
            "connection reset",
        ]
        .iter()
        .any(|m| t.contains(m))
        {
            self.network = Some(CONNECTION_FAILED);
        } else if t.contains("failed to recognize file format") {
            self.format = true;
        }
    }

    /// The `error` event for a file that ended with mpv error `error`:
    /// Media3's code name and the HTTP status, if a server answered one.
    pub fn error(&self, error: i32) -> (&'static str, Option<u16>) {
        use super::ffi::{
            MPV_ERROR_LOADING_FAILED, MPV_ERROR_NOTHING_TO_PLAY, MPV_ERROR_UNKNOWN_FORMAT,
        };
        if let Some(code) = self.transport {
            return (code, None);
        }
        if let Some(status) = self.http {
            return (HTTP_STATUS, Some(status));
        }
        if let Some(code) = self.network {
            return (code, None);
        }
        let code = match error {
            _ if self.format => CONTAINER_UNSUPPORTED,
            MPV_ERROR_UNKNOWN_FORMAT => CONTAINER_UNSUPPORTED,
            MPV_ERROR_NOTHING_TO_PLAY => FORMAT_UNSUPPORTED,
            MPV_ERROR_LOADING_FAILED => IO_UNSPECIFIED,
            _ => UNSPECIFIED,
        };
        (code, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_core::playback::ErrorKind;

    fn kind(clues: &Clues, error: i32) -> ErrorKind {
        let (code, http) = clues.error(error);
        ErrorKind::from_player_code(code, http)
    }

    #[test]
    fn only_a_reopen_of_the_same_stream_keeps_the_last_frame() {
        let (a, b) = (
            Some("http://a.test/live/1.ts"),
            Some("http://a.test/live/2.ts"),
        );
        assert!(keeps_frame(a, a, false), "a retry: only the token changed");
        assert!(
            keeps_frame(a, b, true),
            "the app reconnects (a new portal link)"
        );
        assert!(
            !keeps_frame(a, b, false),
            "another channel or catch-up offset"
        );
        assert!(!keeps_frame(None, a, false), "the first stream");
        assert!(!keeps_frame(None, a, true));
        assert!(!keeps_frame(a, None, true), "a stop");
    }

    #[test]
    fn a_live_error_keeps_the_last_frame_and_catch_up_goes_black() {
        assert!(keeps_frame_on_error(true));
        assert!(!keeps_frame_on_error(false));
    }

    fn option<'a>(options: &'a [(&'static str, String)], name: &str) -> Option<&'a str> {
        options
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn load_options_buffer_live_streams_without_delaying_the_first_frame() {
        for (live, reconnect) in [(true, false), (true, true), (false, false), (false, true)] {
            let o = load_options(live, reconnect);
            assert_eq!(option(&o, "cache"), Some("yes"));
            assert_eq!(
                option(&o, "cache-pause-initial"),
                Some("no"),
                "zapping waits for the first frame only"
            );
            let names: Vec<_> = o.iter().map(|(n, _)| *n).collect();
            let mut unique = names.clone();
            unique.dedup();
            assert_eq!(names, unique);
        }
        let live = load_options(true, false);
        assert_eq!(option(&live, "cache-secs"), Some("20"));
        assert_eq!(option(&live, "cache-pause-wait"), Some("2"));
        assert_eq!(
            option(&live, "demuxer-lavf-o"),
            Some(""),
            "a first open: ffmpeg's own start, and no spare's options"
        );
        let again = load_options(true, true);
        assert_eq!(
            option(&again, "demuxer-lavf-o"),
            Some("live_start_index=-3"),
            "a reopen starts segments back, not at the live edge"
        );
        let archive = load_options(false, true);
        assert_eq!(option(&archive, "cache-secs"), Some("1000"));
        assert_eq!(option(&archive, "cache-pause-wait"), Some("1"));
        assert_eq!(option(&archive, "demuxer-lavf-o"), Some(""));
    }

    #[test]
    fn picture_sizes_match_the_android_frame() {
        assert_eq!(picture("normal", Some(1.5)), picture("anything", None));
        assert!(!picture("full", None).keepaspect);
        assert_eq!(picture("zoom", None).panscan, 1.0);
        assert_eq!(picture("4:3", None).aspect, "4:3");
        assert_eq!(picture("16:9", None).aspect, "16:9");
        assert_eq!(picture("2.35:1", None).aspect, "2.35:1");
        assert!((picture("reduced", None).zoom.exp2() - 0.85).abs() < 1e-9);
        // Cinema zooms a 16:9 picture by 2.35 / (16/9); never shrinks.
        let cinema = picture("cinema", Some(16.0 / 9.0)).zoom.exp2();
        assert!((cinema - 2.35 * 9.0 / 16.0).abs() < 1e-9);
        assert_eq!(picture("cinema", Some(2.4)).zoom, 0.0);
    }

    #[test]
    fn headers_become_mpv_options() {
        let h = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<Vec<_>>()
        };
        let got = http(&h(&[
            ("user-agent", "Box/1"),
            ("Referer", "http://example.test/"),
            ("Cookie", "a=1, b=2"),
            ("X-Bad", "v\r\nInjected: 1"),
            ("", "empty"),
        ]));
        assert_eq!(got.user_agent, "Box/1");
        assert_eq!(
            got.fields,
            ["Referer: http://example.test/", "Cookie: a=1, b=2"]
        );
        // No agent of the source's, or a blank one: the player's own. It
        // is never a request header field of its own.
        let own = crate::mpv::DEFAULT_UA;
        assert_eq!(http(&[]).user_agent, own);
        let blank = http(&h(&[("User-Agent", "  "), ("X-Synthetic", "1")]));
        assert_eq!(blank.user_agent, own);
        assert_eq!(blank.fields, ["X-Synthetic: 1"]);
        let split = http(&h(&[("User-Agent", "Box/1\r\nInjected: 1")]));
        assert_eq!(split.user_agent, own, "a value that splits is dropped");
    }

    #[test]
    fn only_network_streams_open() {
        assert!(playable("http://127.0.0.1:8089/ch0/index.m3u8"));
        assert!(playable("HTTPS://example.test/live.ts"));

        for url in [
            "udp://@239.0.0.1:1234",
            "mms://example.test/channel",
            "file:///etc/hosts",
            "/etc/hosts",
            "av://lavfi:sine",
            "concat:a|b",
            "",
        ] {
            assert!(!playable(url), "{url}");
        }
    }

    fn gone_404() -> Clues {
        let mut c = Clues::default();
        c.hear("http: HTTP error 404 Not Found");
        c
    }

    #[test]
    fn failures_classify_as_on_android() {
        let mut c = Clues::default();
        c.hear("http: HTTP error 404 Not Found");
        assert_eq!(c.error(MPV_LOADING), (HTTP_STATUS, Some(404)));
        assert_eq!(kind(&c, MPV_LOADING), ErrorKind::Http(404));

        let mut c = Clues::default();
        c.hear("Server returned 403 Forbidden (access denied)");
        assert_eq!(kind(&c, MPV_LOADING), ErrorKind::Http(403));
        assert_eq!(
            kind(&c, MPV_LOADING).cause(),
            iptv_core::playback::Cause::Refused,
            "refused for now: retried"
        );
        assert!(
            !kind(&gone_404(), MPV_LOADING).retryable(),
            "gone: no retry"
        );

        let mut c = Clues::default();
        c.hear("tcp: Connection to tcp://127.0.0.1:9 failed: Connection refused");
        assert_eq!(c.error(MPV_LOADING).0, CONNECTION_FAILED);
        assert_eq!(kind(&c, MPV_LOADING), ErrorKind::Network);

        let mut c = Clues::default();
        c.hear("tcp: Connection to tcp://192.0.2.1:80 failed: Connection timed out");
        assert_eq!(c.error(MPV_LOADING).0, CONNECTION_TIMEOUT);
        assert_eq!(kind(&c, MPV_LOADING), ErrorKind::Network);

        let mut c = Clues::default();
        c.hear("Failed to recognize file format.");
        assert_eq!(kind(&c, MPV_LOADING), ErrorKind::Format);

        let c = Clues::default();
        assert_eq!(
            kind(&c, super::super::ffi::MPV_ERROR_UNKNOWN_FORMAT),
            ErrorKind::Format
        );
        assert_eq!(
            kind(&c, super::super::ffi::MPV_ERROR_NOTHING_TO_PLAY),
            ErrorKind::Format
        );
        assert_eq!(kind(&c, MPV_LOADING), ErrorKind::Network);
        assert_eq!(kind(&c, -20), ErrorKind::Other);
        assert_eq!(
            ErrorKind::from_player_code(UNSUPPORTED_SCHEME, None),
            ErrorKind::Format
        );
        assert_eq!(
            ErrorKind::from_player_code(DECODER_INIT_FAILED, None),
            ErrorKind::Format
        );
        // A 2xx/3xx in a message is no failure status.
        let mut c = Clues::default();
        c.hear("HTTP error 302 Found");
        assert_eq!(c.error(MPV_LOADING).1, None);
    }

    const MPV_LOADING: i32 = super::super::ffi::MPV_ERROR_LOADING_FAILED;

    #[test]
    fn the_relays_error_wins_over_the_logged_status_in_either_order() {
        // The relay refuses a playlist it cannot classify and answers the
        // player 415; FFmpeg logs that status.
        let logged = "http: HTTP error 415 Unsupported Media Type";

        let mut log_first = Clues::default();
        log_first.hear(logged);
        log_first.transport_error(CONTAINER_UNSUPPORTED);
        assert_eq!(log_first.error(MPV_LOADING), (CONTAINER_UNSUPPORTED, None));
        assert_eq!(kind(&log_first, MPV_LOADING), ErrorKind::Format);

        let mut log_last = Clues::default();
        log_last.transport_error(CONTAINER_UNSUPPORTED);
        log_last.hear(logged);
        assert_eq!(log_last.error(MPV_LOADING), (CONTAINER_UNSUPPORTED, None));
        assert_eq!(kind(&log_last, MPV_LOADING), ErrorKind::Format);

        // Other log clues do not displace it either.
        let mut timed_out = Clues::default();
        timed_out.transport_error(CONNECTION_FAILED);
        timed_out.hear("tcp: Connection timed out");
        timed_out.hear("Failed to recognize file format.");
        assert_eq!(timed_out.error(MPV_LOADING), (CONNECTION_FAILED, None));

        // The relay's latest report replaces its earlier one.
        let mut again = Clues::default();
        again.transport_error(CONNECTION_FAILED);
        again.transport_error(CONNECTION_TIMEOUT);
        assert_eq!(again.error(MPV_LOADING), (CONNECTION_TIMEOUT, None));

        // A live stream that handed over to its spare left the old
        // connection's error behind: a later status is the new one's.
        let mut bridged = Clues::default();
        bridged.transport_error(CONNECTION_TIMEOUT);
        bridged.handed_over();
        bridged.hear("http: HTTP error 403 Forbidden");
        assert_eq!(bridged.error(MPV_LOADING), (HTTP_STATUS, Some(403)));

        // Without a relay error the logged status is still what is shown,
        // and the next load starts from nothing.
        let mut plain = Clues::default();
        plain.hear("http: HTTP error 403 Forbidden");
        assert_eq!(plain.error(MPV_LOADING), (HTTP_STATUS, Some(403)));
        assert_eq!(Clues::default().error(MPV_LOADING), (IO_UNSPECIFIED, None));
    }

    #[test]
    fn track_props_become_mpv_values() {
        assert_eq!(track_choice(Some("3")), "3");
        assert_eq!(track_choice(Some("auto")), "auto");
        assert_eq!(track_choice(Some("2; quit")), "auto");
        assert_eq!(track_choice(None), "auto");
        assert_eq!(text_choice(Some("off")), "no");
        assert_eq!(text_choice(Some("4")), "4");
    }

    #[test]
    fn mpvs_track_list_becomes_the_tracks_event() {
        let list = r#"[
            {"id":1,"type":"video","selected":false,"demux-w":640,"demux-h":360,"demux-fps":25,"hls-bitrate":800000},
            {"id":2,"type":"video","selected":true,"demux-w":1280,"demux-h":720,"demux-fps":25,"demux-bitrate":2400000},
            {"id":3,"type":"video","image":true,"albumart":true},
            {"id":1,"type":"audio","selected":false,"lang":"eng","demux-channel-count":2,"codec":"aac"},
            {"id":2,"type":"audio","selected":true,"lang":"fre","title":"Version française","audio-channels":6,"codec":"ac3"},
            {"id":1,"type":"sub","selected":false,"lang":"ger","codec":"webvtt"}
        ]"#;
        let auto = tracks(list, true).unwrap();
        let t = crate::parse_tracks(&auto);
        assert_eq!(t.video.len(), 2, "cover art is no rendition");
        assert_eq!(
            (t.video[0].bitrate, t.video[1].bitrate),
            (800_000, 2_400_000)
        );
        assert_eq!(
            (t.audio[1].channels, t.audio[1].label.as_str()),
            (6, "Version française")
        );
        assert_eq!(t.selected_video, None, "left to mpv");
        assert_eq!(t.selected_audio.as_deref(), Some("2"));
        assert_eq!(t.selected_text, None);
        let chosen = crate::parse_tracks(&tracks(list, false).unwrap());
        assert_eq!(chosen.selected_video.as_deref(), Some("2"));
        assert!(tracks("{}", true).is_none());
        let empty = crate::parse_tracks(&tracks("[]", true).unwrap());
        assert_eq!(empty, iptv_core::tracks::Tracks::default());
    }
}
