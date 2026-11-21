//! What the web host keeps between launches, per playlist: the user's lists
//! (channel keys) and the stored guide with what the refresh policy reads
//! about it (`iptv_import::GuideState`). The SQLite store keeps the same
//! things on the native hosts.
//!
//! Keys name the playlist by a hash of its identity, never its
//! credentials. Values are JSON, except the guide (`iptv_epg`'s snapshot).
//!
//! Old data does not accumulate: each refresh overwrites the one stored
//! guide, which holds only bound channels, no descriptions and programs
//! that ended at most a day ago (`app::GUIDE_STORE`, applied again on
//! load); replacing the playlist removes the previous one's data.

use iptv_core::favorites::{Favorites, Recent};
use iptv_epg::{FeedId, StoreOptions, TimePolicy};
use iptv_import::{EpgSourceConfig, GuideState};
use iptv_providers::SecretUrl;
use iptv_providers::model::SourceConfig;
use serde_json::{Value, json};

use crate::exec::{storage_get, storage_put};

fn fnv(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// A stable name for a playlist: what identifies the account, hashed
/// (FNV-1a), so the storage keys carry no credentials in clear.
pub(crate) fn source_id(config: &SourceConfig) -> String {
    let identity = match config {
        SourceConfig::Xtream(x) => format!(
            "xtream\n{}\n{}",
            x.server_url
                .trim()
                .trim_end_matches('/')
                .to_ascii_lowercase(),
            x.username.expose().trim()
        ),
        SourceConfig::M3u(m) => format!("m3u\n{}", m.location.expose().trim()),
        SourceConfig::Stalker(_) => "stalker".to_owned(),
    };
    fnv(&identity)
}

fn lists_key(id: &str) -> String {
    format!("lists:{id}")
}
fn guide_key(id: &str) -> String {
    format!("guide:{id}")
}
fn guide_state_key(id: &str) -> String {
    format!("guide-state:{id}")
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) async fn load_lists(id: &str) -> (Favorites, Recent) {
    let Some(bytes) = storage_get(&lists_key(id)).await else {
        return Default::default();
    };
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (
        Favorites::from_keys(strings(&v["favorites"])),
        Recent::from_keys(strings(&v["recent"])),
    )
}

pub(crate) fn save_lists(id: &str, favorites: &Favorites, recent: &Recent) {
    let v = json!({ "favorites": favorites.keys(), "recent": recent.keys() });
    storage_put(&lists_key(id), v.to_string().as_bytes());
}

/// A feed as stored: its address only as a hash, since an Xtream guide
/// address carries the account's credentials. [`with_current_urls`] puts
/// the playlist's current address back where the hash still matches.
fn config_json(c: &EpgSourceConfig) -> Value {
    json!({
        "contentSupplied": c.content_supplied,
        "id": c.id.0,
        "urlHash": fnv(c.url.expose()),
        "priority": c.priority,
        "userAgent": c.user_agent,
        "assumedOffset": c.time.assumed_offset_minutes,
        "shift": c.time.shift_minutes,
        "descriptions": c.store.store_descriptions,
        "pastDays": c.store.past_days_to_keep,
    })
}

fn config_from(v: &Value) -> Option<EpgSourceConfig> {
    let int = |k: &str| v.get(k).and_then(Value::as_i64);
    // A placeholder no current address equals until restored.
    let mut c = EpgSourceConfig::new(
        FeedId(u32::try_from(int("id")?).ok()?),
        SecretUrl::new(format!("stored:{}", v.get("urlHash")?.as_str()?)),
    )
    .with_priority(i32::try_from(int("priority")?).ok()?);
    c.content_supplied = v.get("contentSupplied")?.as_bool()?;
    c.user_agent = v
        .get("userAgent")
        .and_then(Value::as_str)
        .map(str::to_owned);
    c.time = TimePolicy {
        assumed_offset_minutes: i32::try_from(int("assumedOffset")?).ok()?,
        shift_minutes: i32::try_from(int("shift")?).ok()?,
    };
    c.store = StoreOptions {
        store_descriptions: v.get("descriptions")?.as_bool()?,
        past_days_to_keep: int("pastDays").and_then(|d| u32::try_from(d).ok()),
    };
    Some(c)
}

fn state_json(s: &GuideState) -> Value {
    json!({
        "fetchedAt": s.fetched_at,
        "attemptedAt": s.attempted_at,
        "failed": s.last_attempt_failed,
        "feeds": s.feeds.iter().map(config_json).collect::<Vec<_>>(),
        // A string: JSON numbers above 2^53 lose precision in JavaScript tools.
        "fingerprint": s.match_fingerprint.map(|f| f.to_string()),
        "endsAt": s.ends_at,
        "bound": s.bound_channels,
    })
}

fn state_from(v: &Value) -> Option<GuideState> {
    Some(GuideState {
        fetched_at: v.get("fetchedAt").and_then(Value::as_i64),
        attempted_at: v.get("attemptedAt").and_then(Value::as_i64),
        last_attempt_failed: v.get("failed")?.as_bool()?,
        feeds: v
            .get("feeds")?
            .as_array()?
            .iter()
            .map(config_from)
            .collect::<Option<Vec<_>>>()?,
        match_fingerprint: v
            .get("fingerprint")
            .and_then(Value::as_str)
            .and_then(|f| f.parse().ok()),
        ends_at: v.get("endsAt").and_then(Value::as_i64),
        bound_channels: v.get("bound").and_then(Value::as_u64).unwrap_or(0) as usize,
    })
}

/// `state` with each stored feed's address replaced by the current feed's
/// when they are the same address; a changed address stays a placeholder,
/// so the refresh policy sees changed feeds.
pub(crate) fn with_current_urls(mut state: GuideState, current: &[EpgSourceConfig]) -> GuideState {
    for feed in &mut state.feeds {
        let Some(hash) = feed.url.expose().strip_prefix("stored:").map(str::to_owned) else {
            continue;
        };
        if let Some(now) = current
            .iter()
            .find(|c| c.id == feed.id && fnv(c.url.expose()) == hash)
        {
            feed.url = now.url.clone();
        }
    }
    state
}

/// What the store knows about the playlist's guide (unreadable: nothing).
pub(crate) async fn load_guide_state(id: &str) -> Option<GuideState> {
    let bytes = storage_get(&guide_state_key(id)).await?;
    state_from(&serde_json::from_slice(&bytes).ok()?)
}

pub(crate) fn save_guide_state(id: &str, state: &GuideState) {
    storage_put(
        &guide_state_key(id),
        state_json(state).to_string().as_bytes(),
    );
}

pub(crate) async fn load_guide(id: &str) -> Option<Vec<u8>> {
    storage_get(&guide_key(id)).await
}

/// Stores the guide, then its state: a state never describes a guide that
/// is not stored (writes keep their order).
pub(crate) fn save_guide(id: &str, snapshot: &[u8], state: &GuideState) {
    storage_put(&guide_key(id), snapshot);
    save_guide_state(id, state);
}

pub(crate) fn discard_guide(id: &str) {
    storage_put(&guide_state_key(id), &[]);
    storage_put(&guide_key(id), &[]);
}

const CURRENT_KEY: &str = "current-source";

/// This host shows one playlist: replacing it removes what was kept for
/// the previous one (its guide and lists), as removing a playlist does on
/// the native hosts. Returns false, changing nothing, when `current` says
/// the import was superseded meanwhile.
pub(crate) async fn make_current(id: &str, current: &dyn Fn() -> bool) -> bool {
    let previous = storage_get(CURRENT_KEY).await;
    if !current() {
        return false;
    }
    match previous.as_deref().map(String::from_utf8_lossy) {
        Some(old) if old == id => return true,
        Some(old) => {
            discard_guide(&old);
            storage_put(&lists_key(&old), &[]);
        }
        None => {}
    }
    storage_put(CURRENT_KEY, id.as_bytes());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_state_round_trips_through_json() {
        let mut feed =
            EpgSourceConfig::new(FeedId(3), SecretUrl::new("http://guide.example.com/x.xml"))
                .with_priority(2);
        feed.content_supplied = true;
        feed.user_agent = Some("Demo".into());
        feed.time = TimePolicy {
            assumed_offset_minutes: 60,
            shift_minutes: -30,
        };
        feed.store = StoreOptions {
            store_descriptions: false,
            past_days_to_keep: Some(1),
        };
        let state = GuideState {
            fetched_at: Some(10),
            attempted_at: Some(20),
            last_attempt_failed: true,
            feeds: vec![feed],
            match_fingerprint: Some(u64::MAX),
            ends_at: Some(30),
            bound_channels: 4,
        };
        let stored = state_from(&state_json(&state)).unwrap();
        assert!(
            !state_json(&state).to_string().contains("guide.example.com"),
            "no address stored"
        );
        assert_ne!(stored, state, "the address is a placeholder until restored");
        assert_eq!(with_current_urls(stored.clone(), &state.feeds), state);
        let mut moved = state.feeds.clone();
        moved[0].url = SecretUrl::new("http://guide.example.com/other.xml");
        assert_ne!(with_current_urls(stored, &moved).feeds[0].url, moved[0].url);
        assert_eq!(state_from(&json!({ "failed": false })), None);
    }

    #[test]
    fn a_source_is_named_without_its_credentials() {
        use iptv_providers::Secret;
        use iptv_providers::model::XtreamCredentials;
        let x = |user: &str, server: &str| {
            SourceConfig::Xtream(XtreamCredentials {
                server_url: server.into(),
                username: Secret::new(user),
                password: Secret::new("secret"),
                user_agent: None,
            })
        };
        let id = source_id(&x("demo", "http://TV.example.com/"));
        assert_eq!(id, source_id(&x("demo", "http://tv.example.com")));
        assert_ne!(id, source_id(&x("other", "http://tv.example.com")));
        assert!(!id.contains("demo") && !id.contains("secret"));
    }
}
