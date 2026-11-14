//! Decodes and encodes the shared native player track event.
use iptv_core::tracks::{AUTO, AudioTrack, OFF, TextTrack, Tracks, VideoTrack};
use serde_json::{Value, json};

fn text_of(v: &Value, key: &str) -> String {
    match &v[key] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn num(v: &Value, key: &str) -> u32 {
    v[key]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0)
        .map_or(0, |n| n.min(f64::from(u32::MAX)) as u32)
}

fn list<T>(v: &Value, key: &str, f: impl Fn(&Value) -> T) -> Vec<T> {
    v[key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|t| !text_of(t, "id").is_empty())
                .map(f)
                .collect()
        })
        .unwrap_or_default()
}

/// The `tracks` event's data: `{video: [{id, label, width, height, fps,
/// bitrate}], audio: [{id, label, language, channels, codec}], text: [{id,
/// label, language}], selected: {video: "auto"|id, audio: id|null, text:
/// "off"|id}}`. Missing fields read as unknown.
pub fn parse_tracks(data: &Value) -> Tracks {
    let selected = &data["selected"];
    let pick =
        |key: &str, none: &str| Some(text_of(selected, key)).filter(|s| !s.is_empty() && s != none);
    Tracks {
        video: list(data, "video", |t| VideoTrack {
            id: text_of(t, "id"),
            label: text_of(t, "label"),
            width: num(t, "width"),
            height: num(t, "height"),
            fps: t["fps"]
                .as_f64()
                .filter(|f| f.is_finite() && *f > 0.0)
                .unwrap_or(0.0) as f32,
            bitrate: num(t, "bitrate"),
        }),
        audio: list(data, "audio", |t| AudioTrack {
            id: text_of(t, "id"),
            label: text_of(t, "label"),
            language: text_of(t, "language"),
            channels: num(t, "channels"),
            codec: text_of(t, "codec"),
        }),
        text: list(data, "text", |t| TextTrack {
            id: text_of(t, "id"),
            label: text_of(t, "label"),
            language: text_of(t, "language"),
        }),
        selected_video: pick("video", AUTO),
        selected_audio: pick("audio", ""),
        selected_text: pick("text", OFF),
    }
}

/// The `tracks` event's data for `tracks`, as [`parse_tracks`] reads it
/// back: an unknown number is null, automatic video `auto`, no subtitles
/// `off`. Whatever [`parse_tracks`] read is written so that it reads back
/// the same, so a host may send what it read from another form (mpv's
/// `track-list`) without the app seeing other tracks.
pub(crate) fn tracks_data(tracks: &Tracks) -> Value {
    let known = |n: u32| (n > 0).then_some(n);
    // A rate past `f32` was read as infinite, which JSON cannot say: the
    // largest number it can reads back as that.
    let rate = |fps: f32| (fps > 0.0).then(|| f64::from(fps).min(f64::MAX));
    let video: Vec<Value> = tracks
        .video
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "label": t.label,
                "width": known(t.width),
                "height": known(t.height),
                "fps": rate(t.fps),
                "bitrate": known(t.bitrate),
            })
        })
        .collect();
    let audio: Vec<Value> = tracks
        .audio
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "label": t.label,
                "language": t.language,
                "channels": known(t.channels),
                "codec": t.codec,
            })
        })
        .collect();
    let text: Vec<Value> = tracks
        .text
        .iter()
        .map(|t| json!({ "id": t.id, "label": t.label, "language": t.language }))
        .collect();
    json!({
        "video": video,
        "audio": audio,
        "text": text,
        "selected": {
            "video": tracks.selected_video.as_deref().unwrap_or(AUTO),
            "audio": tracks.selected_audio,
            "text": tracks.selected_text.as_deref().unwrap_or(OFF),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hosts_tracks_event_reads_into_the_model() {
        let t = parse_tracks(&json!({
            "video": [
                {"id": "0:0", "label": "", "width": 640, "height": 360, "fps": 25.0, "bitrate": 800000},
                {"id": "0:1", "width": 1280, "height": 720, "fps": -1, "bitrate": -1}
            ],
            "audio": [
                {"id": 2, "label": "Commentary", "language": "eng", "channels": 2, "codec": "aac"},
                {"label": "no id"}
            ],
            "text": [{"id": "3", "language": "fre"}],
            "selected": {"video": "auto", "audio": "2", "text": "off"}
        }));
        assert_eq!(t.video.len(), 2);
        assert_eq!(
            (t.video[1].height, t.video[1].fps, t.video[1].bitrate),
            (720, 0.0, 0)
        );
        assert_eq!(t.audio.len(), 1, "a track without an id is dropped");
        assert_eq!(t.audio[0].id, "2", "numeric ids read as text");
        assert_eq!(t.text[0].language, "fre");
        assert_eq!(t.selected_video, None, "automatic");
        assert_eq!(t.selected_audio.as_deref(), Some("2"));
        assert_eq!(t.selected_text, None, "off");

        let chosen = parse_tracks(&json!({"selected": {"video": "0:1", "text": "3"}}));
        assert_eq!(chosen.selected_video.as_deref(), Some("0:1"));
        assert_eq!(chosen.selected_text.as_deref(), Some("3"));
        assert_eq!(chosen.selected_audio, None);
        assert_eq!(parse_tracks(&Value::Null), Tracks::default());
    }

    /// What is written reads back the same, unknown values included.
    #[test]
    fn the_tracks_event_is_written_as_it_is_read() {
        let tracks = parse_tracks(&json!({
            "video": [
                {"id": "0:0", "label": "HD", "width": 1280, "height": 720, "fps": 50.0, "bitrate": 2500000},
                {"id": "0:1"}
            ],
            "audio": [{"id": "1:0", "language": "eng", "channels": 2, "codec": "audio/mp4a-latm"}],
            "text": [{"id": "2:0", "label": "Synthetic", "language": "fre"}],
            "selected": {"video": "0:1", "audio": "1:0", "text": "2:0"}
        }));
        let data = tracks_data(&tracks);
        assert_eq!(parse_tracks(&data), tracks);
        assert_eq!(data["video"][1]["width"], Value::Null, "unknown, not 0");
        assert_eq!(data["video"][1]["fps"], Value::Null);
        let none = tracks_data(&Tracks::default());
        assert_eq!(
            none["selected"],
            json!({"video": "auto", "audio": null, "text": "off"})
        );
        assert_eq!(parse_tracks(&none), Tracks::default());
    }

    /// The mpv adapter sends the typed event of what [`parse_tracks`] reads
    /// from its own JSON (`mpv/map.rs`): the app must read the same tracks
    /// from it as it read from that JSON, whatever mpv's numbers are.
    #[test]
    fn writing_what_was_read_changes_no_track() {
        let cases = [
            // mpv's numbers are floats; some are fractions.
            json!({
                "video": [
                    {"id": "1", "label": "", "width": 1280.0, "height": 720.0,
                     "fps": 29.97002997002997, "bitrate": 2400000.0},
                    {"id": "2", "label": "SD", "width": 719.6, "height": 0.4,
                     "fps": 23.976, "bitrate": null}
                ],
                "audio": [
                    {"id": "1", "label": "", "language": "eng", "channels": 5.1, "codec": "ac3"},
                    {"id": "2", "label": "Synthetic", "language": "", "channels": null, "codec": ""}
                ],
                "text": [{"id": "1", "label": "", "language": "ger"}],
                "selected": {"video": "auto", "audio": "2", "text": "off"}
            }),
            // Numbers past what the model holds, and none at all.
            json!({
                "video": [
                    {"id": "1", "width": 1e12, "height": -720, "fps": 1e300, "bitrate": 1e30},
                    {"id": "2", "width": "1280", "fps": 1e-60},
                    {"id": 3, "label": 7, "fps": -25.0}
                ],
                "audio": [{"id": 2.5, "channels": 4294967296.0}, {"label": "no id"}],
                "text": [{"id": ""}, {"id": "9", "language": 0}],
                "selected": {"video": "2", "audio": null, "text": "9"}
            }),
            // What is selected, in every form it was read.
            json!({"selected": {"video": "", "audio": "", "text": ""}}),
            json!({"selected": {"video": 2, "audio": 1, "text": 3}}),
            json!({"selected": {"video": "off", "audio": "auto", "text": "auto"}}),
            json!({"video": "none", "audio": {}, "text": null, "selected": []}),
            Value::Null,
        ];
        for case in cases {
            let read = parse_tracks(&case);
            assert_eq!(parse_tracks(&tracks_data(&read)), read, "{case}");
        }
        // The cases do reach the edges: a rate and a size past the model.
        let edges = parse_tracks(&json!({
            "video": [{"id": "1", "width": 1e12, "fps": 1e300}]
        }));
        assert_eq!(edges.video[0].width, u32::MAX);
        assert!(edges.video[0].fps.is_infinite());
    }
}
