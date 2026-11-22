//! The player's sound: volume and mute (the desktop's own controls; TV
//! remotes and tablets use the system volume) and the audio delay chosen
//! over full-screen video.

/// One volume step (keys, the − and + buttons).
pub const VOLUME_STEP: i32 = 5;

/// Volume in percent and mute, as the `volume` and `muted` props say them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    /// 0–100.
    pub level: u8,
    pub muted: bool,
}

impl Default for Volume {
    fn default() -> Self {
        Volume {
            level: 100,
            muted: false,
        }
    }
}

impl Volume {
    /// Up or down by `delta` percent; changing the volume unmutes.
    pub fn step(&mut self, delta: i32) {
        self.set(i32::from(self.level) + delta);
    }

    /// Sets the level (clamped to 0–100); unmutes.
    pub fn set(&mut self, level: i32) {
        self.level = level.clamp(0, 100) as u8;
        self.muted = false;
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
    }

    /// What is heard, 0–100.
    pub fn effective(&self) -> u8 {
        if self.muted { 0 } else { self.level }
    }
}

/// The audio delay choices in milliseconds (positive: the sound comes
/// later than the picture), in 50 ms steps up to a second either way.
pub fn audio_delays() -> Vec<i32> {
    (-20..=20).map(|i| i * 50).collect()
}

/// "+150 ms", "0 ms", "−200 ms", with the unit as `language` writes it.
pub fn delay_label(language: &crate::i18n::Language, ms: i32) -> String {
    let value = match ms {
        0 => "0".to_owned(),
        ms if ms > 0 => format!("+{ms}"),
        ms => format!("−{}", ms.unsigned_abs()),
    };
    crate::i18n::format(language, "{value} ms", &[("value", (&value).into())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_steps_clamp_and_unmute() {
        let mut v = Volume::default();
        v.step(VOLUME_STEP);
        assert_eq!(v.level, 100);
        v.toggle_mute();
        assert_eq!((v.muted, v.effective()), (true, 0));
        v.step(-VOLUME_STEP);
        assert_eq!((v.level, v.muted, v.effective()), (95, false, 95));
        v.set(-20);
        assert_eq!(v.level, 0);
        v.toggle_mute();
        v.toggle_mute();
        assert!(!v.muted);
    }

    #[test]
    fn delays_are_symmetric_around_zero() {
        let d = audio_delays();
        assert_eq!((d.len(), d[0], d[20], d[40]), (41, -1000, 0, 1000));
        let english = crate::i18n::resolve("en", None);
        assert_eq!(delay_label(english, 150), "+150 ms");
        assert_eq!(delay_label(english, 0), "0 ms");
        assert_eq!(delay_label(english, -200), "−200 ms");
        // The unit is the language's own.
        let russian = crate::i18n::resolve("ru", None);
        assert_eq!(delay_label(russian, 150), "+150 мс");
        assert_eq!(
            delay_label(crate::i18n::resolve("ja", None), -200),
            "−200ミリ秒"
        );
    }
}
