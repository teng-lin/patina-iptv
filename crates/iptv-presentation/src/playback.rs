//! The live player's current and previous channel identities.

use iptv_core::favorites::Recent;

/// The channels played live, newest first, by content identity (stable
/// keys): "previous channel" and the player's History list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelHistory {
    recent: Recent,
}

impl ChannelHistory {
    /// `key` started playing: it moves to the front. Replaying the current
    /// channel changes nothing.
    pub fn played(&mut self, key: &str) {
        self.recent.played(key);
    }

    pub fn current(&self) -> Option<&str> {
        self.recent.keys().first().map(String::as_str)
    }

    /// The channel before the current one (pressing it again comes back).
    pub fn previous(&self) -> Option<&str> {
        self.recent.keys().get(1).map(String::as_str)
    }

    /// Every channel played, newest (the current one) first.
    pub fn recent(&self) -> &[String] {
        self.recent.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_core::favorites::RECENT_LIMIT;

    #[test]
    fn previous_channel_toggles() {
        let mut h = ChannelHistory::default();
        assert_eq!(h.previous(), None);
        h.played("a");
        h.played("a");
        assert_eq!(h.previous(), None, "replaying is not a change");
        h.played("b");
        h.played("c");
        assert_eq!((h.current(), h.previous()), (Some("c"), Some("b")));
        let prev = h.previous().unwrap().to_owned();
        h.played(&prev);
        assert_eq!((h.current(), h.previous()), (Some("b"), Some("c")));
    }

    #[test]
    fn recent_channels_are_newest_first_without_repeats_and_bounded() {
        let mut h = ChannelHistory::default();
        for key in ["a", "b", "c", "a"] {
            h.played(key);
        }
        assert_eq!(h.recent(), ["a", "c", "b"]);
        for i in 0..RECENT_LIMIT + 5 {
            h.played(&format!("k{i}"));
        }
        assert_eq!(h.recent().len(), RECENT_LIMIT);
        assert_eq!(h.current(), Some(format!("k{}", RECENT_LIMIT + 4).as_str()));
    }
}
