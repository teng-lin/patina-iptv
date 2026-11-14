//! The events of `tv.player`: what a host tells its app, typed. The mpv
//! adapter writes them with [`PlayerEvent::wire`], the shell reads either
//! host's with [`PlayerEvent::decode`].
use std::fmt;

use iptv_core::playback::PlayerState;
use iptv_core::tracks::Tracks;
use serde_json::{Value, json};

use crate::tracks::{parse_tracks, tracks_data};
use crate::wire::wire_enum;

wire_enum! {
    /// Which event: its name on the wire.
    pub enum PlayerEventKind {
        State = "state",
        Size = "size",
        Error = "error",
        Position = "position",
        Tracks = "tracks",
        Format = "format",
        Pip = "pip",
        PipAction = "pipAction",
    }
}

wire_enum! {
    /// A button of Android's picture-in-picture window.
    pub enum PipAction {
        Play = "play",
        Pause = "pause",
        Next = "next",
        Previous = "previous",
    }
}

/// One event from a player.
#[derive(Clone, Debug, PartialEq)]
pub enum PlayerEvent {
    /// `state` {state}.
    State(PlayerState),
    /// `size` {width, height}: the video's, in pixels.
    Size { width: u64, height: u64 },
    /// `error` {code, http?}: Media3's code name (`?` when the host gave
    /// none) and the HTTP status behind it.
    Error { code: String, http: Option<u16> },
    /// `position` {ms}, from where the source opened.
    Position { ms: u64 },
    /// `tracks`: the open stream's, and what plays.
    Tracks(Tracks),
    /// `format` {fps?, channels?}: whichever the host knows by now.
    Format {
        fps: Option<f64>,
        channels: Option<u32>,
    },
    /// `pip` {supported, tv, active}: picture in picture as the host has it.
    Pip {
        supported: bool,
        tv: bool,
        active: bool,
    },
    /// `pipAction` {action}.
    PipAction(PipAction),
}

/// Why an event was not read, with what was in it: the shell still does
/// with these what it did before the events were typed (`video.rs`), so a
/// host outside the contract is seen exactly as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayerEventError {
    /// No event of the contract has this name.
    UnknownName,
    /// A `state` that names none of the contract's states: the name it gave
    /// (`?` when it gave none). Android's host says `unknown` for a Media3
    /// state outside the four.
    UnknownState(String),
    /// A `size` without two whole numbers: each as it came, as JSON.
    NoSize { width: String, height: String },
    /// A `position` without its `ms`.
    NoPosition,
    /// A `pipAction` that names none of the window's buttons: the name it
    /// gave (`?` when it gave none).
    UnknownAction(String),
}

impl fmt::Display for PlayerEventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownName => f.write_str("not an event of the player contract"),
            Self::UnknownState(name) => write!(f, "`state` names no state: {name}"),
            Self::NoSize { width, height } => {
                write!(f, "`size` is not two whole numbers: {width}×{height}")
            }
            Self::NoPosition => f.write_str("`position` has no `ms`"),
            Self::UnknownAction(name) => write!(f, "`pipAction` names no button: {name}"),
        }
    }
}

impl std::error::Error for PlayerEventError {}

impl PlayerEvent {
    pub fn kind(&self) -> PlayerEventKind {
        match self {
            Self::State(_) => PlayerEventKind::State,
            Self::Size { .. } => PlayerEventKind::Size,
            Self::Error { .. } => PlayerEventKind::Error,
            Self::Position { .. } => PlayerEventKind::Position,
            Self::Tracks(_) => PlayerEventKind::Tracks,
            Self::Format { .. } => PlayerEventKind::Format,
            Self::Pip { .. } => PlayerEventKind::Pip,
            Self::PipAction(_) => PlayerEventKind::PipAction,
        }
    }

    /// The event's data as the hosts send it.
    pub fn data(&self) -> Value {
        match self {
            Self::State(state) => json!({ "state": state.name() }),
            Self::Size { width, height } => json!({ "width": width, "height": height }),
            Self::Error { code, http } => {
                let mut data = json!({ "code": code });
                if let Some(status) = http {
                    data["http"] = json!(status);
                }
                data
            }
            Self::Position { ms } => json!({ "ms": ms }),
            Self::Tracks(tracks) => tracks_data(tracks),
            Self::Format { fps, channels } => {
                let mut data = serde_json::Map::new();
                if let Some(fps) = fps {
                    data.insert("fps".into(), json!(fps));
                }
                if let Some(channels) = channels {
                    data.insert("channels".into(), json!(channels));
                }
                Value::Object(data)
            }
            Self::Pip {
                supported,
                tv,
                active,
            } => json!({ "supported": supported, "tv": tv, "active": active }),
            Self::PipAction(action) => json!({ "action": action.wire() }),
        }
    }

    /// Its name and data: what a host emits.
    pub fn wire(&self) -> (&'static str, Value) {
        (self.kind().wire(), self.data())
    }

    /// The event `name` with `data`, from either host. What the contract
    /// marks optional may be missing (`http`, `fps`, `channels`, a track's
    /// details, an unset `pip` flag); an error is never lost for want of a
    /// code. What cannot be read otherwise comes back as the
    /// [`PlayerEventError`] that says what it held.
    pub fn decode(name: &str, data: &Value) -> Result<Self, PlayerEventError> {
        let kind = PlayerEventKind::from_wire(name).ok_or(PlayerEventError::UnknownName)?;
        let flag = |key: &str| data[key].as_bool().unwrap_or(false);
        Ok(match kind {
            PlayerEventKind::State => {
                let name = data["state"].as_str().unwrap_or("?");
                let state = PlayerState::parse(name)
                    .ok_or_else(|| PlayerEventError::UnknownState(name.to_owned()))?;
                Self::State(state)
            }
            PlayerEventKind::Size => {
                let (width, height) = (&data["width"], &data["height"]);
                match (width.as_u64(), height.as_u64()) {
                    // Keep the wire values for diagnostics. The shell narrows
                    // its layout dimensions, as it did before decoding here.
                    (Some(width), Some(height)) => Self::Size { width, height },
                    _ => {
                        return Err(PlayerEventError::NoSize {
                            width: width.to_string(),
                            height: height.to_string(),
                        });
                    }
                }
            }
            PlayerEventKind::Error => Self::Error {
                code: data["code"].as_str().unwrap_or("?").to_owned(),
                http: data["http"].as_u64().and_then(|h| u16::try_from(h).ok()),
            },
            PlayerEventKind::Position => Self::Position {
                ms: data["ms"].as_u64().ok_or(PlayerEventError::NoPosition)?,
            },
            PlayerEventKind::Tracks => Self::Tracks(parse_tracks(data)),
            PlayerEventKind::Format => Self::Format {
                fps: data["fps"].as_f64(),
                channels: data["channels"]
                    .as_u64()
                    .map(|c| u32::try_from(c).unwrap_or(0)),
            },
            PlayerEventKind::Pip => Self::Pip {
                supported: flag("supported"),
                tv: flag("tv"),
                active: flag("active"),
            },
            PlayerEventKind::PipAction => {
                let name = data["action"].as_str().unwrap_or("?");
                let action = PipAction::from_wire(name)
                    .ok_or_else(|| PlayerEventError::UnknownAction(name.to_owned()))?;
                Self::PipAction(action)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use iptv_core::tracks::{AudioTrack, TextTrack, VideoTrack};

    use super::*;

    /// The shared fixture's events, in its order: what the Kotlin host's
    /// test builds from its own typed events (`PlayerContractTest.kt`).
    fn expected() -> Vec<PlayerEvent> {
        let tracks = Tracks {
            video: vec![
                VideoTrack {
                    id: "0:0".into(),
                    label: "HD".into(),
                    width: 1280,
                    height: 720,
                    fps: 50.0,
                    bitrate: 2_500_000,
                },
                VideoTrack {
                    id: "0:1".into(),
                    ..VideoTrack::default()
                },
            ],
            audio: vec![
                AudioTrack {
                    id: "1:0".into(),
                    label: "Commentary".into(),
                    language: "eng".into(),
                    channels: 2,
                    codec: "audio/mp4a-latm".into(),
                },
                AudioTrack {
                    id: "1:1".into(),
                    ..AudioTrack::default()
                },
            ],
            text: vec![TextTrack {
                id: "2:0".into(),
                label: "Synthetic".into(),
                language: "fre".into(),
            }],
            selected_video: Some("0:1".into()),
            selected_audio: Some("1:0".into()),
            selected_text: Some("2:0".into()),
        };
        let error = |code: &str, http| PlayerEvent::Error {
            code: code.to_owned(),
            http,
        };
        let format = |fps, channels| PlayerEvent::Format { fps, channels };
        vec![
            PlayerEvent::State(PlayerState::Idle),
            PlayerEvent::State(PlayerState::Buffering),
            PlayerEvent::State(PlayerState::Ready),
            PlayerEvent::State(PlayerState::Ended),
            PlayerEvent::Size {
                width: 1280,
                height: 720,
            },
            error("ERROR_CODE_IO_BAD_HTTP_STATUS", Some(404)),
            error("ERROR_CODE_IO_NETWORK_CONNECTION_FAILED", None),
            PlayerEvent::Position { ms: 4000 },
            PlayerEvent::Tracks(tracks),
            PlayerEvent::Tracks(Tracks::default()),
            format(Some(25.0), Some(2)),
            format(Some(50.0), None),
            format(None, Some(6)),
            PlayerEvent::Pip {
                supported: true,
                tv: false,
                active: true,
            },
            PlayerEvent::PipAction(PipAction::Play),
            PlayerEvent::PipAction(PipAction::Pause),
            PlayerEvent::PipAction(PipAction::Next),
            PlayerEvent::PipAction(PipAction::Previous),
        ]
    }

    fn fixture() -> Vec<Value> {
        serde_json::from_str(include_str!("../fixtures/player-events.json")).unwrap()
    }

    /// Each fixture event reads as its typed value and is written back as
    /// the same JSON; the fixture leaves no kind of event out.
    #[test]
    fn shared_android_fixture_is_the_exact_rust_event_format() {
        let fixture = fixture();
        let expected = expected();
        assert_eq!(fixture.len(), expected.len());
        for (entry, want) in fixture.iter().zip(&expected) {
            let name = entry["name"].as_str().unwrap();
            assert_eq!(
                entry.as_object().unwrap().len(),
                2,
                "name and data: {entry}"
            );
            let got = PlayerEvent::decode(name, &entry["data"]);
            assert_eq!(got.as_ref(), Ok(want), "{entry}");
            assert_eq!(want.wire(), (name, entry["data"].clone()), "{entry}");
        }
        for kind in PlayerEventKind::ALL {
            assert!(
                expected.iter().any(|e| e.kind() == *kind),
                "no `{}` event in the fixture",
                kind.wire()
            );
        }
        for action in PipAction::ALL {
            let event = PlayerEvent::PipAction(*action);
            assert!(expected.contains(&event), "no `{}` action", action.wire());
        }
    }

    /// What the contract marks optional may be missing.
    #[test]
    fn optional_event_data_may_be_missing() {
        let decode = |name, data: Value| PlayerEvent::decode(name, &data);
        assert_eq!(
            decode("error", json!({})),
            Ok(PlayerEvent::Error {
                code: "?".into(),
                http: None
            }),
            "an error is never lost"
        );
        assert_eq!(
            decode("error", json!({ "code": "E", "http": 70_000 })),
            Ok(PlayerEvent::Error {
                code: "E".into(),
                http: None
            }),
            "not an HTTP status"
        );
        assert_eq!(
            decode("format", json!({})),
            Ok(PlayerEvent::Format {
                fps: None,
                channels: None
            })
        );
        assert_eq!(
            decode("pip", json!({ "supported": true })),
            Ok(PlayerEvent::Pip {
                supported: true,
                tv: false,
                active: false
            })
        );
        assert_eq!(
            decode("tracks", Value::Null),
            Ok(PlayerEvent::Tracks(Tracks::default()))
        );
    }

    /// A name outside the contract is told apart from an event of it that
    /// cannot be read, and each of those says what it held: the shell does
    /// with it what it did before the events were typed.
    #[test]
    fn what_cannot_be_read_says_what_it_held() {
        let decode = |name, data: Value| PlayerEvent::decode(name, &data);
        let state = |name: &str| Err(PlayerEventError::UnknownState(name.to_owned()));
        let size = |width: &str, height: &str| {
            Err(PlayerEventError::NoSize {
                width: width.to_owned(),
                height: height.to_owned(),
            })
        };
        let action = |name: &str| Err(PlayerEventError::UnknownAction(name.to_owned()));
        assert_eq!(
            decode("unknown", json!({})),
            Err(PlayerEventError::UnknownName)
        );
        assert_eq!(
            decode("pipaction", json!({ "action": "play" })),
            Err(PlayerEventError::UnknownName),
            "names are exact"
        );

        // Android's host says `unknown` for a Media3 state outside the
        // four: the name stays, for the status line.
        assert_eq!(
            decode("state", json!({ "state": "unknown" })),
            state("unknown")
        );
        assert_eq!(decode("state", json!({ "state": "Ready" })), state("Ready"));
        assert_eq!(decode("state", json!({})), state("?"));
        assert_eq!(decode("state", json!({ "state": 3 })), state("?"));

        // A size is two whole numbers; anything else is kept as it came.
        assert_eq!(decode("size", json!({})), size("null", "null"));
        assert_eq!(
            decode("size", json!({ "width": 1280 })),
            size("1280", "null")
        );
        assert_eq!(
            decode("size", json!({ "width": 1280.5, "height": "720" })),
            size("1280.5", "\"720\"")
        );
        assert_eq!(
            decode("size", json!({ "width": -1, "height": 720 })),
            size("-1", "720")
        );

        assert_eq!(
            decode("position", json!({})),
            Err(PlayerEventError::NoPosition)
        );
        assert_eq!(
            decode("position", json!({ "ms": -1 })),
            Err(PlayerEventError::NoPosition)
        );
        assert_eq!(
            decode("pipAction", json!({ "action": "rewind" })),
            action("rewind")
        );
        assert_eq!(decode("pipAction", json!({})), action("?"));
        assert_eq!(
            PlayerEventError::UnknownAction("rewind".into()).to_string(),
            "`pipAction` names no button: rewind"
        );
    }

    /// The shell never asked a size to be above zero or to fit 32 bits:
    /// any two whole numbers are a size, as they were.
    #[test]
    fn any_two_whole_numbers_are_a_size() {
        let decode = |width: u64, height: u64| {
            PlayerEvent::decode("size", &json!({ "width": width, "height": height }))
        };
        let size = |width, height| Ok(PlayerEvent::Size { width, height });
        assert_eq!(decode(0, 720), size(0, 720));
        assert_eq!(decode(0, 0), size(0, 0));
        assert_eq!(decode(u64::from(u32::MAX), 1), size(u64::from(u32::MAX), 1));
        assert_eq!(decode((1 << 32) + 1280, 720), size((1 << 32) + 1280, 720));
    }
}
