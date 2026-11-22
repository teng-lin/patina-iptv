//! Display rows for the recent channel history.

/// The recently watched channels as the player's History lists them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    /// Catalog channels, newest first.
    pub channels: Vec<usize>,
    /// The row of the playing channel, marked (and focused when it opens).
    pub playing: Option<usize>,
}

/// Build the history display from the ordered channels and the playing channel.
pub fn history(channels: Vec<usize>, playing: Option<usize>) -> History {
    let playing = playing.and_then(|playing| channels.iter().position(|&c| c == playing));
    History { channels, playing }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_core::favorites::{KeyIndex, Recent};
    use iptv_core::guide::SyntheticSchedule;

    fn key(channel: usize) -> String {
        format!("synthetic:{channel}")
    }

    #[test]
    fn history_is_the_recent_list_newest_first_with_the_playing_one_marked() {
        // As stored (newest first), then two channels played this session.
        let mut r = Recent::from_keys([key(9), key(4), "gone".to_owned(), key(2)]);
        r.played(&key(2));
        r.played(&key(30));
        let index = KeyIndex::build(&SyntheticSchedule);
        let h = history(r.channels(&index), Some(30));
        assert_eq!(
            h.channels,
            [30, 2, 9, 4],
            "newest first, once, known keys only"
        );
        assert_eq!(h.playing, Some(0));
        // Replaying an older one (catch-up does not record) keeps the order.
        assert_eq!(history(r.channels(&index), Some(9)).playing, Some(2));
        assert_eq!(history(r.channels(&index), Some(500)).playing, None);
        assert_eq!(
            history(r.channels(&index), None).channels,
            r.channels(&index)
        );
    }
}
