//! The browser store's memory files: chunk tracking, restoring at launch,
//! staging databases as scratch files and publishing on a copy.

use std::collections::BTreeMap;

use iptv_import::{CatalogRow, CatalogSink};
use iptv_store_sqlite::{BrowserFiles, CHUNK_BYTES, CatalogStage, ChannelQuery, SourceId, Store};

use crate::common::{group, m3u_channel, m3u_source};

/// The host's IndexedDB: chunk index to bytes, and the file size.
#[derive(Default)]
struct Saved {
    chunks: BTreeMap<u32, Vec<u8>>,
    size: u64,
}

/// What one save wrote.
struct Taken {
    size: u64,
    indices: Vec<u32>,
}

impl Taken {
    fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

impl Saved {
    /// What the host does after a commit: takes the changed chunks, writes
    /// them, drops chunks past the end and acknowledges the take.
    fn save(&mut self, store: &Store) -> Taken {
        let changed = store.take_changed_chunks().unwrap();
        for (index, bytes) in changed.chunks() {
            self.chunks.insert(index, bytes.to_vec());
        }
        let count = changed.size().div_ceil(CHUNK_BYTES as u64) as u32;
        self.chunks.retain(|&index, _| index < count);
        self.size = changed.size();
        let taken = Taken {
            size: changed.size(),
            indices: changed.indices().collect(),
        };
        changed.saved();
        taken
    }

    /// A launch: new files restored from the saved chunks.
    fn restore(&self) -> BrowserFiles {
        let files = BrowserFiles::new().unwrap();
        let restore = files.restore_size(self.size).unwrap();
        for (&index, bytes) in &self.chunks {
            files.restore_chunk(restore, index, bytes).unwrap();
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
    let store = Store::open_browser(&files).unwrap();
    let changed = store
        .read_snapshot(|store| Ok(store.take_changed_chunks().is_err()))
        .unwrap();
    assert!(changed, "a read snapshot is a transaction");
    let taken = store.take_changed_chunks().unwrap();
    assert!(!taken.is_empty());
    let indices: Vec<u32> = taken.indices().collect();
    // A write that failed: dropping the take returns its chunks.
    drop(taken);
    let again = store.take_changed_chunks().unwrap();
    assert_eq!(again.indices().collect::<Vec<_>>(), indices);
    again.saved();
    assert!(store.take_changed_chunks().unwrap().is_empty());
}

/// A take is a snapshot of one commit: a commit after it changes neither its
/// bytes nor its size, and no second take is given out before it is saved,
/// so a save never mixes two commits.
#[test]
fn a_take_is_one_commit_and_takes_do_not_overlap() {
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    let mut saved = Saved::default();
    saved.save(&store);
    catalog(&mut store, source, 2000, "first");
    let first = store.take_changed_chunks().unwrap();
    let before: Vec<(u32, Vec<u8>)> = first.chunks().map(|(i, b)| (i, b.to_vec())).collect();
    let size = first.size();
    // A commit before the host has copied the take out: it grows the file.
    catalog(&mut store, source, 4000, "second");
    assert!(
        store.take_changed_chunks().is_err(),
        "no second take before the first is saved"
    );
    let after: Vec<(u32, Vec<u8>)> = first.chunks().map(|(i, b)| (i, b.to_vec())).collect();
    assert_eq!(after, before, "the take still holds the first commit");
    assert_eq!(first.size(), size);
    // Its write fails: the next take covers both commits.
    drop(first);
    saved.save(&store);
    let restored = saved.restore();
    let store = Store::open_browser(&restored).unwrap();
    assert_eq!(channel_count(&store, source), 4000);
}

#[test]
fn restoring_checks_chunk_sizes_and_refuses_an_open_database() {
    let files = BrowserFiles::new().unwrap();
    let restore = files.restore_size(CHUNK_BYTES as u64 + 1024).unwrap();
    assert!(files.restore_chunk(restore, 1, &[0; 512]).is_err());
    assert!(files.restore_chunk(restore, 2, &[0; 1024]).is_err());
    files.restore_chunk(restore, 1, &[0; 1024]).unwrap();
    // Chunk 0 is missing: the store does not open on a hole of zeros.
    assert!(Store::open_browser(&files).is_err());
    // Sizes that are no SQLite database, or absurdly large ones (a corrupt
    // manifest), are refused before anything is allocated.
    assert!(files.restore_size(1000).is_err());
    assert!(
        files
            .restore_size(iptv_store_sqlite::MAX_DATABASE_BYTES + 512)
            .is_err()
    );
    assert!(files.restore_size(1 << 40).is_err());
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
    assert!(!store.is_publishing());
    assert_eq!(channel_count(&store, source), 100);
    catalog(&mut store, source, 60, "third");
    assert_eq!(channel_count(&store, source), 60);
}

/// The browser's guide refresh: the replacement runs in slices on the
/// publish copy, guide reads between slices see the stored guide, the swap
/// shows the new one, and it survives a relaunch.
#[test]
fn a_guide_publishes_in_slices_on_the_copy_while_the_guide_stays_readable() {
    use iptv_epg::{EpgRef, FeedId, GuideStore, MatchKind, SourceBuilder, StoreOptions};
    use iptv_import::EpgSourceConfig;
    use iptv_providers::SecretUrl;
    use iptv_store_sqlite::{EpgFeedMeta, EpgReplacement, EpgWindow, Step};

    const NOW: i64 = 1_800_000_000;
    let build = |tag: &str| {
        let mut feed = SourceBuilder::new(FeedId(1), 1, StoreOptions::default(), NOW);
        for c in 0..50 {
            for p in 0..40 {
                let start = NOW + p * 1800;
                feed.push_program(iptv_epg::XmltvProgram {
                    channel: format!("c{c}.example"),
                    start,
                    stop: Some(start + 1800),
                    title: format!("{tag} {c}/{p}"),
                    ..iptv_epg::XmltvProgram::default()
                });
            }
        }
        let mut guide = GuideStore::new();
        guide.replace_source(feed.finish());
        guide.set_matches(
            (0..50)
                .map(|c| {
                    let candidate = EpgRef {
                        source: FeedId(1),
                        epg_id: format!("c{c}.example"),
                        kind: MatchKind::TvgId,
                    };
                    (iptv_epg::ChannelKey(format!("k{c:02}")), vec![candidate])
                })
                .collect(),
        );
        guide
    };
    let feeds = [EpgFeedMeta {
        config: EpgSourceConfig::new(
            FeedId(1),
            SecretUrl::new("https://guide.example.test/1.xml"),
        ),
        fetched_at: NOW,
        etag: None,
        last_modified: None,
        bytes: 1,
    }];
    let title = |store: &Store, source| {
        let guide = store.load_epg(source, &EpgWindow::from(NOW, 1)).unwrap();
        guide
            .programs(&"k07".into(), NOW, NOW + 1)
            .map(|p| p.title().to_owned())
            .collect::<Vec<_>>()
    };

    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    store.set_clock(|| NOW);
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    crate::staged::replace_epg(&mut store, source, &build("Old"), &feeds, 1).unwrap();
    let mut saved = Saved::default();
    saved.save(&store);

    let copy = store.begin_publish().unwrap();
    let (mut stage, import, _dir) = crate::staged::stage(&copy, &build("New"));
    let mut job = EpgReplacement::new(
        copy,
        source,
        &mut stage,
        std::borrow::Cow::Owned(import),
        feeds.to_vec(),
        2,
    )
    .unwrap();
    let mut slices = 0;
    loop {
        slices += 1;
        match job.step(400).unwrap() {
            Step::Pending => assert_eq!(title(&store, source), ["Old 7/0"]),
            Step::Done(counts) => {
                assert_eq!(counts.programs, 2000);
                break;
            }
        }
    }
    assert!(slices > 5, "{slices} slices");
    let copy = job.into_store();
    drop(stage);
    store.finish_publish(copy).unwrap();
    assert_eq!(title(&store, source), ["New 7/0"]);
    saved.save(&store);
    let files = saved.restore();
    let store = Store::open_browser(&files).unwrap();
    assert_eq!(title(&store, source), ["New 7/0"]);
}

/// Code generic over threads (the runtime's executors) compiles for both
/// targets: a browser store and its jobs are `Send`, as native ones are.
#[test]
fn browser_stores_and_jobs_are_send() {
    fn send<T: Send>() {}
    send::<Store>();
    send::<BrowserFiles>();
    send::<iptv_store_sqlite::CatalogReplacement<'static, Store>>();
    send::<iptv_store_sqlite::EpgReplacement<'static, Store>>();
    send::<iptv_store_sqlite::EpgStage>();
}

/// A publish copy dropped anywhere (an error before its job starts, a job
/// constructor that fails, a job dropped mid-way) abandons the publish: the
/// database takes writes again and is unchanged.
#[test]
fn a_dropped_publish_copy_or_job_abandons_the_publish() {
    use iptv_store_sqlite::{CatalogReplacement, StagePublication, Step};
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    catalog(&mut store, source, 300, "first");
    let id = store
        .load_channels(source, &ChannelQuery::default())
        .unwrap()[0]
        .channel
        .id
        .clone();

    // The copy itself, dropped before any job.
    let copy = store.begin_publish().unwrap();
    assert!(store.set_favorite(source, &id, true).is_err());
    drop(copy);
    assert!(!store.is_publishing());
    store.set_favorite(source, &id, true).unwrap();

    // A job constructor that fails takes the copy with it.
    let copy = store.begin_publish().unwrap();
    let unfinished =
        CatalogStage::create_in_with("/stages", |path| copy.open_staging_connection(path)).unwrap();
    assert!(
        CatalogReplacement::from_stage(copy, source, unfinished, StagePublication::new()).is_err()
    );
    assert!(!store.is_publishing());
    store.set_favorite(source, &id, false).unwrap();

    // A job dropped mid-way: rolled back, copy gone, database unchanged.
    let copy = store.begin_publish().unwrap();
    let groups: Vec<_> = (0..4).map(group).collect();
    let channels: Vec<_> = (0..500).map(|i| m3u_channel(i, 4, "second")).collect();
    let mut job = CatalogReplacement::new(copy, source, groups, channels, Vec::new(), Vec::new());
    for _ in 0..3 {
        assert_eq!(job.step(100).unwrap(), Step::Pending);
    }
    drop(job);
    assert!(!store.is_publishing());
    assert_eq!(channel_count(&store, source), 300);
    store.set_favorite(source, &id, true).unwrap();
    // A new publish can start.
    let copy = store.begin_publish().unwrap();
    store.abandon_publish(copy).unwrap();
}

/// A catalog replacement in slices on the publish copy, swapped in.
#[test]
fn a_catalog_publishes_in_slices_on_the_copy() {
    use iptv_store_sqlite::{CatalogReplacement, Step};
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    catalog(&mut store, source, 300, "first");
    let mut saved = Saved::default();
    saved.save(&store);
    let copy = store.begin_publish().unwrap();
    let groups: Vec<_> = (0..4).map(group).collect();
    let channels: Vec<_> = (0..900).map(|i| m3u_channel(i, 4, "second")).collect();
    let mut job = CatalogReplacement::new(copy, source, groups, channels, Vec::new(), Vec::new());
    let mut slices = 0;
    loop {
        slices += 1;
        match job.step(200).unwrap() {
            Step::Pending => assert_eq!(channel_count(&store, source), 300),
            Step::Done(published) => {
                assert_eq!(published.counts.channels, 900);
                break;
            }
        }
    }
    assert!(slices > 5);
    store.finish_publish(job.into_store()).unwrap();
    assert_eq!(channel_count(&store, source), 900);
    saved.save(&store);
    let files = saved.restore();
    assert_eq!(
        channel_count(&Store::open_browser(&files).unwrap(), source),
        900
    );
}

/// Only the store opens its database: a stage's connection can neither
/// open nor attach it (and so cannot write it behind the publish).
#[test]
fn other_connections_cannot_open_or_attach_the_database() {
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    assert!(
        store
            .open_staging_connection(std::path::Path::new("patina.sqlite"))
            .is_err()
    );
    let copy = store.begin_publish().unwrap();
    let stage = store
        .open_staging_connection(std::path::Path::new("/stages/intruder.sqlite"))
        .unwrap();
    for name in ["patina.sqlite", "patina-publish.sqlite"] {
        assert!(
            stage
                .execute("ATTACH DATABASE ?1 AS other", [name])
                .is_err(),
            "{name}"
        );
    }
    drop(stage);
    store.finish_publish(copy).unwrap();
    assert!(store.sources().unwrap().iter().any(|s| s.id == source));
}

/// Freed pages are zeroed: a deleted source's credential leaves no trace in
/// the saved chunks.
#[test]
fn a_deleted_credential_is_not_in_the_saved_chunks() {
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let mut saved = Saved::default();
    let source = store
        .add_source(&crate::common::xtream_source("Secret"))
        .unwrap();
    saved.save(&store);
    let secret = crate::common::XT_PASSWORD.as_bytes();
    let contains = |saved: &Saved| {
        saved
            .chunks
            .values()
            .any(|bytes| bytes.windows(secret.len()).any(|w| w == secret))
    };
    assert!(contains(&saved), "the credential is stored before");
    store.delete_source(source).unwrap();
    saved.save(&store);
    assert!(
        !contains(&saved),
        "a deleted credential stays in a saved page"
    );
}

/// The file-based operations are refused in the browser build, which has
/// no files, with `Unavailable`.
#[test]
fn file_operations_are_unavailable_in_the_browser() {
    use iptv_store_sqlite::{DatabaseKey, EpgStage, StoreError};
    // Refused for the reason that there are no files, not some other one.
    let no_files = |e: StoreError| matches!(e, StoreError::Unavailable(reason) if reason == "the browser build has no files");
    // Refused with the browser build's way to do it instead.
    let unavailable =
        |e: StoreError| matches!(e, StoreError::Unavailable(reason) if reason.contains("browser"));
    let path = std::path::Path::new("/data/store.sqlite");
    assert!(unavailable(Store::open(path).unwrap_err()));
    assert!(unavailable(
        Store::open_with(path, &iptv_store_sqlite::OpenOptions::default()).unwrap_err()
    ));
    let key = DatabaseKey::from_bytes([7; 32]);
    assert!(no_files(Store::open_encrypted(path, &key).unwrap_err()));
    assert!(unavailable(CatalogStage::create_in("/data").err().unwrap()));
    assert!(unavailable(
        EpgStage::create("/data/stage.sqlite").unwrap_err()
    ));
    assert!(unavailable(EpgStage::create_in("/data").unwrap_err()));
    let store = Store::open_in_memory().unwrap();
    assert!(unavailable(store.open_reader().unwrap_err()));
    assert!(no_files(store.snapshot_to(path).unwrap_err()));
    assert!(no_files(store.seal_snapshot(path).unwrap_err()));
    assert!(no_files(Store::validate_snapshot(path).unwrap_err()));
    assert!(no_files(
        Store::validate_encrypted_snapshot(path, &key).unwrap_err()
    ));
    let mut input: &[u8] = b"synthetic";
    assert!(no_files(
        store.write_private_file(path, &mut input).unwrap_err()
    ));
    assert!(no_files(store.read_private_file(path).err().unwrap()));
    use iptv_store_sqlite::InspectionStage;
    assert!(no_files(InspectionStage::create(path, &key).err().unwrap()));
    assert!(no_files(InspectionStage::open(path, &key).err().unwrap()));
}

/// A restore is refused while a take is out: the host's write of it would
/// land on the restored database and mix two databases in storage.
#[test]
fn a_restore_waits_for_the_outstanding_take() {
    use iptv_store_sqlite::StoreError;
    let files = BrowserFiles::new().unwrap();
    let store = Store::open_browser(&files).unwrap();
    let take = store.take_changed_chunks().unwrap();
    drop(store);
    assert!(matches!(
        files.restore_size(0),
        Err(StoreError::Unavailable(reason)) if reason.contains("take")
    ));
    take.saved();
    let restore = files.restore_size(0).unwrap();
    // An empty file has no chunk to give; the store opens.
    assert!(files.restore_chunk(restore, 0, &[0; 512]).is_err());
    Store::open_browser(&files).unwrap();
}

/// A chunk of a superseded restore (read late from IndexedDB after the host
/// restarted the restore) cannot enter the current one, nor a database the
/// store has opened.
#[test]
fn a_late_chunk_of_an_old_restore_is_refused() {
    use iptv_store_sqlite::StoreError;
    let files = BrowserFiles::new().unwrap();
    let old = files.restore_size(8192).unwrap();
    let new = files.restore_size(8192).unwrap();
    assert!(matches!(
        files.restore_chunk(old, 0, &[1; 8192]),
        Err(StoreError::Unavailable(_))
    ));
    // The new restore still misses its chunk: the store does not open.
    assert!(Store::open_browser(&files).is_err());
    let mut saved = Saved::default();
    let source = {
        let files = BrowserFiles::new().unwrap();
        let mut store = Store::open_browser(&files).unwrap();
        let source = store.add_source(&m3u_source("Synthetic")).unwrap();
        saved.save(&store);
        source
    };
    let files = BrowserFiles::new().unwrap();
    let restore = files.restore_size(saved.size).unwrap();
    for (&index, bytes) in &saved.chunks {
        files.restore_chunk(restore, index, bytes).unwrap();
    }
    let store = Store::open_browser(&files).unwrap();
    assert!(store.sources().unwrap().iter().any(|s| s.id == source));
    // Opening ends the restore: a late chunk of it is refused too.
    drop(store);
    let (&index, bytes) = saved.chunks.iter().next().unwrap();
    assert!(matches!(
        files.restore_chunk(restore, index, bytes),
        Err(StoreError::Unavailable(_))
    ));
    let _ = new;
}

/// A replacement that committed some steps on a publish copy and was then
/// dropped leaves half a publish: swapping it in is refused.
#[test]
fn a_partly_published_copy_is_not_swapped_in() {
    use iptv_store_sqlite::{CatalogReplacement, Step};
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    catalog(&mut store, source, 300, "first");
    let mut copy = store.begin_publish().unwrap();
    let groups: Vec<_> = (0..4).map(group).collect();
    let channels: Vec<_> = (0..500).map(|i| m3u_channel(i, 4, "second")).collect();
    let mut job =
        CatalogReplacement::new(&mut copy, source, groups, channels, Vec::new(), Vec::new());
    for _ in 0..3 {
        assert_eq!(job.step(100).unwrap(), Step::Pending);
    }
    drop(job);
    assert!(store.finish_publish(copy).is_err());
    assert!(!store.is_publishing());
    assert_eq!(channel_count(&store, source), 300);
}

/// A job whose committed slices changed nothing on the copy (an empty stage
/// for a new source) does not break the copy when it is dropped.
#[test]
fn a_job_that_changed_nothing_leaves_the_copy_publishable() {
    use iptv_store_sqlite::{CatalogReplacement, StagePublication, Step};
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    let source = store.add_source(&m3u_source("Synthetic")).unwrap();
    let mut copy = store.begin_publish().unwrap();
    let mut stage =
        CatalogStage::create_in_with("/stages", |path| copy.open_staging_connection(path)).unwrap();
    stage.finish().unwrap();
    let mut job =
        CatalogReplacement::from_stage(&mut copy, source, stage, StagePublication::new()).unwrap();
    assert_eq!(job.step(1).unwrap(), Step::Pending);
    assert_eq!(job.step(1).unwrap(), Step::Pending);
    drop(job);
    store.finish_publish(copy).unwrap();
}

/// A stage belongs to the store whose files hold it: another store's stage
/// at the same path is refused, not published from the wrong file.
#[test]
fn a_stage_of_another_store_is_refused() {
    use iptv_store_sqlite::{CatalogReplacement, StagePublication};
    // Both stages at one explicit path, each in its own store's files.
    let path = std::path::PathBuf::from("/stages/same.sqlite");
    let stage_of = |store: &Store, channels: usize| {
        let conn = store.open_staging_connection(&path).unwrap();
        let mut stage = CatalogStage::from_connection(path.clone(), conn).unwrap();
        for i in 0..channels {
            stage
                .push(CatalogRow::Channel(m3u_channel(i, 1, "staged")))
                .unwrap();
        }
        stage.finish().unwrap();
        stage
    };
    let mut a = Store::open_in_memory().unwrap();
    let mut b = Store::open_in_memory().unwrap();
    let source = b.add_source(&m3u_source("Synthetic")).unwrap();
    a.add_source(&m3u_source("Synthetic")).unwrap();
    let theirs = stage_of(&a, 5);
    // B holds a stage at the same path: the path alone would match.
    let ours = stage_of(&b, 2);
    assert!(
        CatalogReplacement::from_stage(&mut b, source, theirs, StagePublication::new())
            .unwrap()
            .finish()
            .is_err()
    );
    let published = CatalogReplacement::from_stage(&mut b, source, ours, StagePublication::new())
        .unwrap()
        .finish()
        .unwrap();
    assert_eq!(published.counts.channels, 2);
    let _ = &mut a;
}
