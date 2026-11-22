//! Playback state that does not depend on a player: the resolved stream
//! (URL plus request headers), the player's health with retry/backoff and
//! stall detection ([`Health`]), what a failure means ([`Cause`]), the
//! previous-channel history and zapping within a group.
//!
//! Times are milliseconds on the caller's monotonic clock. Nothing here
//! sleeps: [`Health`] says *when* something is due and the shell sets the
//! timer, then calls back in.

use std::ops::Range;

use crate::favorites::Recent;
use crate::guide::Schedule;
use crate::lists::ChannelList;

mod error;
mod health;
mod states;

pub use error::{Cause, ErrorKind, Failure};
pub use health::Health;
pub use states::{Phase, PlayerState, Retry, RetryPolicy, Waiting};

/// What the player should open: a URL and the HTTP headers to send with it
/// (User-Agent, Referer, provider identity). Header names keep their case;
/// lookups are case-insensitive.
#[derive(Clone, PartialEq, Eq)]
pub struct PlaySource {
    /// Embedded content URLs require public destinations at every request hop.
    /// False trusts only the explicitly configured original origin.
    pub public_only: bool,
    pub url: String,
    pub headers: Vec<(String, String)>,
}

impl std::fmt::Debug for PlaySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<_> = self
            .headers
            .iter()
            .map(|(name, _)| (name, "[redacted]"))
            .collect();
        f.debug_struct("PlaySource")
            .field("public_only", &self.public_only)
            .field("url", &"[redacted]")
            .field("headers", &headers)
            .finish()
    }
}

impl Default for PlaySource {
    fn default() -> Self {
        Self::new("")
    }
}

impl PlaySource {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            public_only: true,
            url: url.into(),
            headers: Vec::new(),
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Sets `name` (replacing any value under the same name, any case).
    /// Blank values are ignored.
    pub fn set_header(&mut self, name: &str, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        match self
            .headers
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
        {
            Some(slot) => slot.1 = value.to_owned(),
            None => self.headers.push((name.to_owned(), value.to_owned())),
        }
    }

    /// Adds `name` only when no value is set yet (a fallback).
    pub fn default_header(&mut self, name: &str, value: &str) {
        if self.header(name).is_none() {
            self.set_header(name, value);
        }
    }

    /// Applies `headers` over the current ones (later wins).
    pub fn with_headers<'a>(
        mut self,
        headers: impl IntoIterator<Item = &'a (String, String)>,
    ) -> Self {
        for (k, v) in headers {
            self.set_header(k, v);
        }
        self
    }
}

/// How many channels the recent-channels list keeps.
pub const RECENT_MAX: usize = crate::favorites::RECENT_LIMIT;

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

/// The channels of the group containing `channel` (channels are stored
/// grouped; the last group runs to the end).
pub fn group_range(schedule: &dyn Schedule, channel: usize) -> Range<usize> {
    let count = schedule.channel_count();
    let members = schedule.channel_group_channels(channel);
    members.iter().min().copied().unwrap_or(0)..members.iter().max().map_or(count, |c| c + 1)
}

/// Channel up/down from `channel` by `step`, wrapping within its group.
pub fn zap_target(schedule: &dyn Schedule, channel: usize, step: isize) -> usize {
    let mut channels = schedule.channel_group_channels(channel);
    channels.retain(|&c| {
        !schedule.channel_hidden(c) && schedule.channel_block(c) == crate::organization::Block::None
    });
    if channels.is_empty() {
        return channel;
    }
    let at = channels.iter().position(|&c| c == channel);
    let to = match at {
        Some(at) => (at as isize + step).rem_euclid(channels.len() as isize) as usize,
        None if step < 0 => channels.len() - 1,
        None => 0,
    };
    channels[to]
}

/// The starting point for a channel change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZapOrigin {
    Row(usize),
    OtherGroup(usize),
}

impl ZapOrigin {
    /// Finds the playing channel in the displayed list, or falls back to
    /// the focused row when nothing is playing.
    pub fn from_channel(
        channel: Option<usize>,
        row_of: impl FnOnce(usize) -> Option<usize>,
        focused_row: impl FnOnce() -> usize,
    ) -> Self {
        match channel {
            Some(channel) => row_of(channel).map_or(Self::OtherGroup(channel), Self::Row),
            None => Self::Row(focused_row()),
        }
    }
}

/// Zaps within the displayed list or within the playing channel's group.
pub fn zap_from_list(
    schedule: &dyn Schedule,
    list: &ChannelList,
    from: ZapOrigin,
    step: isize,
    allowed: impl Fn(usize) -> bool,
) -> Option<usize> {
    match from {
        ZapOrigin::Row(row) => (1..=list.len())
            .filter_map(|offset| list.zap(row, (offset as isize).wrapping_mul(step)))
            .filter_map(|row| list.channel(row))
            .find(|&channel| allowed(channel)),
        ZapOrigin::OtherGroup(channel) => {
            Some(zap_target(schedule, channel, step)).filter(|&c| allowed(c))
        }
    }
}

/// Counts from native position samples while the viewer is watching live TV.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchTime {
    last: Option<(u64, u64)>,
    pending_ms: u64,
}

impl WatchTime {
    /// Adds forward playback bounded by elapsed real time.
    pub fn sample(&mut self, now_ms: u64, position_ms: u64) {
        if let Some((last_now, last_position)) = self.last.replace((now_ms, position_ms)) {
            let elapsed = now_ms.saturating_sub(last_now);
            // A missing heartbeat can span buffering, an inactive host, or a reopen.
            if elapsed <= 3_000 {
                self.pending_ms += elapsed.min(position_ms.saturating_sub(last_position));
            }
        }
    }

    pub fn pending_ms(&self) -> u64 {
        self.pending_ms
    }

    pub fn take_ms(&mut self) -> u64 {
        std::mem::take(&mut self.pending_ms)
    }
}

#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod tests;

/// Admission for callbacks from the native source actually published to a
/// persistent player. Selecting/preparing its successor must not relabel old
/// native progress as belonging to the new content identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceOwnership {
    #[default]
    Local,
    RuntimeProvider,
    NativeProvider,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Publication {
    published: Option<(u64, u64)>,
    ownership: SourceOwnership,
}
impl Publication {
    pub fn publish(&mut self, session: u64, token: u64, ownership: SourceOwnership) {
        self.published = Some((session, token));
        self.ownership = ownership;
    }
    /// Clearing callback admission retains the old transport's ownership until
    /// its replacement publishes. Unowned provider I/O cannot be probed over.
    pub fn may_probe_replacement(&self) -> bool {
        self.ownership != SourceOwnership::NativeProvider
    }
    pub fn clear(&mut self) {
        self.published = None;
    }
    pub fn accepts(&self, session: u64, token: Option<u64>) -> bool {
        self.published
            .is_some_and(|(published_session, published_token)| {
                published_session == session && token.is_none_or(|token| token == published_token)
            })
    }
}

#[cfg(test)]
mod publication_tests {
    use super::{Publication, SourceOwnership};
    #[test]
    fn callbacks_belong_to_published_session_and_token() {
        let mut publication = Publication::default();
        assert!(!publication.accepts(1, None));
        publication.publish(1, 7, SourceOwnership::RuntimeProvider);
        assert!(publication.accepts(1, None));
        assert!(publication.accepts(1, Some(7)));
        assert!(!publication.accepts(2, Some(7)));
        publication.clear();
        assert!(
            !publication.accepts(1, Some(7)),
            "a same-session retry is also fenced during preparation"
        );
        publication.publish(1, 8, SourceOwnership::RuntimeProvider);
        assert!(!publication.accepts(1, Some(7)));
        assert!(publication.accepts(1, Some(8)));
        publication.publish(2, 9, SourceOwnership::NativeProvider);
        publication.clear();
        assert!(!publication.may_probe_replacement());
        publication.publish(3, 10, SourceOwnership::Local);
        assert!(
            publication.may_probe_replacement(),
            "a saved local file does not occupy a provider"
        );
    }
}

#[cfg(test)]
mod diagnostic_regression {
    use super::*;
    #[test]
    fn resolved_source_redacts_url_and_arbitrary_header_values() {
        let mut source = PlaySource::new(
            "http://user-synthetic:password-synthetic@media.example.test/path-secret?key=query-secret",
        );
        source.set_header("X-Arbitrary-Credential", "header-secret");
        let debug = format!("{source:?}");
        for secret in [
            "user-synthetic",
            "password-synthetic",
            "path-secret",
            "query-secret",
            "header-secret",
        ] {
            assert!(!debug.contains(secret), "{debug}");
        }
        assert_eq!(
            source.header("X-Arbitrary-Credential"),
            Some("header-secret")
        );
    }
}
