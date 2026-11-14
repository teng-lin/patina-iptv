//! What the player's info card and top bar say: the stream badges
//! (resolution class, frame rate, audio layout), the minutes left of a
//! programme and the date with the time. Pure functions of the values the
//! caller passes (no clock access here).

use crate::clock::Style;
use crate::i18n::{self, Language};
use crate::tracks::{audio_layout, fps_label, resolution_class};

/// What the player reports of the stream that plays.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StreamFormat {
    /// Picture size (`size` event); 0 while unknown.
    pub width: u32,
    pub height: u32,
    /// Frames per second (`format` event); 0 while unknown.
    pub fps: f32,
    /// Audio channels (`format` event); 0 while unknown.
    pub channels: u32,
}

/// The card's badges, in order: resolution class, frame rate, audio
/// layout; each only once the player knows it.
pub fn badges(f: &StreamFormat) -> Vec<String> {
    let mut out = Vec::new();
    if f.width > 0 && f.height > 0 {
        out.push(resolution_class(f.width, f.height).to_owned());
    }
    out.extend(fps_label(f.fps));
    if f.channels > 0 {
        out.push(audio_layout(f.channels));
    }
    out
}

/// "25 min left" in `language`, for a programme ending at guide minute
/// `end` at `now` (`None` once it ended or before it starts).
pub fn minutes_left(language: &Language, start: i32, end: i32, now: i32) -> Option<String> {
    (start <= now && now < end).then(|| {
        let minutes = u64::from((end - now).unsigned_abs());
        i18n::format(
            language,
            "{minutes} min left",
            &[("minutes", minutes.into())],
        )
    })
}

/// "Tue 10 Nov  18:00": local date and time of `unix` at a UTC offset of
/// `offset_minutes`, in the language and on the clock of `style`.
pub fn date_time(unix: i64, offset_minutes: i32, style: Style) -> String {
    crate::clock::date_time(unix, offset_minutes, style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::SYNTHETIC_EPOCH_UNIX as EPOCH;

    #[test]
    fn badges_show_what_the_player_knows() {
        assert!(badges(&StreamFormat::default()).is_empty());
        let f = StreamFormat {
            width: 1920,
            height: 1080,
            fps: 50.0,
            channels: 2,
        };
        assert_eq!(badges(&f), ["FHD", "50 fps", "Stereo"]);
        let audio_only = StreamFormat {
            channels: 6,
            ..StreamFormat::default()
        };
        assert_eq!(badges(&audio_only), ["5.1"]);
    }

    #[test]
    fn minutes_left_while_airing() {
        let english = i18n::resolve("en", None);
        let left = |start, end, now| minutes_left(english, start, end, now);
        assert_eq!(left(0, 30, 5).as_deref(), Some("25 min left"));
        assert_eq!(left(0, 30, 29).as_deref(), Some("1 min left"));
        assert_eq!(left(0, 30, 30), None);
        assert_eq!(left(10, 30, 5), None);
        // In the interface language, with the number where it puts it.
        let of = |tag| minutes_left(i18n::resolve(tag, None), 0, 30, 5).unwrap();
        assert_eq!(of("de"), "noch 25 Min.");
        assert_eq!(of("ja"), "残り25分");
    }

    #[test]
    fn date_and_time_follow_the_style() {
        use crate::clock::HourCycle;
        assert_eq!(
            date_time(EPOCH, 0, Style::english_24()),
            "Tue 10 Nov  18:00"
        );
        let german = Style::new(i18n::resolve("de", None), HourCycle::H24);
        assert_eq!(date_time(EPOCH, 0, german), "Di., 10. Nov.  18:00");
    }
}
