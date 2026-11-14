//! Owned search indexes and their native database reader.
use iptv_epg::{ChannelKey, UnixTime};
use iptv_library::SourceId;
use iptv_search::{ChannelIndex, ProgrammeIndex, ProgrammeIndexBuilder};
use iptv_store_sqlite::StoreError;
#[derive(Default)]
pub struct Indexes {
    /// The `data_version` they were built for.
    pub version: Option<u64>,
    pub channels: ChannelIndex,
    /// Every day the store keeps; dropped when the screen closes (it is
    /// about the size of a week's guide).
    pub programmes: Option<ProgrammeIndex>,
    /// The `data_version` whose programme index is being built.
    pub building: Option<u64>,
}

fn programmes_native(
    sources: Vec<(SourceId, Vec<ChannelKey>, usize)>,
    rows: usize,
    from: UnixTime,
) -> impl std::future::Future<Output = Result<ProgrammeIndex, StoreError>> {
    super::store_service::read(move |store| {
        let mut builder = ProgrammeIndexBuilder::new(rows);
        for (source, keys, offset) in sources {
            store.for_each_epg_title(
                source,
                &keys,
                from,
                UnixTime::MAX,
                |channel, start, stop, title| builder.push(channel + offset, start, stop, title),
            )?;
        }
        Ok(builder.finish())
    })
}

impl crate::Runtime {
    pub fn prepare_search_channels(&self, version: u64, schedule: &dyn iptv_core::guide::Schedule) {
        if self.search_indexes.borrow().version == Some(version) {
            return;
        }
        let mut channels = ChannelIndex::from_schedule(schedule);
        for channel in 0..schedule.channel_count() {
            if !schedule.channel_hidden(channel) {
                channels.add_key(channel, &schedule.channel_number(channel).to_string());
            }
        }
        *self.search_indexes.borrow_mut() = Indexes {
            version: Some(version),
            channels,
            programmes: None,
            building: None,
        };
    }
    pub fn has_programme_index(&self) -> bool {
        self.search_indexes.borrow().programmes.is_some()
    }
    pub fn begin_programme_index(&self, version: u64) -> bool {
        let mut indexes = self.search_indexes.borrow_mut();
        let wanted = indexes.version == Some(version) && indexes.programmes.is_none();
        wanted && indexes.building.replace(version) != Some(version)
    }
    pub fn finish_programme_index(&self, version: u64) -> bool {
        let mut indexes = self.search_indexes.borrow_mut();
        if indexes.building == Some(version) {
            indexes.building = None;
        }
        indexes.version == Some(version)
    }
    pub fn install_programme_index(&self, index: ProgrammeIndex) {
        self.search_indexes.borrow_mut().programmes = Some(index);
    }
    pub fn release_programme_index(&self) {
        self.search_indexes.borrow_mut().programmes = None;
    }
    pub fn search_channels(
        &self,
        query: &str,
        favorites: &std::collections::BTreeSet<usize>,
        options: iptv_search::ChannelSearchOptions,
    ) -> Vec<iptv_search::ChannelHit> {
        self.search_indexes
            .borrow()
            .channels
            .search_channels(query, favorites, options)
    }
    pub fn search_programmes(
        &self,
        query: &str,
        now: UnixTime,
        options: iptv_search::ProgrammeSearchOptions<'_>,
    ) -> Vec<iptv_search::ProgrammeHit> {
        self.search_indexes
            .borrow()
            .programmes
            .as_ref()
            .map_or_else(Vec::new, |index| {
                index.search_programmes(query, now, options)
            })
    }
}

pub fn programmes(
    sources: Vec<(SourceId, Vec<ChannelKey>, usize)>,
    rows: usize,
    from: UnixTime,
) -> impl std::future::Future<Output = Result<ProgrammeIndex, crate::Error>> {
    async move {
        programmes_native(sources, rows, from)
            .await
            .map_err(crate::Error::from)
    }
}
