//! A playlist's guide sources: the provider's own guides (playlist header,
//! Xtream `xmltv.php`) and any the user added, in the user's order, each
//! switched on or off and with its time shift (`crate::
//! GuideSource`, schema v6). Pure: what changes when the user edits the
//! list or a playlist import lists the provider's guides, and the configs a
//! guide download takes from it.
//!
//! Feed ids are stable per playlist. Existing identities never change; new
//! provider ids start at 2 and new user ids prefer 1 for a fresh list or
//! otherwise start above 1000. Retained histories reserve their ids through a
//! durable high-water supplied by the store. Priorities follow the list
//! (first wins) and are the smallest that keep that order above each
//! source's natural one (the user's 1000, the provider's in its own order),
//! so a guide arranged as a v5 build had it is not downloaded again.

use crate::GuideSource;
use iptv_epg::FeedId;
use iptv_providers::SecretUrl;
use std::collections::HashSet;

/// The feed of the first guide the user adds.
pub const FIRST_USER_FEED: FeedId = FeedId(1);
/// The first fresh provider guide id.
const PROVIDER_FEEDS: u32 = 2;
/// Maximum provider guides planned for one playlist import.
pub const MAX_PROVIDER_GUIDES: usize = 999;
/// Later guides the user adds: above this.
const MORE_USER_FEEDS: u32 = 1000;
/// The natural priority of a guide the user added (above the provider's).
pub const USER_PRIORITY: i32 = 1000;
/// Time shifts the user can choose, in minutes either way.
pub const MAX_SHIFT_MINUTES: i32 = 12 * 60;

/// A guide source the user adds.
pub fn user_source(feed: FeedId, url: SecretUrl, name: Option<String>) -> GuideSource {
    GuideSource {
        feed,
        provider_index: None,
        url,
        name: name.filter(|n| !n.trim().is_empty()),
        enabled: true,
        shift_minutes: 0,
    }
}

/// An invalid guide identity set cannot be published or edited safely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuideFeedError {
    DuplicateFeed(FeedId),
    DuplicateProviderIndex(u32),
    Exhausted,
    TooManyProviderGuides { count: usize },
}
impl std::fmt::Display for GuideFeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateFeed(feed) => write!(f, "duplicate guide feed id {}", feed.0),
            Self::DuplicateProviderIndex(index) => {
                write!(f, "duplicate provider guide index {index}")
            }
            Self::Exhausted => f.write_str("guide feed identity space exhausted"),
            Self::TooManyProviderGuides { count } => write!(
                f,
                "{count} provider guides exceed the limit of {MAX_PROVIDER_GUIDES}"
            ),
        }
    }
}
impl std::error::Error for GuideFeedError {}

/// Validate identities without renumbering existing feeds or mappings.
pub fn validate_sources(sources: &[GuideSource]) -> Result<(), GuideFeedError> {
    let mut feeds = HashSet::with_capacity(sources.len());
    let mut providers = HashSet::new();
    for source in sources {
        if !feeds.insert(source.feed) {
            return Err(GuideFeedError::DuplicateFeed(source.feed));
        }
        if let Some(index) = source.provider_index
            && !providers.insert(index)
        {
            return Err(GuideFeedError::DuplicateProviderIndex(index));
        }
    }
    Ok(())
}

fn high_water(sources: &[GuideSource], reserved_through: u32) -> Result<u32, GuideFeedError> {
    validate_sources(sources)?;
    Ok(sources
        .iter()
        .map(|source| source.feed.0)
        .max()
        .unwrap_or(0)
        .max(reserved_through))
}

/// A new user feed based on the currently retained rows only.
/// Persistent callers must use [`next_user_feed_with_high_water`].
pub fn next_user_feed(sources: &[GuideSource]) -> Result<FeedId, GuideFeedError> {
    next_user_feed_with_high_water(sources, 0)
}

/// Allocate above all current and durably reserved identities.
pub fn next_user_feed_with_high_water(
    sources: &[GuideSource],
    reserved_through: u32,
) -> Result<FeedId, GuideFeedError> {
    let highest = high_water(sources, reserved_through)?;
    if sources.is_empty() && highest == 0 {
        return Ok(FIRST_USER_FEED);
    }
    highest
        .max(MORE_USER_FEEDS)
        .checked_add(1)
        .map(FeedId)
        .ok_or(GuideFeedError::Exhausted)
}

/// Synchronize provider guides using current rows as the reservation floor.
/// Persistent callers must use [`sync_provider_with_high_water`].
pub fn sync_provider(
    sources: &[GuideSource],
    urls: &[SecretUrl],
) -> Result<Vec<GuideSource>, GuideFeedError> {
    sync_provider_with_high_water(sources, urls, 0)
}

/// Keep every retained guide's identity, position, switch, shift and name.
/// Allocate new provider guides above current and historical reservations.
/// Removed provider indices are omitted without reusing their identities.
pub fn sync_provider_with_high_water(
    sources: &[GuideSource],
    urls: &[SecretUrl],
    reserved_through: u32,
) -> Result<Vec<GuideSource>, GuideFeedError> {
    let mut highest = high_water(sources, reserved_through)?;
    if urls.len() > MAX_PROVIDER_GUIDES {
        return Err(GuideFeedError::TooManyProviderGuides { count: urls.len() });
    }
    let mut out: Vec<GuideSource> = sources
        .iter()
        .filter_map(|source| match source.provider_index {
            None => Some(source.clone()),
            Some(index) => urls.get(index as usize).map(|url| GuideSource {
                url: url.clone(),
                ..source.clone()
            }),
        })
        .collect();
    for (index, url) in urls.iter().enumerate() {
        let index = index as u32;
        if !out
            .iter()
            .any(|source| source.provider_index == Some(index))
        {
            highest = highest
                .checked_add(1)
                .ok_or(GuideFeedError::Exhausted)?
                .max(PROVIDER_FEEDS);
            out.push(GuideSource {
                feed: FeedId(highest),
                provider_index: Some(index),
                url: url.clone(),
                name: None,
                enabled: true,
                shift_minutes: 0,
            });
        }
    }
    Ok(out)
}

/// An edit of the list on the guide sources screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Add {
        url: SecretUrl,
        name: Option<String>,
    },
    /// Only a guide the user added; the provider's are switched off.
    Remove(FeedId),
    SetEnabled(FeedId, bool),
    /// One place up (`-1`) or down (`1`).
    Move(FeedId, i32),
    Shift(FeedId, i32),
    Rename(FeedId, Option<String>),
    /// A new address for a guide the user added.
    Relink(FeedId, SecretUrl),
}

/// `sources` after `edit`; `None` when it changes nothing.
pub fn apply(
    sources: &[GuideSource],
    edit: &Edit,
) -> Result<Option<Vec<GuideSource>>, GuideFeedError> {
    apply_with_high_water(sources, edit, 0)
}

/// Apply an edit without allocating any durably reserved identity.
pub fn apply_with_high_water(
    sources: &[GuideSource],
    edit: &Edit,
    reserved_through: u32,
) -> Result<Option<Vec<GuideSource>>, GuideFeedError> {
    validate_sources(sources)?;
    let new_feed = if matches!(edit, Edit::Add { .. }) {
        Some(next_user_feed_with_high_water(sources, reserved_through)?)
    } else {
        None
    };
    let changed = (|| {
        let mut out = sources.to_vec();
        let at = |id: FeedId| out.iter().position(|s| s.feed == id);
        match edit {
            Edit::Add { url, name } => {
                let feed = new_feed?;
                out.push(user_source(feed, url.clone(), name.clone()));
            }
            Edit::Remove(id) => {
                let i = at(*id).filter(|&i| !out[i].is_provider())?;
                out.remove(i);
            }
            Edit::SetEnabled(id, on) => {
                let i = at(*id).filter(|&i| out[i].enabled != *on)?;
                out[i].enabled = *on;
            }
            Edit::Move(id, step) => {
                let i = at(*id)?;
                let to = usize::try_from(i as i64 + i64::from(*step))
                    .ok()
                    .filter(|&to| to < out.len() && to != i)?;
                let s = out.remove(i);
                out.insert(to, s);
            }
            Edit::Shift(id, minutes) => {
                let i = at(*id)?;
                let m = (*minutes).clamp(-MAX_SHIFT_MINUTES, MAX_SHIFT_MINUTES);
                (out[i].shift_minutes != m).then_some(())?;
                out[i].shift_minutes = m;
            }
            Edit::Rename(id, name) => {
                let i = at(*id)?;
                let name = name.clone().filter(|n| !n.trim().is_empty());
                (out[i].name != name).then_some(())?;
                out[i].name = name;
            }
            Edit::Relink(id, url) => {
                let i = at(*id).filter(|&i| !out[i].is_provider())?;
                (out[i].url != *url).then_some(())?;
                out[i].url = url.clone();
            }
        }
        Some(out)
    })();
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> SecretUrl {
        SecretUrl::new(format!("http://{s}.example.test/guide.xml"))
    }

    fn feeds(sources: &[GuideSource]) -> Vec<u32> {
        sources.iter().map(|s| s.feed.0).collect()
    }

    #[test]
    fn the_providers_guides_follow_each_import() {
        let typed = user_source(FIRST_USER_FEED, url("typed"), None);
        let first = sync_provider(std::slice::from_ref(&typed), &[url("a"), url("b")]).unwrap();
        assert_eq!(feeds(&first), [1, 2, 3]);
        assert_eq!(first[1].provider_index, Some(0));
        // The user's arrangement stays; a changed address is taken over.
        let mut arranged = first.clone();
        arranged.swap(0, 2);
        arranged[0].enabled = false;
        arranged[0].shift_minutes = 60;
        let again = sync_provider(&arranged, &[url("a2"), url("b")]).unwrap();
        assert_eq!(feeds(&again), [3, 2, 1]);
        assert_eq!((again[0].enabled, again[0].shift_minutes), (false, 60));
        assert_eq!(again[1].url, url("a2"));
        // One the provider dropped goes; the user's stay.
        let fewer = sync_provider(&again, &[url("a2")]).unwrap();
        assert_eq!(feeds(&fewer), [2, 1]);
        assert_eq!(sync_provider(&fewer, &[]).unwrap(), [typed]);
    }

    #[test]
    fn feeds_the_user_adds_never_clash_with_the_providers() {
        assert_eq!(next_user_feed(&[]).unwrap(), FIRST_USER_FEED);
        let mut s = sync_provider(&[], &[url("a"), url("b")]).unwrap();
        assert_eq!(next_user_feed(&s).unwrap(), FeedId(1001));
        s = apply(
            &s,
            &Edit::Add {
                url: url("u1"),
                name: None,
            },
        )
        .unwrap()
        .unwrap();
        s = apply(
            &s,
            &Edit::Add {
                url: url("u2"),
                name: Some(" ".into()),
            },
        )
        .unwrap()
        .unwrap();
        s = apply(
            &s,
            &Edit::Add {
                url: url("u3"),
                name: Some("Mine".into()),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(feeds(&s), [2, 3, 1001, 1002, 1003]);
        assert_eq!(
            (s[3].name.as_deref(), s[4].name.as_deref()),
            (None, Some("Mine"))
        );
    }

    #[test]
    fn edits_change_only_what_they_name() {
        let s =
            sync_provider(&[user_source(FIRST_USER_FEED, url("u"), None)], &[url("p")]).unwrap();
        let provider = FeedId(2);
        assert_eq!(
            apply(&s, &Edit::Remove(provider)).unwrap(),
            None,
            "switched off instead"
        );
        assert_eq!(apply(&s, &Edit::Relink(provider, url("x"))).unwrap(), None);
        assert_eq!(
            apply(&s, &Edit::Move(FIRST_USER_FEED, -1)).unwrap(),
            None,
            "already first"
        );
        assert_eq!(
            apply(&s, &Edit::Move(provider, 1)).unwrap(),
            None,
            "already last"
        );
        assert_eq!(apply(&s, &Edit::SetEnabled(provider, true)).unwrap(), None);
        assert_eq!(apply(&s, &Edit::Shift(provider, 0)).unwrap(), None);
        let far = apply(&s, &Edit::Shift(provider, 100 * 60))
            .unwrap()
            .unwrap();
        assert_eq!(far[1].shift_minutes, MAX_SHIFT_MINUTES);
        let removed = apply(&s, &Edit::Remove(FIRST_USER_FEED)).unwrap().unwrap();
        assert_eq!(feeds(&removed), [2]);
        let renamed = apply(&s, &Edit::Rename(provider, Some("Provider".into())))
            .unwrap()
            .unwrap();
        assert_eq!(renamed[1].name.as_deref(), Some("Provider"));
        assert_eq!(
            apply(&renamed, &Edit::Rename(provider, Some("".into())))
                .unwrap()
                .unwrap()[1]
                .name,
            None
        );
    }

    #[test]
    fn reordered_and_relinked_guides_keep_their_identity_and_preferences_after_refresh() {
        let s = sync_provider(
            &[user_source(
                FIRST_USER_FEED,
                url("old-user"),
                Some("Local listings".into()),
            )],
            &[url("provider")],
        )
        .expect("initial guides");
        let provider = s[1].feed;
        let shuffled = apply(&s, &Edit::Move(provider, -1))
            .expect("valid identities")
            .expect("moved provider");
        assert_eq!(feeds(&shuffled), [provider.0, FIRST_USER_FEED.0]);
        let disabled = apply(&shuffled, &Edit::SetEnabled(provider, false))
            .expect("valid identities")
            .expect("disabled provider");
        assert!(!disabled[0].enabled);
        let relink = Edit::Relink(FIRST_USER_FEED, url("new-user"));
        let relinked = apply(&disabled, &relink)
            .expect("valid identities")
            .expect("changed user address");
        assert_eq!(relinked[1].url, url("new-user"));
        assert_eq!(relinked[1].name.as_deref(), Some("Local listings"));
        assert_eq!(
            apply(&relinked, &relink),
            Ok(None),
            "repeating the edit is a no-op"
        );

        let refreshed = sync_provider(&relinked, &[url("new-provider")]).expect("provider refresh");
        assert_eq!(
            refreshed,
            [
                GuideSource {
                    url: url("new-provider"),
                    ..relinked[0].clone()
                },
                relinked[1].clone(),
            ]
        );
    }

    #[test]
    fn invalid_guide_identities_report_the_reason_and_offending_value() {
        for (error, diagnostic) in [
            (
                GuideFeedError::DuplicateFeed(FeedId(7)),
                "duplicate guide feed id 7",
            ),
            (
                GuideFeedError::DuplicateProviderIndex(8),
                "duplicate provider guide index 8",
            ),
            (
                GuideFeedError::Exhausted,
                "guide feed identity space exhausted",
            ),
            (
                GuideFeedError::TooManyProviderGuides {
                    count: MAX_PROVIDER_GUIDES + 1,
                },
                "1000 provider guides exceed the limit of 999",
            ),
        ] {
            assert_eq!(error.to_string(), diagnostic);
        }
    }

    #[test]
    fn provider_growth_preserves_custom_ids_and_rejects_surplus_without_mutation() {
        let current = vec![
            user_source(FeedId(1), url("first"), None),
            user_source(FeedId(1001), url("second"), None),
            user_source(FeedId(2), url("repaired"), Some("Generated repair".into())),
        ];
        let urls: Vec<_> = (0..1000)
            .map(|index| url(&format!("provider-{index}")))
            .collect();
        assert!(matches!(
            sync_provider(&current, &urls),
            Err(GuideFeedError::TooManyProviderGuides { count: 1000 })
        ));
        let synced = sync_provider(&current, &urls[..MAX_PROVIDER_GUIDES]).unwrap();
        assert_eq!(&synced[..current.len()], current.as_slice());
        assert_eq!(synced.len(), current.len() + MAX_PROVIDER_GUIDES);
        let unique: HashSet<_> = synced.iter().map(|source| source.feed).collect();
        assert_eq!(unique.len(), synced.len());
        assert_eq!(synced[current.len()].feed, FeedId(1002));
        assert!(
            synced[current.len()..]
                .iter()
                .all(|source| source.feed.0 > 1001)
        );
    }

    #[test]
    fn duplicate_or_exhausted_identity_sets_return_errors_without_renumbering() {
        let original = user_source(FeedId(7), url("original"), None);
        assert_eq!(
            sync_provider(&[original.clone(), original.clone()], &[]),
            Err(GuideFeedError::DuplicateFeed(FeedId(7)))
        );
        let mut provider_a = original;
        provider_a.provider_index = Some(0);
        let mut provider_b = provider_a.clone();
        provider_b.feed = FeedId(8);
        assert_eq!(
            sync_provider(&[provider_a.clone(), provider_b], &[url("provider")]),
            Err(GuideFeedError::DuplicateProviderIndex(0))
        );
        assert_eq!(
            sync_provider_with_high_water(&[], &[url("new")], u32::MAX),
            Err(GuideFeedError::Exhausted)
        );
        assert_eq!(
            next_user_feed_with_high_water(&[], u32::MAX),
            Err(GuideFeedError::Exhausted)
        );
        let edit = Edit::Add {
            url: url("new"),
            name: None,
        };
        assert_eq!(
            apply_with_high_water(&[provider_a.clone()], &edit, u32::MAX),
            Err(GuideFeedError::Exhausted)
        );
        // Exhaustion prevents new allocation, not updates of an existing identity.
        let existing =
            sync_provider_with_high_water(&[provider_a], &[url("changed")], u32::MAX).unwrap();
        assert_eq!(existing[0].feed, FeedId(7));
        assert_eq!(existing[0].url, url("changed"));
    }

    #[test]
    fn a_durable_reservation_floor_is_shared_by_provider_and_user_allocation() {
        let provider = sync_provider_with_high_water(&[], &[url("provider")], 5000).unwrap();
        assert_eq!(provider[0].feed, FeedId(5001));
        assert_eq!(next_user_feed_with_high_water(&[], 5000), Ok(FeedId(5001)));
        let edited = apply_with_high_water(
            &provider,
            &Edit::Add {
                url: url("user"),
                name: None,
            },
            9000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(edited[0], provider[0]);
        assert_eq!(edited[1].feed, FeedId(9001));
    }
}
