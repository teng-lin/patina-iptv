//! Progress events for the import screen.

use iptv_epg::FeedId;
use iptv_providers::model::ContentKind;

/// What the importer is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Phase {
    /// Finding the API endpoint (Stalker discovery).
    Connecting,
    /// Stalker handshake/profile/login.
    Authenticating,
    /// Xtream account check.
    Account,
    /// Downloading and parsing an M3U playlist.
    Playlist,
    /// Categories/genres of one content kind.
    Groups(ContentKind),
    /// Channels, movies or series of one content kind.
    Items(ContentKind),
    /// Downloading and parsing one EPG source.
    Guide(FeedId),
    /// Matching playlist channels to EPG channels.
    Matching,
    Done,
}

/// One progress report. Counters are totals for the current phase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    /// Items parsed so far in this phase (channels, programs, groups).
    pub items: u64,
    /// Body bytes received so far in this phase.
    pub bytes: u64,
    /// Wire size when the server declared it.
    pub total_bytes: Option<u64>,
    /// 1-based page for paginated catalogs (Stalker).
    pub page: Option<u32>,
}

impl Progress {
    pub fn phase(phase: Phase) -> Self {
        Self {
            phase,
            items: 0,
            bytes: 0,
            total_bytes: None,
            page: None,
        }
    }
}

/// Receives progress. Implemented for every `FnMut(&Progress)`; reports
/// arrive on the importing task, so a UI should coalesce them per frame.
pub trait ProgressSink {
    fn report(&mut self, progress: &Progress);
}

impl<F: FnMut(&Progress)> ProgressSink for F {
    fn report(&mut self, progress: &Progress) {
        self(progress)
    }
}

/// Discards progress.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn report(&mut self, _: &Progress) {}
}

/// Body bytes between two download reports.
pub(crate) const REPORT_EVERY_BYTES: u64 = 256 * 1024;
