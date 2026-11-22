//! The open stream's tracks as the player host reports them (`tv.player`'s
//! `tracks` event, the same on Media3 and mpv), the viewer's choice for
//! each kind.
//!
//! Track ids are the host's own, opaque strings, valid for the stream that
//! reported them; a new stream starts again from the defaults (video
//! automatic, the player's audio, subtitles off).

/// A video rendition.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoTrack {
    pub id: String,
    pub label: String,
    /// Pixels; 0 when unknown.
    pub width: u32,
    pub height: u32,
    /// Frames per second; 0 when unknown.
    pub fps: f32,
    /// Bits per second; 0 when unknown.
    pub bitrate: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioTrack {
    pub id: String,
    pub label: String,
    /// ISO 639 code as the stream gives it (`en`, `eng`, `fre`); may be empty.
    pub language: String,
    /// 0 when unknown.
    pub channels: u32,
    /// The host's codec name (`aac`, `audio/ac3`, ...); may be empty.
    pub codec: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextTrack {
    pub id: String,
    pub label: String,
    pub language: String,
}

/// Which kind of track a chooser picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
    Text,
}

/// The stream's tracks and what plays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tracks {
    pub video: Vec<VideoTrack>,
    pub audio: Vec<AudioTrack>,
    pub text: Vec<TextTrack>,
    /// The chosen video track; `None` is automatic (the player adapts).
    pub selected_video: Option<String>,
    /// The audio track playing, if the host said.
    pub selected_audio: Option<String>,
    /// The subtitles shown; `None` is off.
    pub selected_text: Option<String>,
}

/// The `videoTrack` prop for automatic selection.
pub const AUTO: &str = "auto";
/// The `textTrack` prop for no subtitles.
pub const OFF: &str = "off";

impl Tracks {
    /// Whether the stream offers a choice of `kind` (one video rendition or
    /// one audio track is no choice; one subtitle track is: on or off).
    pub fn has_choice(&self, kind: Kind) -> bool {
        match kind {
            Kind::Video => self.video.len() > 1,
            Kind::Audio => self.audio.len() > 1,
            Kind::Text => !self.text.is_empty(),
        }
    }

    /// The chooser entry currently in effect for `kind`.
    pub fn selected_entry(&self, kind: Kind) -> usize {
        let at = |list: Vec<&str>, id: &Option<String>| {
            id.as_deref()
                .and_then(|id| list.iter().position(|t| *t == id))
        };
        match kind {
            Kind::Video => at(
                self.video.iter().map(|t| t.id.as_str()).collect(),
                &self.selected_video,
            )
            .map_or(0, |i| i + 1),
            Kind::Audio => at(
                self.audio.iter().map(|t| t.id.as_str()).collect(),
                &self.selected_audio,
            )
            .unwrap_or(0),
            Kind::Text => at(
                self.text.iter().map(|t| t.id.as_str()).collect(),
                &self.selected_text,
            )
            .map_or(0, |i| i + 1),
        }
    }

    /// The prop value for entry `index` of `kind`'s chooser (`None` past
    /// the end).
    pub fn pick(&self, kind: Kind, index: usize) -> Option<String> {
        match (kind, index) {
            (Kind::Video, 0) => Some(AUTO.to_owned()),
            (Kind::Video, i) => self.video.get(i - 1).map(|t| t.id.clone()),
            (Kind::Audio, i) => self.audio.get(i).map(|t| t.id.clone()),
            (Kind::Text, 0) => Some(OFF.to_owned()),
            (Kind::Text, i) => self.text.get(i - 1).map(|t| t.id.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_needs_alternatives() {
        let t = Tracks {
            video: vec![VideoTrack::default(); 2],
            audio: vec![AudioTrack::default(); 2],
            text: vec![TextTrack::default()],
            ..Default::default()
        };
        assert!(t.has_choice(Kind::Video) && t.has_choice(Kind::Audio) && t.has_choice(Kind::Text));
        let single = Tracks {
            video: t.video[..1].to_vec(),
            audio: t.audio[..1].to_vec(),
            text: vec![],
            ..t
        };
        assert!(!single.has_choice(Kind::Video));
        assert!(!single.has_choice(Kind::Audio));
        assert!(!single.has_choice(Kind::Text));
    }
}
