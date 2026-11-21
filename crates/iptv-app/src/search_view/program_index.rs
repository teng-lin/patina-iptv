//! The programme index of the search screen, built on its own thread from
//! every day the store keeps (`Store::for_each_epg_title`): the guide in
//! memory holds only the last day (`data/pages.rs`), and search reaches
//! back as far as the store does. A guide that is not stored (none on
//! screen) is indexed from memory.

use std::time::Instant;

use iptv_core::guide::Schedule;
use iptv_epg::{ChannelKey, DAY, UnixTime};
use iptv_guide::{EpgSchedule, combined::CombinedSchedule};
use iptv_search::ProgramIndex;

/// Builds the index of `epg`'s channels (by row), completing on the UI thread.
pub fn build(
    schedule: &dyn Schedule,
    rows: usize,
    done: impl FnOnce(Result<ProgramIndex, crate::runtime::Error>) + 'static,
) {
    let mut sources: Vec<(
        crate::runtime::model::SourceId,
        String,
        Vec<ChannelKey>,
        usize,
    )> = Vec::new();
    if let Some(all) = schedule
        .as_any()
        .and_then(|s| s.downcast_ref::<CombinedSchedule>())
    {
        let mut offset = 0;
        for part in all.sources() {
            let keys = (0..part.schedule.channel_count())
                .filter_map(|c| part.schedule.channel(c).map(|c| c.key.clone()))
                .collect();
            if part.schedule.channel_count() == 0 {
                continue;
            }
            let Some(authority) = part
                .schedule
                .source_authority()
                .filter(|authority| authority.source() == part.id)
            else {
                done(Err(crate::runtime::Error::unavailable(
                    "Search source is unavailable",
                )));
                return;
            };
            sources.push((
                crate::runtime::model::SourceId(part.id),
                authority.incarnation().to_owned(),
                keys,
                offset,
            ));
            offset += part.schedule.channel_count();
        }
    } else if let Some(epg) = schedule
        .as_any()
        .and_then(|s| s.downcast_ref::<EpgSchedule>())
    {
        let keys: Vec<_> = (0..rows)
            .map_while(|c| epg.channel(c).map(|g| g.key.clone()))
            .collect();
        let Some(source) = crate::runtime()
            .shown_source()
            .filter(|_| epg.loaded_from().is_some())
        else {
            let mut index = ProgramIndex::build(epg.store(), &keys);
            for channel in 0..rows {
                if let Some(identity) = crate::runtime().search_channel_identity(schedule, channel)
                {
                    index.set_identity(channel, identity);
                }
            }
            done(Ok(index));
            return;
        };
        let Some(authority) = epg
            .source_authority()
            .filter(|authority| authority.source() == source.0)
        else {
            done(Err(crate::runtime::Error::unavailable(
                "Search source is unavailable",
            )));
            return;
        };
        sources.push((source, authority.incarnation().to_owned(), keys, 0));
    } else {
        done(Err(crate::runtime::Error::unavailable(
            "Search guide is unavailable",
        )));
        return;
    }
    let keep = crate::data::stored::keep_past_days(&crate::settings::get());
    let from: UnixTime =
        crate::time::wall_unix().saturating_sub(i64::from(keep).saturating_mul(DAY));
    let started = Instant::now();
    let read = crate::runtime().read_program_index(sources, rows, from);
    crate::data::spawn_in_app(async move {
        match read.await {
            Ok(index) => {
                log::info!(
                    "iptv search: read {} stored programmes in {} ms",
                    index.program_count(),
                    started.elapsed().as_millis()
                );
                done(Ok(index));
            }
            Err(error) => done(Err(error)),
        }
    });
}
