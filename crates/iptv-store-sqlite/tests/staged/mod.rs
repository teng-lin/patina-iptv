//! Stores an in-memory [`GuideStore`] the way a download is stored: its
//! bound schedules streamed into an [`EpgStage`], then
//! [`Store::replace_epg`]. Tests build small guides by hand this way.
#![allow(dead_code)]

use std::collections::HashSet;

use iptv_epg::{ChannelKey, EpgRef, GuideStore, StoreOptions};
use iptv_import::{EpgImport, GuideSink, ListedChannel};
use iptv_store_sqlite::{EpgCounts, EpgFeedMeta, EpgStage, Result, SourceId, Store};

/// `guide`'s bound schedules (every program, descriptions included)
/// and its feeds' channel lists, as an import of them would leave them.
pub fn stage(guide: &GuideStore) -> (EpgStage, EpgImport, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut stage = EpgStage::create_in(dir.path()).unwrap();
    let mut bindings: Vec<(ChannelKey, EpgRef)> = guide
        .bindings()
        .map(|(k, e)| (k.clone(), e.clone()))
        .collect();
    bindings.sort_by(|a, b| a.0.cmp(&b.0));
    let mut listed = Vec::new();
    for src in guide.sources() {
        let opts = StoreOptions {
            store_descriptions: true,
            past_days_to_keep: None,
        };
        stage.begin_feed(src.id(), src.priority(), opts, 0).unwrap();
        let mut done = HashSet::new();
        for (_, epg) in bindings.iter().filter(|(_, e)| e.source == src.id()) {
            if !done.insert(epg.epg_id.as_str()) {
                continue;
            }
            for p in src.schedule(&epg.epg_id).into_iter().flat_map(|s| s.iter()) {
                stage.program(&epg.epg_id, &p.data()).unwrap();
            }
        }
        stage.finish_feed().unwrap();
        let channels = src
            .channels()
            .iter()
            .map(|c| ListedChannel {
                id: c.id.clone(),
                name: c.display_names.first().cloned(),
            })
            .collect();
        listed.push((src.id(), channels));
    }
    let import = EpgImport {
        bindings,
        listed,
        ..EpgImport::default()
    };
    (stage, import, dir)
}

/// Replaces `source`'s stored guide with `guide`.
pub fn replace_epg(
    store: &mut Store,
    source: SourceId,
    guide: &GuideStore,
    feeds: &[EpgFeedMeta],
    match_fingerprint: u64,
) -> Result<EpgCounts> {
    let (mut stage, import, _dir) = stage(guide);
    store.replace_epg(source, &mut stage, &import, feeds, match_fingerprint)
}
