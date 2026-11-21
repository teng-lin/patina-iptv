//! Where [`crate::import_epg`] puts a guide's programs as they are
//! parsed: a [`GuideSink`]. The import keeps only the feeds' channel lists
//! in memory; the sink holds the programs (on disk for the app,
//! `iptv_store_sqlite::EpgStage`; in memory with [`MemoryGuide`]).

use std::fmt;

use iptv_epg::{
    EpgSource, FeedId, GuideStore, IngestReport, ProgramData, SourceBuilder, StoreOptions, UnixTime,
};

use crate::epg::EpgImport;

/// A sink could not keep what it was given (disk full, I/O). Ends the
/// import ([`crate::ImportError::Storage`]). The message must not quote
/// guide data or addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SinkError(pub String);

impl fmt::Display for SinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SinkError {}

/// Receives one feed at a time: [`Self::begin_feed`], its programs,
/// then [`Self::finish_feed`] (it loaded) or [`Self::discard_feed`] (it
/// failed or the import was canceled).
///
/// A sink applies [`iptv_epg::ScheduleRules`] for the feed's storage
/// settings (retention, duplicates, overlaps) exactly as
/// [`SourceBuilder`] does, and stores descriptions only when the settings
/// say so.
pub trait GuideSink {
    /// Feed `feed` starts. `now` is for retention only.
    fn begin_feed(
        &mut self,
        feed: FeedId,
        priority: i32,
        store: StoreOptions,
        now: UnixTime,
    ) -> Result<(), SinkError>;

    /// One program of EPG channel `channel` of the current feed, as
    /// parsed ([`ProgramData::from_xmltv`]).
    fn program(&mut self, channel: &str, p: &ProgramData<'_>) -> Result<(), SinkError>;

    /// The current feed loaded: settle its schedules. It replaces an
    /// earlier feed with the same id. Returns its counters.
    fn finish_feed(&mut self) -> Result<IngestReport, SinkError>;

    /// The current feed failed: forget its programs.
    fn discard_feed(&mut self) -> Result<(), SinkError>;

    /// After [`Self::finish_feed`]: whether EPG channel `epg_id` of `feed`
    /// kept at least one program.
    fn has_programs(&self, feed: FeedId, epg_id: &str) -> bool;
}

/// Keeps the guide in memory ([`SourceBuilder`] per feed), for tests,
/// tools and runtimes without a store. Holds every program the import
/// keeps: not for large guides on small devices.
#[derive(Default)]
pub struct MemoryGuide {
    building: Option<SourceBuilder>,
    /// Loaded feeds in load order.
    sources: Vec<EpgSource>,
}

impl MemoryGuide {
    pub fn new() -> Self {
        Self::default()
    }

    /// The guide bound as `import` decided.
    pub fn into_guide(self, import: &EpgImport) -> GuideStore {
        let mut guide = GuideStore::new();
        for src in self.sources {
            guide.replace_source(src);
        }
        guide.set_matches(
            import
                .bindings
                .iter()
                .map(|(key, epg)| (key.clone(), vec![epg.clone()]))
                .collect(),
        );
        guide
    }
}

impl GuideSink for MemoryGuide {
    fn begin_feed(
        &mut self,
        feed: FeedId,
        priority: i32,
        store: StoreOptions,
        now: UnixTime,
    ) -> Result<(), SinkError> {
        self.building = Some(SourceBuilder::new(feed, priority, store, now));
        Ok(())
    }

    fn program(&mut self, channel: &str, p: &ProgramData<'_>) -> Result<(), SinkError> {
        match self.building.as_mut() {
            Some(b) => {
                b.push_data(channel, p);
                Ok(())
            }
            None => Err(SinkError("no feed started".into())),
        }
    }

    fn finish_feed(&mut self) -> Result<IngestReport, SinkError> {
        let src = self
            .building
            .take()
            .ok_or_else(|| SinkError("no feed started".into()))?
            .finish();
        let report = src.report();
        self.sources.retain(|s| s.id() != src.id());
        self.sources.push(src);
        Ok(report)
    }

    fn discard_feed(&mut self) -> Result<(), SinkError> {
        self.building = None;
        Ok(())
    }

    fn has_programs(&self, feed: FeedId, epg_id: &str) -> bool {
        self.sources
            .iter()
            .find(|s| s.id() == feed)
            .and_then(|s| s.schedule(epg_id))
            .is_some_and(|s| !s.is_empty())
    }
}
