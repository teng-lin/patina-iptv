//! When to download a playlist's guide again.
//!
//! Sans-IO and clock-free: the caller passes what the store knows
//! ([`GuideState`]), the current EPG configuration and `now`, and gets a
//! [`Decision`]. At start, [`plan_startup`] says whether to show the stored
//! guide right away (almost always) and whether to refresh it in the
//! background.
//!
//! Rules, in order:
//! 1. No EPG source configured: [`Decision::Discard`] a stored guide.
//! 2. [`Trigger::Manual`] always refreshes.
//! 3. A recent failed attempt waits ([`Decision::Backoff`]) for
//!    [`RefreshPolicy::retry_after_failure`], whatever the reason below.
//! 4. Nothing stored yet ([`RefreshReason::NeverFetched`]) or the feeds'
//!    settings changed ([`RefreshReason::FeedsChanged`]): refresh. This is
//!    not an "update", so it happens even with the interval at 0.
//! 5. With `on_playlist_change` (always, from the settings): a
//!    [`Trigger::PlaylistChanged`], or stored
//!    bindings matched from other channels or overrides
//!    ([`RefreshReason::ChannelsChanged`]; matching needs the XMLTV channel
//!    list, which is not stored).
//! 6. With an interval: older than it ([`RefreshReason::Stale`]), or the
//!    stored programs end within [`RefreshPolicy::min_ahead`]
//!    ([`RefreshReason::RunningOut`], at most once per
//!    `retry_after_failure`). At [`Trigger::Startup`] only with `on_start`.

use iptv_epg::{Override, Overrides, StoreOptions, UnixTime};
use iptv_providers::model::Channel;
use iptv_settings::{Settings, Target, keys};

use crate::epg::EpgSourceConfig;

const HOUR: i64 = 3600;

/// Refresh settings (`epg.update`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefreshPolicy {
    /// Download again after this many seconds; `None` updates only on
    /// request (`epg.update` = `on_request`).
    pub interval: Option<i64>,
    /// Allow a stale-guide refresh at app start (always, with an interval).
    pub on_start: bool,
    /// Refresh after the playlist changed (always, from the settings).
    pub on_playlist_change: bool,
    /// After a failed attempt, automatic triggers wait this long (seconds).
    pub retry_after_failure: i64,
    /// Refresh early when the stored programs end sooner than `now` plus
    /// this (seconds; only when an interval is set).
    pub min_ahead: i64,
}

impl Default for RefreshPolicy {
    /// The settings' defaults: every 24 h, on start and on playlist change.
    fn default() -> Self {
        Self {
            interval: Some(24 * HOUR),
            on_start: true,
            on_playlist_change: true,
            retry_after_failure: HOUR,
            min_ahead: 12 * HOUR,
        }
    }
}

impl RefreshPolicy {
    /// From the `epg.update` choice at `at` (usually the playlist). A
    /// guide always updates after its playlist changed, and a stale one at
    /// startup (#282 retired the switches for both).
    pub fn from_settings(settings: &Settings, at: &Target) -> Self {
        Self {
            interval: update_interval(settings.choice(keys::EPG_UPDATE, at)),
            on_start: true,
            on_playlist_change: true,
            ..Self::default()
        }
    }
}

/// The period of an `epg.update` choice, in seconds; `None` for
/// `on_request` (and, defensively, anything unknown keeps the default).
pub fn update_interval(choice: &str) -> Option<i64> {
    let hours = match choice {
        "on_request" => return None,
        "every_6_hours" => 6,
        "every_12_hours" => 12,
        "every_2_days" => 48,
        "weekly" => 168,
        _ => 24,
    };
    Some(hours * HOUR)
}

/// The most days of finished programs a guide keeps: the old
/// `epg.keep_past` setting's ceiling.
pub const KEEP_PAST_MAX_DAYS: u32 = 14;
/// The fewest: a day of finished programs stays for search's past
/// listings and the guide's look back, also without any catch-up.
pub const KEEP_PAST_MIN_DAYS: u32 = 1;

/// Days of finished programs to keep when the longest catch-up archive of
/// the stored channels is `longest_archive_days` (0 without catch-up):
/// that archive, within [`KEEP_PAST_MIN_DAYS`]..=[`KEEP_PAST_MAX_DAYS`].
/// Replaces the `epg.keep_past` setting (#282).
pub fn keep_past_days(longest_archive_days: u32) -> u32 {
    longest_archive_days.clamp(KEEP_PAST_MIN_DAYS, KEEP_PAST_MAX_DAYS)
}

/// Storage options of a guide download. Descriptions are kept wherever
/// the guide is stored on disk; browser builds hold it in memory and a
/// small IndexedDB quota, where they would take more than half of it
/// (#282 retired `epg.store_descriptions`). The download keeps up to
/// [`KEEP_PAST_MAX_DAYS`] of finished programs; the store then prunes to
/// [`keep_past_days`]. Fixed, so the stored feed settings stay equal and a
/// new catch-up archive does not count as changed feeds.
pub fn store_options() -> StoreOptions {
    StoreOptions {
        store_descriptions: !cfg!(target_family = "wasm"),
        past_days_to_keep: Some(KEEP_PAST_MAX_DAYS),
    }
}

/// What the store knows about one playlist's guide.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuideState {
    /// Last successful replace.
    pub fetched_at: Option<UnixTime>,
    /// Last attempt, successful or not.
    pub attempted_at: Option<UnixTime>,
    pub last_attempt_failed: bool,
    /// The settings the stored feeds were fetched with.
    pub feeds: Vec<EpgSourceConfig>,
    /// [`matching_fingerprint`] of the stored bindings.
    pub match_fingerprint: Option<u64>,
    /// Latest stored program end.
    pub ends_at: Option<UnixTime>,
    pub bound_channels: usize,
}

impl GuideState {
    fn has_guide(&self) -> bool {
        self.fetched_at.is_some()
    }
}

/// What asked for a decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Startup,
    /// A playlist was added, edited or refreshed.
    PlaylistChanged,
    /// A timer set from the `next_check` of [`Decision::Keep`] fired.
    Timer,
    /// "Update EPG now".
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshReason {
    Requested,
    NeverFetched,
    FeedsChanged,
    ChannelsChanged,
    PlaylistChanged,
    Stale { age: i64 },
    RunningOut { ends_at: UnixTime },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Up to date. Ask again with [`Trigger::Timer`] at `next_check`
    /// (`None`: only on request).
    Keep { next_check: Option<UnixTime> },
    /// Download every feed and replace the stored guide.
    Refresh(RefreshReason),
    /// The last attempt failed recently; ask again at `retry_at`.
    Backoff { retry_at: UnixTime },
    /// No EPG source is configured any more: delete the stored guide.
    Discard,
}

/// Decides whether to refresh. `configs`: the playlist's EPG sources now
/// (from the catalog or, without a playlist download, the stored feeds);
/// `fingerprint`: [`matching_fingerprint`] of the current channels.
pub fn decide(
    policy: &RefreshPolicy,
    trigger: Trigger,
    configs: &[EpgSourceConfig],
    fingerprint: u64,
    state: Option<&GuideState>,
    now: UnixTime,
) -> Decision {
    let empty = GuideState::default();
    let s = state.unwrap_or(&empty);
    if configs.is_empty() {
        return if s.has_guide() || state.is_some() {
            Decision::Discard
        } else {
            Decision::Keep { next_check: None }
        };
    }
    if trigger == Trigger::Manual {
        return Decision::Refresh(RefreshReason::Requested);
    }
    if s.last_attempt_failed
        && let Some(at) = s.attempted_at
        && now < at + policy.retry_after_failure
    {
        return Decision::Backoff {
            retry_at: at + policy.retry_after_failure,
        };
    }
    let Some(fetched_at) = s.fetched_at else {
        return Decision::Refresh(RefreshReason::NeverFetched);
    };
    if !same_feeds(&s.feeds, configs) {
        return Decision::Refresh(RefreshReason::FeedsChanged);
    }
    if policy.on_playlist_change {
        if trigger == Trigger::PlaylistChanged {
            return Decision::Refresh(RefreshReason::PlaylistChanged);
        }
        if s.match_fingerprint != Some(fingerprint) {
            return Decision::Refresh(RefreshReason::ChannelsChanged);
        }
    }
    let Some(interval) = policy.interval else {
        return Decision::Keep { next_check: None };
    };
    let stale_at = fetched_at + interval;
    // Running out early refreshes at most once per retry period.
    let early_at = s
        .ends_at
        .map(|e| (e - policy.min_ahead).max(fetched_at + policy.retry_after_failure));
    let due = early_at.map_or(stale_at, |e| e.min(stale_at));
    if trigger == Trigger::Startup && !policy.on_start {
        // Not at start: the next timer does it.
        return Decision::Keep {
            next_check: Some(if due > now { due } else { now + interval }),
        };
    }
    if now >= stale_at {
        return Decision::Refresh(RefreshReason::Stale {
            age: now - fetched_at,
        });
    }
    if let (Some(ends_at), Some(early)) = (s.ends_at, early_at)
        && now >= early
    {
        return Decision::Refresh(RefreshReason::RunningOut { ends_at });
    }
    Decision::Keep {
        next_check: Some(due),
    }
}

/// What to do at app start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupPlan {
    /// Load the stored guide now (before any network).
    pub use_stored: bool,
    /// Then, in the background: refresh, discard, or nothing.
    pub decision: Decision,
}

/// [`decide`] for [`Trigger::Startup`], plus whether the stored guide is
/// worth showing meanwhile (any stored guide is: a refresh replaces it).
pub fn plan_startup(
    policy: &RefreshPolicy,
    configs: &[EpgSourceConfig],
    fingerprint: u64,
    state: Option<&GuideState>,
    now: UnixTime,
) -> StartupPlan {
    let decision = decide(policy, Trigger::Startup, configs, fingerprint, state, now);
    StartupPlan {
        use_stored: decision != Decision::Discard
            && state.is_some_and(|s| s.has_guide() && s.bound_channels > 0),
        decision,
    }
}

/// The same feeds with the same settings, in any order.
fn same_feeds(stored: &[EpgSourceConfig], now: &[EpgSourceConfig]) -> bool {
    stored.len() == now.len()
        && now
            .iter()
            .all(|c| stored.iter().filter(|s| s.id == c.id).any(|s| s == c))
}

/// A hash of what EPG matching reads from the playlist: each channel's key,
/// tvg-id, name and tvg-name, plus the manual overrides. Independent of
/// channel order. Deterministic across builds and platforms (stored).
pub fn matching_fingerprint(channels: &[Channel], overrides: &Overrides) -> u64 {
    let mut sum = 0u64;
    for c in channels {
        let mut h = Fnv::new(0);
        h.text(Some(c.id.as_str()));
        h.text(c.epg_id.as_deref());
        h.text(Some(&c.name));
        h.text(c.epg_name.as_deref());
        sum = sum.wrapping_add(mix(h.0));
    }
    for (key, o) in overrides {
        let mut h = Fnv::new(1);
        h.text(Some(key.as_str()));
        match o {
            Override::Epg { source, epg_id } => {
                h.bytes(&source.map_or(u64::MAX, |s| u64::from(s.0)).to_le_bytes());
                h.text(Some(epg_id));
            }
            Override::NoEpg => h.bytes(&[2]),
        }
        sum = sum.wrapping_add(mix(h.0));
    }
    mix(sum ^ channels.len() as u64)
}

/// FNV-1a over explicitly framed fields: deterministic across Rust
/// releases and platforms (std's hashers are not; this value is stored).
struct Fnv(u64);

impl Fnv {
    fn new(tag: u8) -> Self {
        let mut h = Self(0xcbf2_9ce4_8422_2325);
        h.bytes(&[tag]);
        h
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    /// Presence, length, bytes.
    fn text(&mut self, s: Option<&str>) {
        match s {
            None => self.bytes(&[0]),
            Some(s) => {
                self.bytes(&[1]);
                self.bytes(&(s.len() as u64).to_le_bytes());
                self.bytes(s.as_bytes());
            }
        }
    }
}

/// splitmix64 finalizer: spreads FNV's output before summing.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
#[path = "refresh_tests.rs"]
mod tests;
