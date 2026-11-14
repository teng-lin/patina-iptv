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

pub fn programmes(
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
