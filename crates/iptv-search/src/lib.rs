#![forbid(unsafe_code)]
//! Search layer of iptv-rs.
//!
//! - [`tokens`] / [`Query`]: text folding shared with EPG matching (iptv-epg's
//!   [`normalize_name`](iptv_epg::normalize_name), word by word) and the one
//!   matcher every search uses: all query tokens must match, the last one
//!   may be the start of a word; quality is [`MatchQuality`].
//! - [`ChannelIndex`]: channel names (plus extra keys such as numbers),
//!   favourites first on request.
//! - [`ProgrammeIndex`]: an inverted index of programme titles from an
//!   [`iptv_epg::GuideStore`], with time window, catch-up and grouping.
//! - [`SearchHistory`]: recent queries, de-duplicated, saved as a string list.
//! - [`SearchState`]: the search screen (history, typing, Channels/Programs
//!   results, no results) with focus kept by result identity.
//!
//! No NSH, no platform APIs, no I/O and no clock: `now` comes from the
//! caller. Builds for `wasm32-unknown-unknown`.

mod channels;
mod history;
mod programmes;
mod programmes_builder;
mod state;
mod text;

pub use channels::{ChannelHit, ChannelIndex, ChannelSearchOptions};
pub use history::SearchHistory;
pub use programmes::{GroupBy, ProgrammeHit, ProgrammeIndex, ProgrammeSearchOptions};
pub use programmes_builder::ProgrammeIndexBuilder;
pub use state::{Focus, Phase, ResultId, SearchAction, SearchEffect, SearchState, Tab};
pub use text::{MIN_INFIX, MatchQuality, Query, fold, tokens};

/// Source-qualified content identity, independent of catalogue position.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChannelId {
    pub source: Option<i64>,
    pub content: String,
    incarnation: Option<String>,
    anonymous: Option<u64>,
}
impl ChannelId {
    pub fn new(source: Option<i64>, content: impl Into<String>) -> Self {
        Self {
            source,
            content: content.into(),
            incarnation: None,
            anonymous: None,
        }
    }

    /// Bind stored content to the accepting source incarnation. Connection edits
    /// and delete/recreate change it; ordinary catalogue refreshes do not.
    pub fn in_source(
        source: i64,
        incarnation: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            source: Some(source),
            content: content.into(),
            incarnation: Some(incarnation.into()),
            anonymous: None,
        }
    }

    /// Anonymous rows have identity only within the index that created them.
    /// Set an explicit content identity to retain focus across index rebuilds.
    pub fn is_anonymous(&self) -> bool {
        self.anonymous.is_some()
    }

    pub(crate) fn anonymous() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("anonymous search identity space exhausted");
        Self {
            source: None,
            content: String::new(),
            incarnation: None,
            anonymous: Some(id),
        }
    }

    pub fn incarnation(&self) -> Option<&str> {
        self.incarnation.as_deref()
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.content.capacity() + self.incarnation.as_ref().map_or(0, String::capacity)
    }
}
