//! The browser store's memory files: chunk tracking, restoring at launch,
//! staging databases as scratch files and publishing on a copy.

use std::collections::BTreeMap;

use iptv_import::{CatalogRow, CatalogSink};
use iptv_store_sqlite::{
    BrowserFiles, CHUNK_BYTES, CatalogStage, ChangedChunks, ChannelQuery, SourceId, Store,
};

use crate::common::{group, m3u_channel, m3u_source};

/// The host's IndexedDB: chunk index to bytes, and the file size.
#[derive(Default)]
struct Saved {
    chunks: BTreeMap<u32, Vec<u8>>,
    size: u64,
}

impl Saved {
    /// What the host does after a commit: takes the changed chunks, copies
    /// them out and writes them; drops chunks past the end.
    fn save(&mut self, store: &Store) -> ChangedChunks {
        let changed = store.take_changed_chunks().unwrap();
        for &index in &changed.indices {
            let mut bytes = Vec::new();
            store.read_chunk(index, &mut bytes).unwrap();
            self.chunks.insert(index, bytes);
        }
        let count = changed.size.div_ceil(CHUNK_BYTES as u64) as u32;
        self.chunks.retain(|&index, _| index < count);
        self.size = changed.size;
        changed
    }

    /// A launch: new files restored from the saved chunks.
    fn restore(&self) -> BrowserFiles {
        let files = BrowserFiles::new().unwrap();
        files.restore_size(self.size).unwrap();
        for (&index, bytes) in &self.chunks {
            files.restore_chunk(index, bytes).unwrap();
        }
        files
    }
}

fn catalog(store: &mut Store, source: SourceId, channels: usize, password: &str) {
    let groups: Vec<_> = (0..4).map(group).collect();
    let channels: Vec<_> = (0..channels)
        .map(|i| m3u_channel(i, groups.len(), password))
        .collect();
    store
        .replace_catalog(source, &groups, &channels, &[], &[])
        .unwrap();
}

fn channel_count(store: &Store, source: SourceId) -> usize {
    store
        .load_channels(source, &ChannelQuery::default())
        .unwrap()
        .len()
}

#[test]
fn a_small_write_changes_few_chunks_and_survives_a_relaunch() {
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    catalog(&mut store, source, 3000, "first");
    let mut saved = Saved::default();
    let first = saved.save(&store);
    let all = first.size.div_ceil(CHUNK_BYTES as u64) as usize;
    assert!(all > 4, "a catalog of 3,000 channels spans several chunks");
    assert_eq!(
        first.indices.len(),
        all,
        "the first save writes every chunk"
    );
    assert!(store.take_changed_chunks().unwrap().is_empty());

    let id = store
        .load_channels(source, &ChannelQuery::default())
        .unwrap()[1234]
        .channel
        .id
        .clone();
    store.set_favorite(source, &id, true).unwrap();
    let toggle = saved.save(&store);
    assert!(
        !toggle.is_empty() && toggle.indices.len() <= 4,
        "a favorite writes a few pages, not the database: {:?}",
        toggle.indices
    );
    drop(store);
    drop(files);

    let files = saved.restore();
    let store = Store::open_browser(&files).unwrap();
    assert_eq!(channel_count(&store, source), 3000);
    assert!(store.channel(source, &id).unwrap().unwrap().favorite);
    assert!(
        store.take_changed_chunks().unwrap().is_empty(),
        "opening a current store writes nothing"
    );
}

#[test]
fn chunks_are_taken_only_between_transactions_and_can_be_given_back() {
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    let changed = store
        .read_snapshot(|store| Ok(store.take_changed_chunks().is_err()))
        .unwrap();
    assert!(changed, "a read snapshot is a transaction");
    let taken = store.take_changed_chunks().unwrap();
    assert!(!taken.is_empty());
    store.return_changed_chunks(&taken).unwrap();
    assert_eq!(store.take_changed_chunks().unwrap(), taken);
    let _ = source;
}

#[test]
fn restoring_checks_chunk_sizes_and_refuses_an_open_database() {
    let files = BrowserFiles::new().unwrap();
    files.restore_size(CHUNK_BYTES as u64 + 10).unwrap();
    assert!(files.restore_chunk(1, &[0; 9]).is_err());
    assert!(files.restore_chunk(2, &[0; 10]).is_err());
    let files = BrowserFiles::new().unwrap();
    let store = Store::open_browser(&files).unwrap();
    assert!(files.restore_size(0).is_err());
    assert!(Store::open_browser(&files).is_err(), "one store per files");
    assert!(
        store.open_reader().is_err(),
        "one connection in the browser"
    );
}

#[test]
fn a_catalog_stage_is_a_scratch_file_of_the_store() {
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    let mut saved = Saved::default();
    saved.save(&store);
    let mut stage =
        CatalogStage::create_in_with("/stages", |path| store.open_staging_connection(path))
            .unwrap();
    stage.push(CatalogRow::Group(group(0))).unwrap();
    for i in 0..500 {
        stage
            .push(CatalogRow::Channel(m3u_channel(i, 1, "staged")))
            .unwrap();
    }
    stage.finish().unwrap();
    assert!(
        store.take_changed_chunks().unwrap().is_empty(),
        "staging writes nothing to the database"
    );
    assert_eq!(
        store.replace_catalog_stage(source, stage).unwrap().channels,
        500
    );
    assert_eq!(channel_count(&store, source), 500);
    saved.save(&store);
    let files = saved.restore();
    assert_eq!(
        channel_count(&Store::open_browser(&files).unwrap(), source),
        500
    );
}

#[test]
fn a_guide_stage_writes_in_place_in_the_browser() {
    use iptv_epg::{FeedId, ProgramData, StoreOptions};
    use iptv_import::GuideSink;
    let files = BrowserFiles::new().unwrap();
    let store = Store::open_browser(&files).unwrap();
    let mut stage = iptv_store_sqlite::EpgStage::create_in_store(&store, "/stages").unwrap();
    stage
        .begin_feed(FeedId(1), 0, StoreOptions::default(), 0)
        .unwrap();
    for i in 0..10_000 {
        let p = ProgramData {
            start: 1_800_000_000 + i * 1800,
            stop: Some(1_800_000_000 + (i + 1) * 1800),
            title: "Synthetic program",
            ..ProgramData::default()
        };
        stage.program("ch1.example", &p).unwrap();
    }
    assert_eq!(stage.finish_feed().unwrap().kept, 10_000);
    assert!(stage.has_programs(FeedId(1), "ch1.example"));
}

#[test]
fn a_publish_copy_takes_writes_while_reads_continue_on_the_database() {
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    catalog(&mut store, source, 2000, "first");
    let mut saved = Saved::default();
    saved.save(&store);

    let mut copy = store.begin_publish().unwrap();
    assert!(store.is_publishing());
    assert!(store.begin_publish().is_err(), "one publish at a time");
    catalog(&mut copy, source, 2500, "second");
    // Reads see the last commit of the database, not the copy.
    assert_eq!(channel_count(&store, source), 2000);
    // A write here would be lost in the swap: it is refused, not dropped.
    let id = store
        .load_channels(source, &ChannelQuery::default())
        .unwrap()[0]
        .channel
        .id
        .clone();
    assert!(store.set_favorite(source, &id, true).is_err());
    assert!(
        store.take_changed_chunks().unwrap().is_empty(),
        "the copy's writes are not the database's"
    );

    store.finish_publish(copy).unwrap();
    assert!(!store.is_publishing());
    assert_eq!(channel_count(&store, source), 2500);
    // Writes held back during the publish run after the swap.
    store.set_favorite(source, &id, true).unwrap();
    saved.save(&store);
    let files = saved.restore();
    let store = Store::open_browser(&files).unwrap();
    assert_eq!(channel_count(&store, source), 2500);
    assert!(store.channel(source, &id).unwrap().unwrap().favorite);
}

#[test]
fn a_swap_marks_only_chunks_that_differ() {
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    catalog(&mut store, source, 3000, "first");
    let mut saved = Saved::default();
    let all = saved.save(&store).indices.len();
    let mut copy = store.begin_publish().unwrap();
    let id = copy
        .load_channels(source, &ChannelQuery::default())
        .unwrap()[10]
        .channel
        .id
        .clone();
    copy.set_favorite(source, &id, true).unwrap();
    store.finish_publish(copy).unwrap();
    let changed = saved.save(&store);
    assert!(
        !changed.is_empty() && changed.indices.len() < all / 2,
        "{} of {all} chunks",
        changed.indices.len()
    );
    let files = saved.restore();
    let store = Store::open_browser(&files).unwrap();
    assert!(store.channel(source, &id).unwrap().unwrap().favorite);
}

#[test]
fn an_abandoned_publish_leaves_the_database_writable_and_unchanged() {
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    catalog(&mut store, source, 100, "first");
    let mut copy = store.begin_publish().unwrap();
    catalog(&mut copy, source, 50, "second");
    let other = Store::open_browser(&BrowserFiles::new().unwrap()).unwrap();
    assert!(
        store.finish_publish(other).is_err(),
        "not this store's copy"
    );
    store.abandon_publish(copy).unwrap();
    assert_eq!(channel_count(&store, source), 100);
    catalog(&mut store, source, 60, "third");
    assert_eq!(channel_count(&store, source), 60);
}
