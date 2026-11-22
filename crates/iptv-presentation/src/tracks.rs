//! Localized track choices and summaries for playback controls.

use iptv_core::i18n::{self, Language, translate};
use iptv_core::tracks::{AUTO, AudioTrack, Kind, OFF, TextTrack, Tracks, VideoTrack};

/// What the viewer chose for the open stream, as the host's props say it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// [`AUTO`] or a video track id.
    pub video: String,
    /// [`AUTO`] (the player's choice) or an audio track id.
    pub audio: String,
    /// [`OFF`] or a text track id.
    pub text: String,
}

impl Default for Choice {
    fn default() -> Self {
        Choice {
            video: AUTO.to_owned(),
            audio: AUTO.to_owned(),
            text: OFF.to_owned(),
        }
    }
}

/// The chooser's entries for `kind`, in order: "Automatic" then the
/// renditions for video, the tracks for audio, "Off" then the tracks
/// for subtitles.
pub fn entries(tracks: &Tracks, kind: Kind) -> Vec<String> {
    entries_in(tracks, kind, &i18n::LANGUAGES[0])
}

/// Localized app-owned names; provider track labels remain unchanged.
pub fn entries_in(tracks: &Tracks, kind: Kind, language: &Language) -> Vec<String> {
    match kind {
        Kind::Video => std::iter::once(translate(language, "Automatic").to_owned())
            .chain(
                tracks
                    .video
                    .iter()
                    .enumerate()
                    .map(|(i, t)| video_label(t, i, language)),
            )
            .collect(),
        Kind::Audio => tracks
            .audio
            .iter()
            .enumerate()
            .map(|(i, t)| audio_label(t, i, language))
            .collect(),
        Kind::Text => std::iter::once(translate(language, "Off").to_owned())
            .chain(
                tracks
                    .text
                    .iter()
                    .enumerate()
                    .map(|(i, t)| text_label(t, i, language)),
            )
            .collect(),
    }
}

/// The control row's short value for `kind`: the playing resolution,
/// audio layout or subtitle language.
pub fn summary(tracks: &Tracks, kind: Kind) -> Option<String> {
    summary_in(tracks, kind, &i18n::LANGUAGES[0])
}

/// A localized summary without translating provider-originated text.
pub fn summary_in(tracks: &Tracks, kind: Kind, language: &Language) -> Option<String> {
    match kind {
        Kind::Video => {
            let id = tracks.selected_video.as_deref()?;
            let t = tracks.video.iter().find(|t| t.id == id)?;
            (t.height > 0).then(|| format!("{}p", t.height))
        }
        Kind::Audio => {
            let id = tracks.selected_audio.as_deref()?;
            let t = tracks.audio.iter().find(|t| t.id == id)?;
            let lang = language_name_in(&t.language, language);
            match (lang, t.channels) {
                (Some(l), 0) => Some(l),
                (Some(l), n) => Some(format!("{l} {}", audio_layout_in(n, language))),
                (None, 0) => None,
                (None, n) => Some(audio_layout_in(n, language)),
            }
        }
        Kind::Text => {
            let id = tracks.selected_text.as_deref()?;
            let t = tracks.text.iter().find(|t| t.id == id)?;
            Some(
                language_name_in(&t.language, language)
                    .unwrap_or_else(|| translate(language, "On").to_owned()),
            )
        }
    }
}

/// "SD", "HD", "FHD" or "4K" for a picture of `width` × `height`.
pub fn resolution_class(width: u32, height: u32) -> &'static str {
    match (width, height) {
        (w, h) if w >= 3200 || h >= 2000 => "4K",
        (w, h) if w >= 1800 || h >= 1000 => "FHD",
        (w, h) if w >= 1200 || h >= 700 => "HD",
        _ => "SD",
    }
}

/// Whole frames per second (23.976 reads 24).
pub fn fps_label(fps: f32) -> Option<String> {
    (fps.is_finite() && fps > 0.5).then(|| format!("{} fps", fps.round() as u32))
}

/// "Mono", "Stereo", "5.1", "7.1", else the channel count.
pub fn audio_layout(channels: u32) -> String {
    audio_layout_in(channels, &i18n::LANGUAGES[0])
}

/// Localized channel layout, preserving standard surround notation.
pub fn audio_layout_in(channels: u32, language: &Language) -> String {
    match channels {
        1 => translate(language, "Mono").to_owned(),
        2 => translate(language, "Stereo").to_owned(),
        6 => "5.1".to_owned(),
        8 => "7.1".to_owned(),
        n => format!("{n} ch"),
    }
}

/// A short codec name from Media3's MIME type or mpv's codec name.
pub fn codec_name(codec: &str) -> Option<String> {
    let c = codec.trim().to_ascii_lowercase();
    let c = c.strip_prefix("audio/").unwrap_or(&c);
    let name = match c {
        "" | "unknown" => return None,
        "aac" | "mp4a-latm" | "mp4a" | "aac_latm" => "AAC",
        "ac3" | "ac-3" => "AC-3",
        "eac3" | "e-ac-3" => "E-AC-3",
        "mpeg" | "mp3" | "mpeg-l3" => "MP3",
        "mp2" | "mpeg-l2" | "mp2a" => "MP2",
        "opus" => iptv_core::message::key("Opus"),
        "vorbis" => iptv_core::message::key("Vorbis"),
        "flac" => "FLAC",
        "dts" | "vnd.dts" => "DTS",
        other => return Some(other.to_ascii_uppercase()),
    };
    Some(name.to_owned())
}

/// "4.5 Mb/s" or "800 kb/s".
pub fn bitrate_label(bps: u32) -> Option<String> {
    match bps {
        0 => None,
        b if b >= 1_000_000 => Some(i18n::format(
            &i18n::LANGUAGES[0],
            "{rate} Mb/s",
            &[("rate", format!("{:.1}", b as f64 / 1e6).as_str().into())],
        )),
        b => Some(format!("{} kb/s", b.div_ceil(1000))),
    }
}

/// English names of common ISO 639-1/639-2 codes; `None` for none.
/// Unknown codes read as themselves, upper case.
pub fn language_name(code: &str) -> Option<String> {
    language_name_in(code, &i18n::LANGUAGES[0])
}

/// An ISO track language in the interface language. Unknown tags stay uppercase.
pub fn language_name_in(code: &str, language: &Language) -> Option<String> {
    let c = code.trim().to_ascii_lowercase();
    let base = c.split(['-', '_']).next().unwrap_or("");
    let name = match base {
        "" | "und" | "unk" | "mis" | "zxx" => return None,
        "en" | "eng" => translate(language, "English"),
        "fr" | "fra" | "fre" => translate(language, "French"),
        "de" | "deu" | "ger" => translate(language, "German"),
        "es" | "spa" => translate(language, "Spanish"),
        "it" | "ita" => translate(language, "Italian"),
        "pt" | "por" => translate(language, "Portuguese"),
        "nl" | "nld" | "dut" => translate(language, "Dutch"),
        "ru" | "rus" => translate(language, "Russian"),
        "uk" | "ukr" => translate(language, "Ukrainian"),
        "pl" | "pol" => translate(language, "Polish"),
        "cs" | "ces" | "cze" => translate(language, "Czech"),
        "hu" | "hun" => translate(language, "Hungarian"),
        "ro" | "ron" | "rum" => translate(language, "Romanian"),
        "el" | "ell" | "gre" => translate(language, "Greek"),
        "tr" | "tur" => translate(language, "Turkish"),
        "ar" | "ara" => translate(language, "Arabic"),
        "he" | "heb" => translate(language, "Hebrew"),
        "fa" | "fas" | "per" => translate(language, "Persian"),
        "hi" | "hin" => translate(language, "Hindi"),
        "zh" | "zho" | "chi" => translate(language, "Chinese"),
        "ja" | "jpn" => translate(language, "Japanese"),
        "ko" | "kor" => translate(language, "Korean"),
        "sv" | "swe" => translate(language, "Swedish"),
        "da" | "dan" => translate(language, "Danish"),
        "no" | "nor" | "nb" | "nob" => translate(language, "Norwegian"),
        "fi" | "fin" => translate(language, "Finnish"),
        _ => return Some(code.trim().to_ascii_uppercase()),
    };
    Some(name.to_owned())
}

fn joined(parts: impl IntoIterator<Item = Option<String>>) -> String {
    parts.into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

fn video_label(t: &VideoTrack, index: usize, language: &Language) -> String {
    let size = (t.height > 0).then(|| format!("{}p", t.height));
    let label = joined([size, fps_label(t.fps), bitrate_label(t.bitrate)]);
    match (label.is_empty(), t.label.trim()) {
        (false, _) => label,
        (true, "") => i18n::format(
            language,
            "Track {number}",
            &[("number", (index + 1).into())],
        ),
        (true, own) => own.to_owned(),
    }
}

fn audio_label(t: &AudioTrack, index: usize, language: &Language) -> String {
    let own = Some(t.label.trim().to_owned()).filter(|l| !l.is_empty());
    let name = language_name_in(&t.language, language)
        .or(own)
        .unwrap_or_else(|| {
            i18n::format(
                language,
                "Track {number}",
                &[("number", (index + 1).into())],
            )
        });
    let layout = (t.channels > 0).then(|| audio_layout_in(t.channels, language));
    joined([Some(name), layout, codec_name(&t.codec)])
}

fn text_label(t: &TextTrack, index: usize, language: &Language) -> String {
    let own = Some(t.label.trim().to_owned()).filter(|l| !l.is_empty());
    match (language_name_in(&t.language, language), own) {
        (Some(lang), Some(own)) if !own.eq_ignore_ascii_case(&lang) => format!("{lang} · {own}"),
        (Some(lang), _) => lang,
        (None, Some(own)) => own,
        (None, None) => i18n::format(
            language,
            "Track {number}",
            &[("number", (index + 1).into())],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tracks {
        Tracks {
            video: vec![
                VideoTrack {
                    id: "v1".into(),
                    width: 640,
                    height: 360,
                    fps: 25.0,
                    bitrate: 800_000,
                    ..VideoTrack::default()
                },
                VideoTrack {
                    id: "v2".into(),
                    width: 1280,
                    height: 720,
                    fps: 50.0,
                    bitrate: 2_500_000,
                    ..VideoTrack::default()
                },
            ],
            audio: vec![
                AudioTrack {
                    id: "a1".into(),
                    language: "eng".into(),
                    channels: 2,
                    codec: "aac".into(),
                    ..AudioTrack::default()
                },
                AudioTrack {
                    id: "a2".into(),
                    language: "fr".into(),
                    channels: 6,
                    codec: "audio/ac3".into(),
                    ..AudioTrack::default()
                },
            ],
            text: vec![TextTrack {
                id: "t1".into(),
                label: "Forced".into(),
                language: "ger".into(),
            }],
            selected_video: None,
            selected_audio: Some("a2".into()),
            selected_text: None,
        }
    }

    #[test]
    fn localized_tracks_preserve_provider_labels_and_ids() {
        let mut tracks = sample();
        let japanese = i18n::resolve("ja", None);
        assert_eq!(
            entries_in(&tracks, Kind::Audio, japanese)[0],
            "英語 · ステレオ · AAC"
        );
        tracks.selected_text = Some("t1".into());
        assert_eq!(
            summary_in(&tracks, Kind::Text, japanese).as_deref(),
            Some("ドイツ語")
        );
        tracks.text[0].language.clear();
        tracks.text[0].label = "French".into();
        assert_eq!(
            entries_in(&tracks, Kind::Text, japanese)[1],
            "French",
            "provider labels that happen to match a catalog key stay unchanged"
        );
        tracks.text[0].label.clear();
        assert_eq!(entries_in(&tracks, Kind::Text, japanese)[1], "トラック1");
        assert_eq!(tracks.pick(Kind::Text, 1).as_deref(), Some("t1"));
        for language in i18n::LANGUAGES {
            assert_eq!(entries_in(&tracks, Kind::Text, language).len(), 2);
            assert!(!audio_layout_in(1, language).is_empty());
            assert!(!language_name_in("eng", language).unwrap().is_empty());
        }
    }

    #[test]
    fn choosers_list_automatic_tracks_and_off() {
        let t = sample();
        assert_eq!(
            entries(&t, Kind::Video),
            [
                "Automatic",
                "360p · 25 fps · 800 kb/s",
                "720p · 50 fps · 2.5 Mb/s"
            ]
        );
        assert_eq!(
            entries(&t, Kind::Audio),
            ["English · Stereo · AAC", "French · 5.1 · AC-3"]
        );
        assert_eq!(entries(&t, Kind::Text), ["Off", "German · Forced"]);
        assert_eq!(t.selected_entry(Kind::Video), 0, "automatic");
        assert_eq!(t.selected_entry(Kind::Audio), 1);
        assert_eq!(t.selected_entry(Kind::Text), 0, "off");
    }

    #[test]
    fn picks_map_to_the_hosts_props() {
        let t = sample();
        assert_eq!(t.pick(Kind::Video, 0).as_deref(), Some(AUTO));
        assert_eq!(t.pick(Kind::Video, 2).as_deref(), Some("v2"));
        assert_eq!(t.pick(Kind::Video, 3), None);
        assert_eq!(t.pick(Kind::Audio, 0).as_deref(), Some("a1"));
        assert_eq!(t.pick(Kind::Text, 0).as_deref(), Some(OFF));
        assert_eq!(t.pick(Kind::Text, 1).as_deref(), Some("t1"));
        let chosen = Tracks {
            selected_video: Some("v2".into()),
            selected_text: Some("t1".into()),
            ..sample()
        };
        assert_eq!(chosen.selected_entry(Kind::Video), 2);
        assert_eq!(chosen.selected_entry(Kind::Text), 1);
        assert_eq!(summary(&chosen, Kind::Video).as_deref(), Some("720p"));
        assert_eq!(summary(&chosen, Kind::Audio).as_deref(), Some("French 5.1"));
        assert_eq!(summary(&chosen, Kind::Text).as_deref(), Some("German"));
        assert_eq!(summary(&sample(), Kind::Text), None, "off");
    }

    #[test]
    fn unnamed_tracks_read_as_numbers_or_their_own_label() {
        let t = Tracks {
            video: vec![VideoTrack {
                id: "1".into(),
                label: "main".into(),
                ..VideoTrack::default()
            }],
            audio: vec![AudioTrack {
                id: "1".into(),
                language: "und".into(),
                ..AudioTrack::default()
            }],
            text: vec![TextTrack {
                id: "1".into(),
                language: "xx".into(),
                ..TextTrack::default()
            }],
            ..Tracks::default()
        };
        assert_eq!(entries(&t, Kind::Video)[1], "main");
        assert_eq!(entries(&t, Kind::Audio), ["Track 1"]);
        assert_eq!(entries(&t, Kind::Text)[1], "XX");
    }

    #[test]
    fn labels() {
        assert_eq!(resolution_class(720, 576), "SD");
        assert_eq!(resolution_class(1280, 720), "HD");
        assert_eq!(resolution_class(1920, 800), "FHD", "cinemascope by width");
        assert_eq!(resolution_class(3840, 2160), "4K");
        assert_eq!(fps_label(23.976).as_deref(), Some("24 fps"));
        assert_eq!(fps_label(0.0), None);
        assert_eq!(fps_label(f32::NAN), None);
        assert_eq!(audio_layout(2), "Stereo");
        assert_eq!(audio_layout(6), "5.1");
        assert_eq!(audio_layout(3), "3 ch");
        assert_eq!(codec_name("audio/mp4a-latm").as_deref(), Some("AAC"));
        assert_eq!(codec_name("eac3").as_deref(), Some("E-AC-3"));
        assert_eq!(codec_name("pcm_s16le").as_deref(), Some("PCM_S16LE"));
        assert_eq!(codec_name(""), None);
        assert_eq!(bitrate_label(4_500_000).as_deref(), Some("4.5 Mb/s"));
        assert_eq!(bitrate_label(96_000).as_deref(), Some("96 kb/s"));
        assert_eq!(language_name("pt-BR").as_deref(), Some("Portuguese"));
        assert_eq!(language_name(""), None);
    }
}
