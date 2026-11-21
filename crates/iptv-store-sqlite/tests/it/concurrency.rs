//! Readers on a second connection while a writer replaces a catalog (WAL).

use crate::common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use common::*;
use iptv_providers::SecretUrl;
use iptv_store_sqlite::{ChannelQuery, GuideSource, OpenOptions, Store, StoreError};

#[test]
fn reader_sees_the_last_commit_while_a_write_is_pending() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let mut writer = Store::open(&path).unwrap();
    assert_eq!(writer.journal_mode().unwrap(), "wal");
    let src = writer.add_source(&xtream_source("Xt")).unwrap();
    replace(&mut writer, src, &catalog(10, 2, XT_PASSWORD));

    let mut reader = Store::open_with(
        &path,
        &OpenOptions {
            busy_timeout: Duration::from_millis(50),
            ..Default::default()
        },
    )
    .unwrap();

    // A second writer holds the write lock with uncommitted deletes.
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch(
        "BEGIN IMMEDIATE;
         DELETE FROM channel_groups;
         DELETE FROM channels;",
    )
    .unwrap();

    // Reads proceed on the last committed snapshot, without waiting.
    let all = ChannelQuery::default();
    assert_eq!(reader.channel_count(src, &all).unwrap(), 10);
    assert_eq!(reader.load_channels(src, &all).unwrap().len(), 10);
    // Writes wait for the lock, then fail as busy instead of hanging.
    let id = catalog(1, 1, XT_PASSWORD).channels[0].id.clone();
    match reader.set_favorite(src, &id, true) {
        Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(e, _))) => {
            assert_eq!(e.code, rusqlite::ErrorCode::DatabaseBusy);
        }
        other => panic!("expected busy, got {other:?}"),
    }

    raw.execute_batch("ROLLBACK").unwrap();
    replace(&mut writer, src, &catalog(4, 1, XT_PASSWORD));
    assert_eq!(reader.channel_count(src, &all).unwrap(), 4);
}

/// The app's reader connection: it shares the writer's key, reads the
/// last commit while another connection holds the write lock, and refuses
/// every write, also through methods that take `&self`.
#[test]
fn a_reader_reads_during_a_held_write_and_cannot_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let key = iptv_store_sqlite::DatabaseKey::from_bytes([0x5a; 32]);
    let mut writer = Store::open_encrypted(&path, &key).unwrap();
    let src = writer.add_source(&xtream_source("Xt")).unwrap();
    replace(&mut writer, src, &catalog(10, 2, XT_PASSWORD));
    let reader = writer.open_reader().unwrap();
    assert_eq!(
        reader.schema_version().unwrap(),
        iptv_store_sqlite::SCHEMA_VERSION
    );

    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.pragma_update(None, "key", format!("x'{}'", "5a".repeat(32)))
        .unwrap();
    raw.execute_batch("BEGIN IMMEDIATE; DELETE FROM channel_groups; DELETE FROM channels;")
        .unwrap();
    let all = ChannelQuery::default();
    let started = std::time::Instant::now();
    assert_eq!(reader.channel_count(src, &all).unwrap(), 10);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the read waited for the writer: {:?}",
        started.elapsed()
    );
    raw.execute_batch("ROLLBACK").unwrap();

    // `query_only`: SQLite refuses the write whatever the method's receiver.
    let refused = |result: iptv_store_sqlite::Result<()>| match result {
        Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(e, _))) => {
            assert_eq!(e.code, rusqlite::ErrorCode::ReadOnly);
        }
        other => panic!("expected a read-only refusal, got {other:?}"),
    };
    refused(reader.mark_private_documents_unsealed());
    let mut reader = reader;
    let id = catalog(1, 1, XT_PASSWORD).channels[0].id.clone();
    refused(reader.set_favorite(src, &id, true));

    // It sees what the writer commits afterwards.
    replace(&mut writer, src, &catalog(4, 1, XT_PASSWORD));
    assert_eq!(reader.channel_count(src, &all).unwrap(), 4);
    assert!(
        Store::open_in_memory().unwrap().open_reader().is_err(),
        "an in-memory store has no second connection"
    );
}

/// Two refreshes save at once: a write that reads first (whether the
/// source exists) waits for the other writer's commit and goes on, instead
/// of failing as "database is locked" on a snapshot that commit made
/// stale (WAL's `SQLITE_BUSY_SNAPSHOT`, which no busy timeout waits out).
#[test]
fn writes_that_read_first_wait_for_another_writers_commit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let mut first = Store::open(&path).unwrap();
    let src = first.add_source(&xtream_source("Xt")).unwrap();
    let mut second = Store::open(&path).unwrap();
    // Another writer holds the lock and commits a change a moment later.
    let hold = |path: &std::path::Path| {
        let raw = rusqlite::Connection::open(path).unwrap();
        raw.execute_batch(
            "BEGIN IMMEDIATE;
             UPDATE sources SET updated_at = updated_at + 1;",
        )
        .unwrap();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            raw.execute_batch("COMMIT").unwrap();
        })
    };
    let other = hold(&path);
    second
        .update_source(src, &xtream_source("Renamed"))
        .unwrap();
    other.join().unwrap();
    let other = hold(&path);
    second.set_guide_sources(src, &[]).unwrap();
    other.join().unwrap();
    assert_eq!(first.source(src).unwrap().unwrap().source.name, "Renamed");
}

#[test]
fn concurrent_reads_never_see_a_partial_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let mut writer = Store::open(&path).unwrap();
    let src = writer.add_source(&xtream_source("Xt")).unwrap();
    let small = catalog(2_000, 20, XT_PASSWORD);
    let large = catalog(3_000, 30, "other-pass");
    replace(&mut writer, src, &small);

    let done = Arc::new(AtomicBool::new(false));
    let start = Arc::new(Barrier::new(2));
    let reader = {
        let (done, start, path) = (done.clone(), start.clone(), path.clone());
        thread::spawn(move || {
            let reader = Store::open(&path).unwrap();
            start.wait();
            let mut reads = 0u32;
            let mut seen = std::collections::BTreeSet::new();
            while !done.load(Ordering::Acquire) || reads < 5 {
                let count = reader.channel_count(src, &ChannelQuery::default()).unwrap();
                assert!(count == 2_000 || count == 3_000, "partial catalog: {count}");
                let page = reader
                    .load_channels(
                        src,
                        &ChannelQuery {
                            offset: 1_990,
                            limit: Some(20),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                assert!(
                    page.len() == 10 || page.len() == 20,
                    "torn page: {}",
                    page.len()
                );
                seen.insert(count);
                reads += 1;
            }
            (reads, seen)
        })
    };
    start.wait();
    for round in 0..10 {
        let next = if round % 2 == 0 { &large } else { &small };
        replace(&mut writer, src, next);
    }
    done.store(true, Ordering::Release);
    let (reads, seen) = reader.join().unwrap();
    assert!(reads >= 5);
    assert!(!seen.is_empty());
}

/// Two refreshes saving their playlists' guide sources at once (the app's
/// "Refresh all"), each on its own connection: neither fails as locked. A
/// write that reads first in a deferred transaction cannot wait for the
/// lock once another commit made its snapshot stale (`SQLITE_BUSY` at once,
/// whatever the busy timeout), so such writes take the lock up front.
#[test]
fn concurrent_guide_source_edits_wait_for_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let mut setup = Store::open(&path).unwrap();
    let sources = [
        setup.add_source(&m3u_source("A")).unwrap(),
        setup.add_source(&m3u_source("B")).unwrap(),
    ];
    drop(setup);
    let start = Arc::new(Barrier::new(2));
    let writers: Vec<_> = sources
        .into_iter()
        .map(|src| {
            let (start, path) = (start.clone(), path.clone());
            thread::spawn(move || {
                let mut store = Store::open(&path).unwrap();
                let guides = [GuideSource {
                    feed: iptv_epg::FeedId(1),
                    provider_index: Some(0),
                    url: SecretUrl::new("http://guide.example.test/g.xml"),
                    name: None,
                    enabled: true,
                    shift_minutes: 0,
                }];
                start.wait();
                for _ in 0..300 {
                    store.set_guide_sources(src, &guides).unwrap();
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
}

/// Hold another writer through the beginning of a mutation. Deferred readers
/// see the old commit and cannot promote that snapshot; Immediate mutations
/// wait before reading, then use the newly committed state.
fn held_mutation_writer(
    path: &std::path::Path,
    update: impl FnOnce(&rusqlite::Connection),
) -> std::thread::JoinHandle<()> {
    let raw = rusqlite::Connection::open(path).unwrap();
    raw.execute_batch("BEGIN IMMEDIATE").unwrap();
    update(&raw);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        raw.execute_batch("COMMIT").unwrap();
    })
}

#[test]
fn recording_favorite_and_custom_order_mutations_wait_for_the_other_writer() {
    use iptv_core::recording::{JobId, Request};
    use iptv_store_sqlite::{ChannelOrder, ChannelScope, MoveTarget};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic-mutations.db");
    let mut writer = Store::open(&path).unwrap();
    let source = writer
        .add_source(&m3u_source("Synthetic mutations"))
        .unwrap();
    let catalog = catalog(3, 3, XT_PASSWORD);
    replace(&mut writer, source, &catalog);
    let first = &catalog.channels[0].id;
    let second = &catalog.channels[1].id;
    writer.set_favorite(source, first, true).unwrap();
    writer.set_favorite(source, second, true).unwrap();
    let group = writer
        .create_custom_group(source, "Synthetic custom")
        .unwrap();
    for channel in &catalog.channels {
        writer.add_to_custom_group(group, &channel.id).unwrap();
    }
    let mut mutation = Store::open(&path).unwrap();
    let hold = |path: &std::path::Path| {
        held_mutation_writer(path, |raw| {
            raw.execute("UPDATE sources SET updated_at=updated_at+1", [])
                .unwrap();
        })
    };
    let request = Request {
        source: source.0,
        channel: first.as_str().into(),
        channel_name: "Synthetic channel".into(),
        title: "Synthetic programme".into(),
        start: 1_800_000_000,
        end: 1_800_003_600,
    };
    let other = hold(&path);
    let scheduled = mutation.schedule_recording(JobId(101), &request, request.start - 1);
    other.join().unwrap();
    assert_eq!(scheduled.unwrap(), JobId(101));

    let other = hold(&path);
    let moved = mutation.move_favorite(source, first, 1);
    other.join().unwrap();
    assert!(moved.unwrap());
    let favorites = mutation
        .load_channels(
            source,
            &ChannelQuery {
                scope: ChannelScope::All,
                favorites_only: true,
                order: ChannelOrder::Effective,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(&favorites[0].channel.id, second);
    assert_eq!(&favorites[1].channel.id, first);

    let other = hold(&path);
    let moved = mutation.move_in_custom_group_to(group, first, MoveTarget::Step(1));
    other.join().unwrap();
    assert!(moved.unwrap());
    let custom = mutation
        .load_channels(
            source,
            &ChannelQuery {
                scope: ChannelScope::Custom(group),
                order: ChannelOrder::Effective,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(&custom[0].channel.id, second);
    assert_eq!(&custom[1].channel.id, first);
}

#[test]
fn group_move_reads_the_order_after_a_competing_commit() {
    use iptv_store_sqlite::MoveTarget;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic-group-order.db");
    let mut writer = Store::open(&path).unwrap();
    let source = writer.add_source(&m3u_source("Synthetic groups")).unwrap();
    let catalog = catalog(3, 3, XT_PASSWORD);
    replace(&mut writer, source, &catalog);
    let groups: Vec<_> = writer
        .library_groups(source)
        .unwrap()
        .into_iter()
        .filter(|row| row.group.kind == iptv_providers::model::ContentKind::Live)
        .map(|row| row.group.id)
        .collect();
    assert_eq!(groups.len(), 3);
    let mut mutation = Store::open(&path).unwrap();
    let other = held_mutation_writer(&path, |raw| {
        for (position, id) in [&groups[1], &groups[2], &groups[0]].into_iter().enumerate() {
            raw.execute(
                "INSERT INTO group_overlays(source_id,group_id,position) VALUES(?1,?2,?3)
                 ON CONFLICT(source_id,group_id) DO UPDATE SET position=excluded.position",
                rusqlite::params![source.0, id.as_str(), position as i64],
            )
            .unwrap();
        }
    });
    let moved = mutation.move_group_to(source, &groups[1], MoveTarget::Step(1));
    other.join().unwrap();
    moved.unwrap();
    let after: Vec<_> = mutation
        .library_groups(source)
        .unwrap()
        .into_iter()
        .filter(|row| row.group.kind == iptv_providers::model::ContentKind::Live)
        .map(|row| row.group.id)
        .collect();
    assert_eq!(
        after,
        [groups[2].clone(), groups[1].clone(), groups[0].clone()]
    );
}

#[test]
fn concurrent_identical_recording_requests_commit_one_job() {
    use iptv_core::recording::{JobId, Request};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic-identical-recordings.db");
    let mut setup = Store::open(&path).unwrap();
    let source = setup
        .add_source(&m3u_source("Synthetic recordings"))
        .unwrap();
    let catalog = catalog(1, 1, XT_PASSWORD);
    replace(&mut setup, source, &catalog);
    let request = Request {
        source: source.0,
        channel: catalog.channels[0].id.as_str().into(),
        channel_name: "Synthetic channel".into(),
        title: "Synthetic programme".into(),
        start: 1_800_000_000,
        end: 1_800_003_600,
    };
    let ready = Arc::new(Barrier::new(2));
    let writers: Vec<_> = (0..2)
        .map(|index| {
            let path = path.clone();
            let request = request.clone();
            let ready = ready.clone();
            thread::spawn(move || {
                let mut store = Store::open(path).unwrap();
                ready.wait();
                store.schedule_recording(JobId(201 + index), &request, request.start - 1)
            })
        })
        .collect();
    let jobs: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().unwrap().unwrap())
        .collect();
    assert_eq!(jobs[0], jobs[1]);
    assert_eq!(setup.recording_jobs().unwrap().len(), 1);
}
