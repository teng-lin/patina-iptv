//! Typed XMLTV records produced by [`crate::XmltvParser`].

use crate::time::UnixTime;

/// A `<channel>` element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XmltvChannel {
    /// The `id` attribute; programs refer to it.
    pub id: String,
    /// Every non-empty `<display-name>`, in document order.
    pub display_names: Vec<String>,
    /// `src` of the first `<icon>`.
    pub icon: Option<String>,
}

/// A `<programme>` element. Times already include the source's
/// [`crate::TimePolicy`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XmltvProgram {
    /// The `channel` attribute (an EPG channel id).
    pub channel: String,
    pub start: UnixTime,
    /// `None` when absent or unparsable; the store then ends the program
    /// at the next one's start.
    pub stop: Option<UnixTime>,
    /// First non-empty `<title>`.
    pub title: String,
    pub sub_title: Option<String>,
    pub desc: Option<String>,
    pub categories: Vec<String>,
    pub episode_num: Vec<EpisodeNum>,
    /// `src` of the first `<icon>`.
    pub icon: Option<String>,
    /// First `<rating>` with a `<value>`.
    pub rating: Option<Rating>,
    /// The `catchup-id` attribute, used by some catch-up URL schemes.
    pub catchup_id: Option<String>,
}

/// An `<episode-num>` element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EpisodeNum {
    /// `system` attribute, e.g. `xmltv_ns` or `onscreen`.
    pub system: Option<String>,
    pub value: String,
}

impl EpisodeNum {
    /// One-based season and episode, when the value can be read: `xmltv_ns`
    /// (`season.episode.part`, zero-based, `/total` allowed) or an on-screen
    /// form like `S01E02` / `s1 e2`.
    pub fn season_episode(&self) -> Option<(Option<u32>, Option<u32>)> {
        let v = self.value.trim();
        if self.system.as_deref() == Some("xmltv_ns")
            || (v.contains('.') && !v.contains(['S', 's']))
        {
            let mut parts = v.split('.');
            let field = |p: Option<&str>| -> Option<u32> {
                let p = p?.split('/').next()?.trim();
                if p.is_empty() {
                    None
                } else {
                    p.parse::<u32>().ok().and_then(|n| n.checked_add(1))
                }
            };
            let season = field(parts.next());
            let episode = field(parts.next());
            return (season.is_some() || episode.is_some()).then_some((season, episode));
        }
        let lower = v.to_ascii_lowercase();
        let s = lower.find('s')?;
        let e = lower[s..].find('e')? + s;
        let num = |t: &str| -> Option<u32> {
            let d: String = t
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            d.parse().ok()
        };
        let season = num(&lower[s + 1..e]);
        let episode = num(&lower[e + 1..]);
        (season.is_some() || episode.is_some()).then_some((season, episode))
    }
}

/// A `<rating>` element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rating {
    pub system: Option<String>,
    pub value: String,
}

/// One parsed top-level record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XmltvItem {
    Channel(XmltvChannel),
    Program(XmltvProgram),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(system: Option<&str>, value: &str) -> Option<(Option<u32>, Option<u32>)> {
        EpisodeNum {
            system: system.map(str::to_string),
            value: value.to_string(),
        }
        .season_episode()
    }

    #[test]
    fn episode_numbers() {
        assert_eq!(
            EpisodeNum {
                system: Some("xmltv_ns".into()),
                value: "4294967295.0.".into()
            }
            .season_episode(),
            Some((None, Some(1)))
        );
        assert_eq!(
            EpisodeNum {
                system: Some("xmltv_ns".into()),
                value: "4294967294.0.".into()
            }
            .season_episode(),
            Some((Some(u32::MAX), Some(1)))
        );
        assert_eq!(ep(Some("xmltv_ns"), "0.4."), Some((Some(1), Some(5))));
        assert_eq!(
            ep(Some("xmltv_ns"), "2/5.11/20.0/1"),
            Some((Some(3), Some(12)))
        );
        assert_eq!(ep(Some("xmltv_ns"), ".3."), Some((None, Some(4))));
        assert_eq!(ep(Some("onscreen"), "S01E02"), Some((Some(1), Some(2))));
        assert_eq!(ep(None, "s3 e10"), Some((Some(3), Some(10))));
        assert_eq!(ep(Some("onscreen"), "Episode"), None);
        assert_eq!(ep(Some("xmltv_ns"), ".."), None);
    }
}
