//! Converts portable guide choices into import pipeline configurations.
use iptv_epg::TimePolicy;
use iptv_import::EpgSourceConfig;
use iptv_library::GuideSource;
pub use iptv_library::guides::*;

/// The configs of a guide download: sources switched on, first one
/// highest. `template` carries the storage options.
pub fn configs_of(sources: &[GuideSource], template: &EpgSourceConfig) -> Vec<EpgSourceConfig> {
    let providers = sources.iter().filter(|s| s.is_provider()).count() as i32;
    let on: Vec<&GuideSource> = sources
        .iter()
        .filter(|s| s.enabled && !s.url.expose().trim().is_empty())
        .collect();
    let mut priorities = vec![0; on.len()];
    let mut below: Option<i32> = None;
    for (i, s) in on.iter().enumerate().rev() {
        let natural = match s.provider_index {
            None => USER_PRIORITY,
            Some(n) => providers - 1 - n as i32,
        };
        let p = below.map_or(natural, |b| natural.max(b + 1));
        priorities[i] = p;
        below = Some(p);
    }
    on.iter()
        .zip(priorities)
        .map(|(s, priority)| EpgSourceConfig {
            content_supplied: s.is_provider(),
            id: s.feed,
            url: s.url.clone(),
            priority,
            time: TimePolicy {
                shift_minutes: s.shift_minutes,
                ..template.time
            },
            ..template.clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptv_epg::FeedId;
    use iptv_providers::SecretUrl;

    fn url(s: &str) -> SecretUrl {
        SecretUrl::new(format!("http://{s}.example.test/guide.xml"))
    }
    fn template() -> EpgSourceConfig {
        EpgSourceConfig::new(FeedId(0), url("template"))
    }

    #[test]
    fn priorities_follow_the_list_and_keep_a_v5_arrangement() {
        let typed = user_source(FIRST_USER_FEED, url("typed"), None);
        // As a v5 build had an Xtream source with a typed guide.
        let v5 = sync_provider(&[typed], &[url("xtream")]);
        let c = configs_of(&v5, &template());
        assert_eq!(
            c.iter().map(|c| (c.id.0, c.priority)).collect::<Vec<_>>(),
            [(1, 1000), (2, 0)]
        );
        // The provider's guide first: above the user's.
        let moved = apply(&v5, &Edit::Move(FeedId(2), -1)).unwrap();
        let c = configs_of(&moved, &template());
        assert_eq!(
            c.iter().map(|c| (c.id.0, c.priority)).collect::<Vec<_>>(),
            [(2, 1001), (1, 1000)]
        );
        // Switched off: not downloaded; the shift travels with the config.
        let off = apply(&moved, &Edit::SetEnabled(FeedId(1), false)).unwrap();
        let shifted = apply(&off, &Edit::Shift(FeedId(2), -120)).unwrap();
        let c = configs_of(&shifted, &template());
        assert_eq!(c.len(), 1);
        assert_eq!(
            (c[0].id.0, c[0].priority, c[0].time.shift_minutes),
            (2, 0, -120)
        );
    }
}
