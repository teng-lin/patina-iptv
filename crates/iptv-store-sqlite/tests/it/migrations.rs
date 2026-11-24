//! Migrations from every historical schema, with data in it.
//!
//! `tests/fixtures/schema_vN.sql` is the exact DDL a build at version N
//! wrote (a `schema_dump`); `tests/fixtures/data_vN.sql` is synthetic data
//! in that shape. Each test builds an old file from those fixtures, opens it
//! with the current build and checks the data through the public API.
//!
//! Adding migration N+1: run
//! `cargo test -p iptv-store-sqlite --lib -- --ignored write_missing_schema_snapshots`
//! (writes `schema_vN+1.sql`, never overwrites), write `data_vN+1.sql` with
//! synthetic data in the new shape, and add it to `HISTORY` below.

use crate::staged;

use std::path::Path;

use iptv_epg::{ChannelKey, GuideStore, MatchIndex, Overrides, PlaylistChannel, SourceBuilder};
use iptv_import::EpgSourceConfig;
use iptv_providers::SecretUrl;
use iptv_providers::model::{ContentKind, SourceConfig, SourceKind};
use iptv_providers::{ContentId, GroupId};
use iptv_store_sqlite::migrate::schema_dump;
use iptv_store_sqlite::{
    ChannelOrder, ChannelQuery, ChannelScope, EpgFeedMeta, EpgOverride, EpgWindow, FeedResult,
    GuideChannel, GuideSource, ImportErrorKind, SCHEMA_VERSION, SourceId, Store,
};

#[test]
fn latest_snapshot_is_the_fresh_schema() {
    let snapshot = std::fs::read_to_string(format!(
        "{}/tests/fixtures/schema_v{SCHEMA_VERSION}.sql",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("snapshot the latest schema (see module docs)");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fresh.db");
    drop(Store::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(schema_dump(&conn).unwrap(), snapshot.replace("\r\n", "\n"));
}

#[test]
fn guide_preferences_survive_reopening_a_v21_store() {
    use iptv_core::organization::Sort;
    use iptv_library::OrganizationScope;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("guide-preferences.db");
    build_old_file(&path, HISTORY.iter().find(|v| v.version == 21).unwrap());
    let store = Store::open(path).unwrap();
    let scope = OrganizationScope::GroupChannels(GroupId::from_stored("xt:live-group:1"));
    assert_eq!(store.guide_sort(SRC, &scope).unwrap(), Sort::Name);
    assert!(store.guide_channel_blocked(SRC, "xt:live:101").unwrap());
    assert!(!store.guide_channel_blocked(SRC, "xt:live:102").unwrap());
}

#[test]
fn existing_episode_progress_gains_a_resume_identity_when_upgraded() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("generated-v16.sqlite");
    build_old_file(&path, HISTORY.iter().find(|v| v.version == 16).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "INSERT INTO vod_progress(source_id,kind,content_id,parent_id,position_ms,duration_ms,completed,updated_at)
         SELECT source_id,'episode','generated-episode-five',id,37000,120000,0,1800000017 FROM series ORDER BY source_id,id LIMIT 1;
         INSERT INTO vod_progress(source_id,kind,content_id,position_ms,duration_ms,completed,updated_at)
         SELECT source_id,'series',id,37000,120000,0,1800000017 FROM series ORDER BY source_id,id LIMIT 1;",
    ).unwrap();
    let (source, id): (i64, String) = conn
        .query_row(
            "SELECT source_id,id FROM series ORDER BY source_id,id LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    drop(conn);
    let store = Store::open(path).unwrap();
    let series = iptv_core::vod::Key {
        source,
        kind: iptv_core::vod::Kind::Series,
        id,
    };
    let episode = store.vod_resume_episode(&series).unwrap().unwrap();
    assert_eq!(episode.id, "generated-episode-five");
    assert_eq!(store.vod_progress(&episode).unwrap().position_ms, 37_000);
}

/// Schema 25 spells stored names the US way (#139): the rows of `favourites`
/// and `epg_programmes`, a `cancelled` recording and `cancelled` import
/// kinds written by a v24 build all survive the upgrade, with their ids and
/// children.
#[test]
fn a_v24_store_keeps_its_data_under_us_spelling() {
    use iptv_core::recording::{JobId, Status};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("generated-v24.db");
    build_old_file(&path, HISTORY.iter().find(|v| v.version == 24).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "INSERT INTO recording_jobs(id,source_id,channel_id,channel_name,title,starts_at,ends_at,
             status,message,source_incarnation)
         SELECT 43,source_id,channel_id,channel_name,title,starts_at+120,ends_at+120,
             'cancelled','Cancelled',source_incarnation FROM recording_jobs WHERE id=41;
         INSERT INTO recording_jobs(id,source_id,channel_id,channel_name,title,starts_at,ends_at,
             status,message,source_incarnation)
         SELECT 45,source_id,channel_id,channel_name,title,starts_at+240,ends_at+240,
             'cancelled','Recurring schedule disabled',source_incarnation
         FROM recording_jobs WHERE id=41;
         INSERT INTO recording_rule_jobs(job_id,rule_id,starts_at) VALUES(45,71,1800000240);
         UPDATE source_imports SET last_error_kind='cancelled',movie_error_kind='cancelled',
             series_error_kind='cancelled' WHERE source_id=1;
         UPDATE epg_guides SET last_error_kind='cancelled';
         UPDATE epg_feed_results SET error_kind='cancelled';",
    )
    .unwrap();
    let old_favorites: i64 = conn
        .query_row("SELECT COUNT(*) FROM favourites", [], |r| r.get(0))
        .unwrap();
    let old_programs: i64 = conn
        .query_row("SELECT COUNT(*) FROM epg_programmes", [], |r| r.get(0))
        .unwrap();
    let old_rule_jobs: i64 = conn
        .query_row("SELECT COUNT(*) FROM recording_rule_jobs", [], |r| r.get(0))
        .unwrap();
    let old_kinds: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM epg_guides WHERE last_error_kind='cancelled')
                  + (SELECT COUNT(*) FROM epg_feed_results WHERE error_kind='cancelled')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(old_favorites > 0 && old_programs > 0 && old_rule_jobs >= 3 && old_kinds > 0);
    drop(conn);

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let jobs = store.recording_jobs().unwrap();
    let canceled = jobs.iter().find(|j| j.id == JobId(43)).unwrap();
    assert_eq!(canceled.status, Status::Canceled);
    assert_eq!(canceled.message, "Canceled");
    // A job a disabled rule canceled keeps the message that lets the rule
    // restore it, and its link to the rule.
    let disabled = jobs.iter().find(|j| j.id == JobId(45)).unwrap();
    assert_eq!(
        (disabled.status, disabled.message.as_str()),
        (Status::Canceled, "Recurring schedule disabled")
    );
    let completed = jobs.iter().find(|j| j.id == JobId(41)).unwrap();
    assert_eq!(
        (completed.status, completed.bytes),
        (Status::Completed, 18_800)
    );
    assert_eq!(
        store.recording_progress(JobId(41)).unwrap().position_ms,
        12_000
    );
    check_recording_completion(&store, 24);
    let status = store.import_status(SRC).unwrap();
    assert_eq!(status.last_error, Some(ImportErrorKind::Canceled));
    assert_eq!(status.series_error, Some(ImportErrorKind::Canceled));
    assert_eq!(status.movies_error, Some(ImportErrorKind::Canceled));
    // check_guide covers v24 in every_historical_version_migrates_with_its_data;
    // here every guide error kind was set to the old canceled slug instead.
    check_overlays(&store, 24);
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(count("SELECT COUNT(*) FROM favorites"), old_favorites);
    assert_eq!(count("SELECT COUNT(*) FROM epg_programs"), old_programs);
    assert_eq!(
        count("SELECT COUNT(*) FROM recording_rule_jobs"),
        old_rule_jobs
    );
    assert_eq!(
        count(
            "SELECT (SELECT COUNT(*) FROM epg_guides WHERE last_error_kind='canceled')
                  + (SELECT COUNT(*) FROM epg_feed_results WHERE error_kind='canceled')"
        ),
        old_kinds
    );
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE '%favourite%' OR name LIKE '%programme%' OR sql LIKE '%cancelled%'"
        ),
        0
    );
    assert_eq!(
        count(
            "SELECT (SELECT COUNT(*) FROM source_imports WHERE 'cancelled' IN (last_error_kind,movie_error_kind,series_error_kind))
                  + (SELECT COUNT(*) FROM epg_guides WHERE last_error_kind='cancelled')
                  + (SELECT COUNT(*) FROM epg_feed_results WHERE error_kind='cancelled')
                  + (SELECT COUNT(*) FROM recording_jobs WHERE status='cancelled')"
        ),
        0
    );
}

#[test]
fn group_preferences_default_cleanly_and_keep_existing_overlays() {
    use iptv_core::organization::{GroupPreference, Sort};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("generated-v25.db");
    build_old_file(&path, HISTORY.iter().find(|v| v.version == 25).unwrap());
    let mut store = Store::open(&path).unwrap();
    let group = "xt:live-group:1";
    let before = store.guide_group_preferences(SRC, group).unwrap();
    assert_eq!(before.sort, Sort::Name);
    assert!(!before.favorites_only);
    assert_eq!(before.epg_offset_minutes, 0);

    store
        .set_guide_group_preference(SRC, group, GroupPreference::FavoritesOnly(true))
        .unwrap();
    store
        .set_guide_group_preference(SRC, group, GroupPreference::ExternalPlayer(true))
        .unwrap();
    store
        .set_guide_group_preference(SRC, group, GroupPreference::EpgOffsetMinutes(-45))
        .unwrap();
    drop(store);

    let store = Store::open(&path).unwrap();
    let saved = store.guide_group_preferences(SRC, group).unwrap();
    assert_eq!(saved.sort, before.sort);
    assert!(saved.favorites_only && saved.external_player);
    assert_eq!(saved.epg_offset_minutes, -45);
    assert_eq!(
        store
            .channel_guide_offset_seconds(SRC, "xt:live:101")
            .unwrap(),
        -2700
    );
}

#[test]
fn custom_group_preferences_share_the_canonical_guide_id() {
    use iptv_core::organization::GroupPreference;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("generated-v25.db");
    build_old_file(&path, HISTORY.iter().find(|v| v.version == 25).unwrap());
    let mut store = Store::open(&path).unwrap();
    let custom = store.custom_groups(SRC).unwrap()[0].id;
    let canonical = format!("custom:{}", custom.0);
    let alias = format!("custom:0{}", custom.0);
    store
        .set_guide_group_preference(SRC, &alias, GroupPreference::ExternalPlayer(true))
        .unwrap();
    let saved = store.guide_group_preferences(SRC, &canonical).unwrap();
    assert!(saved.external_player);
    assert_eq!(store.guide_group_preferences(SRC, &alias).unwrap(), saved);
    assert!(store.delete_custom_group(custom).unwrap());
    assert!(
        !store
            .guide_group_preferences(SRC, &canonical)
            .unwrap()
            .external_player
    );
}

/// Schema 28 retires scheduled recording. A file from 0.1.8 (schema 27),
/// and one from a release before it (schema 26, skipping 0.1.8), opens with
/// nothing left to run: pending jobs are canceled and listed once, repeating
/// rules are disabled and listed, interrupted captures become partial
/// recordings instead of resuming, and retained media keeps its rows.
#[test]
fn upgrading_retires_scheduled_recordings_and_keeps_captures() {
    use iptv_core::recording::{JobId, Repeat, Status};

    for version in [26, 27] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("generated-v{version}.db"));
        build_old_file(
            &path,
            HISTORY.iter().find(|v| v.version == version).unwrap(),
        );
        let conn = rusqlite::Connection::open(&path).unwrap();
        // 41 completed with progress, 42 recording with a completion receipt
        // and 44 canceled come from the fixtures; rules 71 and 72 are enabled.
        conn.execute_batch(
            "INSERT INTO recording_jobs(id,source_id,channel_id,channel_name,title,starts_at,
                 ends_at,status,bytes,source_incarnation)
             SELECT 46,source_id,channel_id,channel_name,'Generated once',starts_at+86400,
                 ends_at+86400,'scheduled',0,source_incarnation FROM recording_jobs WHERE id=41;
             INSERT INTO recording_jobs(id,source_id,channel_id,channel_name,title,starts_at,
                 ends_at,status,bytes,message,source_incarnation)
             SELECT 47,source_id,channel_id,channel_name,'Generated resuming',starts_at+300,
                 ends_at+300,'scheduled',376,'Resuming interrupted recording',source_incarnation
             FROM recording_jobs WHERE id=41;
             INSERT INTO recording_jobs(id,source_id,channel_id,channel_name,title,starts_at,
                 ends_at,status,bytes,message,source_incarnation)
             SELECT 48,source_id,channel_id,channel_name,'Generated capturing',starts_at+600,
                 ends_at+600,'recording',0,'Starting recording',source_incarnation
             FROM recording_jobs WHERE id=41;
             INSERT INTO recording_jobs(id,source_id,channel_id,channel_name,title,starts_at,
                 ends_at,status,bytes,source_incarnation)
             SELECT 49,source_id,channel_id,channel_name,title,starts_at+172800,ends_at+172800,
                 'scheduled',0,source_incarnation FROM recording_jobs WHERE id=41;
             INSERT INTO recording_rule_jobs(job_id,rule_id,starts_at) VALUES(49,71,1800172800);",
        )
        .unwrap();
        drop(conn);

        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        let job = |store: &Store, id| store.recording_job(JobId(id)).unwrap().unwrap();
        for id in [46, 49] {
            let canceled = job(&store, id);
            assert_eq!(canceled.status, Status::Canceled, "v{version} job {id}");
            assert_eq!(canceled.message, "Scheduled recording was removed");
        }
        let resuming = job(&store, 47);
        assert_eq!((resuming.status, resuming.bytes), (Status::Partial, 376));
        assert_eq!(resuming.message, "Interrupted when the app closed");
        assert_eq!(job(&store, 48).status, Status::Partial);
        // A synced capture keeps its receipt, so recovery publishes it.
        assert_eq!(job(&store, 42).status, Status::Recording);
        assert!(store.recording_completion(JobId(42)).unwrap().is_some());
        let kept = job(&store, 41);
        assert_eq!((kept.status, kept.bytes), (Status::Completed, 18800));
        assert_eq!(
            store.recording_progress(JobId(41)).unwrap().position_ms,
            12000
        );
        assert_eq!(job(&store, 44).status, Status::Canceled);
        assert!(
            store
                .recording_jobs()
                .unwrap()
                .iter()
                .all(|job| job.status != Status::Scheduled)
        );
        assert!(store.recording_rules().unwrap().iter().all(|r| !r.enabled));

        let retired = store.retired_recording_schedules().unwrap();
        assert_eq!(
            retired
                .iter()
                .map(|s| (s.title.as_str(), s.repeat))
                .collect::<Vec<_>>(),
            [
                ("Generated programme", Repeat::Daily),
                ("Generated programme", Repeat::Weekly),
                ("Generated once", Repeat::Once),
            ],
            "v{version}: rules, then jobs no rule owns"
        );
        assert!(store.announce_retired_recordings().unwrap());
        assert!(!store.announce_retired_recordings().unwrap());
        drop(store);

        // The retirement ran once: a new Record now job survives reopening.
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.retired_recording_schedules().unwrap().len(), 3);
        assert!(!store.announce_retired_recordings().unwrap());
        store.dismiss_retired_recording_schedules().unwrap();
        assert!(store.retired_recording_schedules().unwrap().is_empty());
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO recording_jobs(id,source_id,channel_id,channel_name,title,starts_at,
                 ends_at,status,source_incarnation)
             SELECT 50,source_id,channel_id,channel_name,title,starts_at,ends_at,'scheduled',
                 source_incarnation FROM recording_jobs WHERE id=41",
            [],
        )
        .unwrap();
        drop((store, conn));
        let store = Store::open(&path).unwrap();
        assert_eq!(job(&store, 50).status, Status::Scheduled);
    }
}

/// A file with nothing scheduled shows no notice.
#[test]
fn upgrading_without_schedules_shows_no_retirement_notice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("generated-v10.db");
    build_old_file(&path, HISTORY.iter().find(|v| v.version == 10).unwrap());
    let mut store = Store::open(&path).unwrap();
    assert!(store.retired_recording_schedules().unwrap().is_empty());
    assert!(!store.announce_retired_recordings().unwrap());
}

/// A historical version: its schema snapshot and the data files to load
/// into it, in order.
struct Version {
    version: u32,
    data: &'static [&'static str],
}

const HISTORY: &[Version] = &[
    Version {
        version: 1,
        data: &["data_v1.sql"],
    },
    Version {
        version: 2,
        data: &["data_v1.sql", "data_v2.sql"],
    },
    Version {
        version: 3,
        data: &["data_v1.sql", "data_v2.sql", "data_v3.sql"],
    },
    Version {
        version: 4,
        data: &["data_v1.sql", "data_v2.sql", "data_v3.sql", "data_v4.sql"],
    },
    Version {
        version: 5,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
        ],
    },
    Version {
        version: 6,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
        ],
    },
    Version {
        version: 7,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
        ],
    },
    Version {
        version: 8,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
        ],
    },
    Version {
        version: 9,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
        ],
    },
    Version {
        version: 10,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
        ],
    },
    Version {
        version: 11,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
        ],
    },
    Version {
        version: 12,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
        ],
    },
    Version {
        version: 13,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
        ],
    },
    Version {
        version: 14,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
        ],
    },
    Version {
        version: 15,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
        ],
    },
    Version {
        version: 16,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
            "data_v16.sql",
        ],
    },
    Version {
        version: 17,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
            "data_v16.sql",
            "data_v17.sql",
        ],
    },
    Version {
        version: 18,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
            "data_v16.sql",
            "data_v17.sql",
            "data_v18.sql",
        ],
    },
    Version {
        version: 19,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
            "data_v16.sql",
            "data_v17.sql",
            "data_v18.sql",
            "data_v19.sql",
        ],
    },
    Version {
        version: 20,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
            "data_v16.sql",
            "data_v17.sql",
            "data_v18.sql",
            "data_v19.sql",
            "data_v20.sql",
        ],
    },
    Version {
        version: 21,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
            "data_v16.sql",
            "data_v17.sql",
            "data_v18.sql",
            "data_v19.sql",
            "data_v20.sql",
            "data_v21.sql",
        ],
    },
    Version {
        version: 22,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
            "data_v16.sql",
            "data_v17.sql",
            "data_v18.sql",
            "data_v19.sql",
            "data_v20.sql",
            "data_v21.sql",
            "data_v22.sql",
        ],
    },
    Version {
        version: 23,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
            "data_v16.sql",
            "data_v17.sql",
            "data_v18.sql",
            "data_v19.sql",
            "data_v20.sql",
            "data_v21.sql",
            "data_v22.sql",
            "data_v23.sql",
        ],
    },
    Version {
        version: 24,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
            "data_v16.sql",
            "data_v17.sql",
            "data_v18.sql",
            "data_v19.sql",
            "data_v20.sql",
            "data_v21.sql",
            "data_v22.sql",
            "data_v23.sql",
            "data_v24.sql",
        ],
    },
    Version {
        version: 25,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
            "data_v16.sql",
            "data_v17.sql",
            "data_v18.sql",
            "data_v19.sql",
            "data_v20.sql",
            "data_v21.sql",
            "data_v22.sql",
            "data_v23.sql",
            "data_v24.sql",
            "data_v25.sql",
        ],
    },
    Version {
        version: 26,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
            "data_v16.sql",
            "data_v17.sql",
            "data_v18.sql",
            "data_v19.sql",
            "data_v20.sql",
            "data_v21.sql",
            "data_v22.sql",
            "data_v23.sql",
            "data_v24.sql",
            "data_v25.sql",
            "data_v26.sql",
        ],
    },
    Version {
        version: 27,
        data: &[
            "data_v1.sql",
            "data_v2.sql",
            "data_v3.sql",
            "data_v4.sql",
            "data_v5.sql",
            "data_v6.sql",
            "data_v7.sql",
            "data_v8.sql",
            "data_v9.sql",
            "data_v10.sql",
            "data_v11.sql",
            "data_v12.sql",
            "data_v13.sql",
            "data_v14.sql",
            "data_v15.sql",
            "data_v16.sql",
            "data_v17.sql",
            "data_v18.sql",
            "data_v19.sql",
            "data_v20.sql",
            "data_v21.sql",
            "data_v22.sql",
            "data_v23.sql",
            "data_v24.sql",
            "data_v25.sql",
            "data_v26.sql",
            "data_v27.sql",
        ],
    },
];

fn seed_sql(version: u32, name: &str) -> String {
    let sql = fixture(name);
    if version >= 25 {
        sql.replace("favourites", "favorites")
            .replace("epg_programmes", "epg_programs")
            .replace("programme_count", "program_count")
            .replace("cancelled", "canceled")
    } else {
        sql
    }
}

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Writes a file exactly as a build at `v.version` left it.
fn build_old_file(path: &Path, v: &Version) {
    // Schema fixtures contain DDL, not ledger rows. Copy the identities a real
    // build records, rather than synthesizing names ordinary opening rejects.
    static NAMES: std::sync::OnceLock<Vec<(u32, String)>> = std::sync::OnceLock::new();
    let names = NAMES.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("canonical-migration-ledger.db");
        drop(Store::open(&fresh).unwrap());
        let conn = rusqlite::Connection::open(fresh).unwrap();
        conn.prepare("SELECT version,name FROM schema_migrations ORDER BY version")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    });
    // Building the fixture is not the code under test, so it need not be
    // durable: the connection is closed before the current build opens the
    // file. In the default rollback-journal mode every autocommit statement
    // creates, syncs and deletes a journal and syncs the database. That is
    // about 400 syncs per version, run 27 times by each test that walks
    // `HISTORY`. A sync costs microseconds on tmpfs but milliseconds on
    // Windows NTFS, where these tests took minutes. One transaction with
    // an in-memory journal and no syncs writes the same file. Both pragmas
    // belong to this connection only: the file keeps its rollback-journal
    // format, and `Store::open` still migrates it with its own WAL and
    // `synchronous` settings.
    let mut conn = rusqlite::Connection::open(path).unwrap();
    conn.pragma_update(None, "synchronous", "OFF").unwrap();
    let _mode: String = conn
        .pragma_update_and_check(None, "journal_mode", "MEMORY", |r| r.get(0))
        .unwrap();
    let conn = conn.transaction().unwrap();
    conn.execute_batch(&fixture(&format!("schema_v{}.sql", v.version)))
        .unwrap();
    for (version, name) in names.iter().take(v.version as usize) {
        conn.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, 0)",
            rusqlite::params![version, name],
        )
        .unwrap();
    }
    conn.pragma_update(None, "user_version", v.version).unwrap();
    for data in v.data {
        conn.execute_batch(&seed_sql(v.version, data)).unwrap();
    }
    conn.commit().unwrap();
}

fn fresh_schema() -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fresh.db");
    drop(Store::open(&path).unwrap());
    schema_dump(&rusqlite::Connection::open(&path).unwrap()).unwrap()
}

const SRC: SourceId = SourceId(1);

fn id(s: &str) -> ContentId {
    ContentId::from_stored(s)
}

/// Data every version has (v1 provider data).
fn check_provider_data(store: &Store) {
    let panel = store.source(SRC).unwrap().unwrap();
    match &panel.source.config {
        SourceConfig::Xtream(c) => {
            assert_eq!(c.server_url, "http://panel.example.com:8080");
            assert_eq!(c.username.expose(), "fixture-user");
            assert_eq!(c.password.expose(), "fixture-pass");
        }
        other => panic!("wrong kind {other:?}"),
    }
    let list = store.source(SourceId(2)).unwrap().unwrap();
    assert_eq!(list.source.kind(), SourceKind::M3u);
    assert_eq!(store.sources().unwrap().len(), 2);

    let status = store.import_status(SRC).unwrap();
    assert_eq!(status.last_success_at, Some(1_700_000_100));
    assert_eq!(status.counts.channels, 3);
    assert_eq!(status.last_error, Some(ImportErrorKind::Timeout));

    let all = store
        .load_channels(
            SRC,
            &ChannelQuery {
                include_hidden: true,
                order: ChannelOrder::Provider,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        all.iter()
            .map(|r| r.channel.name.as_str())
            .collect::<Vec<_>>(),
        ["Fixture News", "Fixture Sport", "Fixture Radio"]
    );
    let catch_up = all[0].channel.catch_up.as_ref().unwrap();
    // Written before correction_secs existed and while days was required.
    assert_eq!((catch_up.days, catch_up.correction_secs), (Some(2), 0));
    assert_eq!(all[2].channel.epg_shift_minutes, Some(60));
    assert!(all[2].channel.radio);
    let news = GroupId::from_stored("xt:live-group:1");
    let in_news = ChannelQuery {
        scope: ChannelScope::Group(news),
        include_hidden: true,
        ..Default::default()
    };
    assert_eq!(store.channel_count(SRC, &in_news).unwrap(), 2);
    assert_eq!(store.groups(SRC, None).unwrap().len(), 3);
    assert_eq!(
        store.load_vod(SRC, None, 0, None).unwrap()[0].year,
        Some(2019)
    );
    assert_eq!(
        store.load_series(SRC, None, 0, None).unwrap()[0].name,
        "Fixture Show"
    );
    assert_eq!(
        store.search_channels(Some(SRC), "sport", 5).unwrap().len(),
        1
    );
}

/// Overlays written by v2 builds (and what v3 data changes about them).
fn check_overlays(store: &Store, version: u32) {
    let visible = store.load_channels(SRC, &ChannelQuery::default()).unwrap();
    let expected: &[&str] = if version >= 3 {
        // v3 data adds a manual order of the all-channels list.
        &["Fixture Sport", "My News"]
    } else {
        &["My News", "Fixture Sport"]
    };
    assert_eq!(
        visible.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        expected
    );
    let sport = visible.iter().find(|r| r.name == "Fixture Sport").unwrap();
    assert_eq!(sport.number, Some(10));
    assert!(sport.blocked);
    let favorites = store
        .load_channels(
            SRC,
            &ChannelQuery {
                favorites_only: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        favorites
            .iter()
            .map(|r| r.channel.id.as_str())
            .collect::<Vec<_>>(),
        ["xt:live:102", "xt:live:101"]
    );
    let orphans = store.orphaned_overlays(Some(SRC)).unwrap();
    let gone = orphans
        .iter()
        .find(|o| o.channel_id == id("xt:live:999"))
        .unwrap();
    assert!(gone.favorite && gone.customized);
    assert_eq!(gone.ordered && gone.in_custom_group, version >= 3);
    assert_eq!(orphans.len(), if version >= 3 { 2 } else { 1 });
    assert_eq!(
        store.search_channels(Some(SRC), "my news", 5).unwrap()[0]
            .channel
            .id,
        id("xt:live:101")
    );
    let news = store.channel(SRC, &id("xt:live:101")).unwrap().unwrap();
    if version < 3 {
        assert_eq!(news.epg_override, None, "v3 columns default to automatic");
    }
}

/// Organization data written by v3 builds.
fn check_organization(store: &Store) {
    let news = store.channel(SRC, &id("xt:live:101")).unwrap().unwrap();
    assert_eq!(
        news.epg_override,
        Some(EpgOverride::Xmltv {
            epg_source: "epg-fixture".into(),
            epg_channel: "alt.news.example".into()
        })
    );
    let groups = store.custom_groups(SRC).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(
        (groups[0].name.as_str(), groups[0].channel_count),
        ("Evening", 2)
    );
    let members = store
        .load_channels(
            SRC,
            &ChannelQuery {
                scope: ChannelScope::Custom(groups[0].id),
                include_hidden: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        members
            .iter()
            .map(|r| r.channel.id.as_str())
            .collect::<Vec<_>>(),
        ["xt:live:103", "xt:live:101"]
    );
    let recent = store.recent(Some(SRC), None, 10).unwrap();
    assert_eq!(
        recent
            .iter()
            .map(|h| (h.item_id.as_str(), h.watched_at))
            .collect::<Vec<_>>(),
        [
            ("xt:live:101", 1_700_000_900),
            ("xt:movie:5", 1_700_000_800)
        ]
    );
    assert_eq!(recent[1].position_secs, Some(1200));
    assert_eq!(
        store
            .recent(None, Some(ContentKind::Live), 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn every_historical_version_migrates_with_its_data() {
    let fresh = fresh_schema();
    for v in HISTORY {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("v{}.db", v.version));
        build_old_file(&path, v);

        let store = Store::open(&path).unwrap_or_else(|e| panic!("v{}: {e}", v.version));
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        let migrated = schema_dump(&rusqlite::Connection::open(&path).unwrap()).unwrap();
        assert_eq!(
            migrated, fresh,
            "v{} migrated schema differs from fresh",
            v.version
        );
        check_provider_data(&store);
        if v.version >= 2 {
            check_overlays(&store, v.version);
        }
        if v.version >= 3 {
            check_organization(&store);
        }
        check_guide(&store, v.version);
        check_settings(&store, v.version);
        check_feed_results(&store, v.version);
        check_search_history(&store, v.version);
        check_recording_completion(&store, v.version);
        drop(store);

        // Opening again (every app start) changes nothing.
        let again = Store::open(&path).unwrap();
        check_provider_data(&again);
        if v.version >= 2 {
            check_overlays(&again, v.version);
        }
        if v.version >= 3 {
            check_organization(&again);
        }
        check_guide(&again, v.version);
        check_settings(&again, v.version);
        check_feed_results(&again, v.version);
        check_search_history(&again, v.version);
        check_recording_completion(&again, v.version);
    }
}

fn check_recording_completion(store: &Store, version: u32) {
    let receipt = store
        .recording_completion(iptv_core::recording::JobId(42))
        .unwrap();
    if version >= 20 {
        let receipt = receipt.expect("synced synthetic recording intent survives migration");
        assert_eq!(receipt.status, iptv_core::recording::Status::Completed);
        assert_eq!(receipt.bytes, 188);
        assert_eq!(receipt.digest, "a".repeat(64));
    } else {
        assert!(receipt.is_none());
    }
}

/// Recent searches (v9 builds on). Before v9 they were a file the app
/// imports on its own, so an upgraded store starts with none.
fn check_search_history(store: &Store, version: u32) {
    let expected: &[&str] = if version >= 9 {
        &["fixture sport", "Fixture News"]
    } else {
        &[]
    };
    assert_eq!(store.search_history().unwrap(), expected);
}

/// The shown source (v5 builds on), and each source's guide sources: a v5
/// build's guide address, or the typed feed a build before v5 kept with
/// the stored guide, becomes the first; the provider's stored feeds follow
/// (v6 builds wrote them so).
fn check_settings(store: &Store, version: u32) {
    let expected_active = (version >= 5).then_some(SourceId(2));
    assert_eq!(store.active_source().unwrap(), expected_active);
    let sources = store.guide_sources(SRC).unwrap();
    let other = store.guide_sources(SourceId(2)).unwrap();
    if version < 4 {
        assert_eq!((sources, other), (vec![], vec![]));
        return;
    }
    let typed = if version == 4 {
        "http://panel.example.com:8080/xmltv.php?username=fixture-user&password=fixture-pass"
    } else {
        "http://epg.example.com/typed.xml"
    };
    assert_eq!(
        sources,
        [
            GuideSource {
                feed: iptv_epg::FeedId(1),
                provider_index: None,
                url: SecretUrl::new(typed),
                name: None,
                enabled: true,
                shift_minutes: 0,
            },
            GuideSource {
                feed: iptv_epg::FeedId(2),
                provider_index: Some(0),
                url: SecretUrl::new("http://epg.example.com/extra.xml.gz"),
                name: None,
                enabled: true,
                shift_minutes: -30,
            },
        ],
        "v{version}"
    );
    if version >= 6 {
        assert_eq!(
            other,
            [GuideSource {
                feed: iptv_epg::FeedId(1001),
                provider_index: None,
                url: SecretUrl::new("http://alt.example.com/guide.xml"),
                name: Some("Fixture Alt".into()),
                enabled: false,
                shift_minutes: -60,
            }]
        );
    } else {
        assert_eq!(other, []);
    }
}

/// How each stored feed's last download went (from the stored feeds for
/// files before v6), and the channels the feeds list.
fn check_feed_results(store: &Store, version: u32) {
    let results = store.epg_feed_results(SRC).unwrap();
    let other = store.epg_feed_results(SourceId(2)).unwrap();
    if version < 4 {
        assert_eq!((results, other), (vec![], vec![]));
        return;
    }
    let loaded = |feed, channels, programs, listed| FeedResult {
        feed: iptv_epg::FeedId(feed),
        attempted_at: 1_700_001_000,
        error: None,
        channel_count: channels,
        program_count: programs,
        listed_count: listed,
    };
    let listed = if version >= 6 { 2 } else { 0 };
    assert_eq!(results, [loaded(1, 1, 3, 0), loaded(2, 1, 1, listed)]);
    if version >= 6 {
        assert_eq!(
            other,
            [FeedResult {
                feed: iptv_epg::FeedId(1001),
                attempted_at: 1_700_001_100,
                error: Some(ImportErrorKind::Timeout),
                channel_count: 0,
                program_count: 0,
                listed_count: 0,
            }]
        );
    } else {
        assert_eq!(other, []);
    }
    let channel = |feed, id: &str, name: Option<&str>, assigned| GuideChannel {
        feed: iptv_epg::FeedId(feed),
        epg_channel: id.into(),
        name: name.map(Into::into),
        assigned,
    };
    let mut expected = vec![channel(1, "news.example", None, true)];
    if version >= 6 {
        expected.push(channel(2, "spare.example", Some("Fixture Spare"), false));
        expected.push(channel(2, "sport.example", Some("Fixture Sport"), true));
    } else {
        expected.push(channel(2, "sport.example", None, true));
    }
    assert_eq!(store.guide_channels(SRC).unwrap(), expected, "v{version}");
}

/// Guide data written by v4 builds; none before.
fn check_guide(store: &Store, version: u32) {
    let guide = store.epg_guide(SRC).unwrap();
    if version < 4 {
        assert_eq!(guide, None);
        assert_eq!(store.epg_guide(SourceId(2)).unwrap(), None);
        return;
    }
    let guide = guide.unwrap();
    assert_eq!(guide.replaced_at, Some(1_700_001_000));
    assert_eq!(guide.match_fingerprint, Some(4242));
    assert_eq!(
        (guide.bound_channels, guide.ends_at),
        (3, Some(1_700_014_400))
    );
    assert_eq!(guide.last_error, None);
    let configs = guide.configs();
    assert_eq!(configs.len(), 2);
    assert_eq!(configs[0].priority, 2, "highest priority first");
    assert!(configs[0].url.expose().contains("xmltv.php"));
    assert_eq!(configs[0].store.past_days_to_keep, Some(2));
    assert_eq!(configs[1].user_agent.as_deref(), Some("Fixture/1"));
    assert_eq!(
        (
            configs[1].time.assumed_offset_minutes,
            configs[1].time.shift_minutes
        ),
        (60, -30)
    );
    assert!(!configs[1].store.store_descriptions);
    assert_eq!(
        guide.feeds[0].meta.etag.as_deref(),
        Some("\"fixture-etag\"")
    );
    assert_eq!(guide.feeds[0].program_count, 3);
    let state = guide.refresh_state();
    assert_eq!(state.feeds, configs);
    assert!(!state.last_attempt_failed);

    let failed = store.epg_guide(SourceId(2)).unwrap().unwrap();
    assert_eq!(failed.last_error, Some(ImportErrorKind::Timeout));
    assert_eq!(failed.replaced_at, None);
    assert!(failed.feeds.is_empty());

    let g = store
        .load_epg(SRC, &EpgWindow::from(1_700_000_000, 0))
        .unwrap();
    assert_eq!(g.bound_count(), 3);
    let news = ChannelKey::new("xt:live:101");
    let titles: Vec<String> = g
        .programs(&news, 0, i64::MAX)
        .map(|p| p.title().to_owned())
        .collect();
    assert_eq!(
        titles,
        ["Morning Fixture", "Fixture Midday", "Fixture Evening"]
    );
    let midday = g.program_at(&news, 1_700_004_000).unwrap();
    assert_eq!(midday.season_episode(), (Some(1), Some(4)));
    assert_eq!(
        (midday.sub_title(), midday.rating()),
        (Some("Part two"), Some("12"))
    );
    assert_eq!(midday.desc(), None, "descriptions load on demand");
    // The radio channel shares the news EPG channel (an override).
    let radio = ChannelKey::new("xt:live:103");
    assert_eq!(
        g.program_at(&radio, 1_700_000_100).unwrap().title(),
        "Morning Fixture"
    );
    assert_eq!(
        store
            .epg_description(SRC, &radio, 1_700_003_600)
            .unwrap()
            .as_deref(),
        Some("An invented midday programme.")
    );
    let sport = ChannelKey::new("xt:live:102");
    assert_eq!(
        g.program_at(&sport, 1_700_000_100).unwrap().icon(),
        Some("http://img.example.com/m.png")
    );
    // A part of it (the app's narrow window, or an older day paged in):
    // only what overlaps, read back as settled slots.
    let part = store
        .load_epg(
            SRC,
            &EpgWindow {
                from: 1_700_004_000,
                to: 1_700_004_001,
                ..EpgWindow::from(0, 0)
            },
        )
        .unwrap();
    let titles: Vec<String> = part
        .programs(&radio, 0, i64::MAX)
        .map(|p| p.title().to_owned())
        .collect();
    assert_eq!(titles, ["Fixture Midday"], "v{version}");
    assert_eq!(
        part.program_at(&radio, 1_700_004_000)
            .unwrap()
            .season_episode(),
        (Some(1), Some(4))
    );
}

/// A small guide matched against the fixture channels.
fn tiny_guide() -> (GuideStore, EpgSourceConfig) {
    let cfg = EpgSourceConfig::new(
        iptv_epg::FeedId(1),
        SecretUrl::new("http://epg.example.com/guide.xml"),
    );
    let mut b = SourceBuilder::new(cfg.id, 0, cfg.store, 1_700_000_000);
    let xml = format!(
        "<tv><channel id=\"news.example\"><display-name>Fixture News</display-name></channel>{}</tv>",
        "<programme channel=\"news.example\" start=\"20231114221320 +0000\" stop=\"20231114231320 +0000\"><title>After Upgrade</title><desc>New data.</desc></programme>"
    );
    let parsed = iptv_epg::parse_xmltv(xml.as_bytes(), Default::default()).unwrap();
    parsed.channels.into_iter().for_each(|c| b.push_channel(c));
    parsed.programs.into_iter().for_each(|p| b.push_program(p));
    let src = b.finish();
    let mut index = MatchIndex::new();
    index.add_source(src.id(), src.priority(), src.channels());
    let key = ChannelKey::new("xt:live:101");
    let matched = index.match_channels(
        [PlaylistChannel {
            key: &key,
            tvg_id: Some("news.example"),
            name: "Fixture News",
            tvg_name: None,
        }],
        &Overrides::new(),
    );
    let mut guide = GuideStore::new();
    guide.replace_source(src);
    guide.set_matches(matched.matches);
    (guide, cfg)
}

/// The first guide refresh after upgrading (from any version) replaces the
/// stored guide and touches nothing else.
#[test]
fn first_guide_refresh_after_upgrading() {
    for v in HISTORY {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        build_old_file(&path, v);
        let mut store = Store::open(&path).unwrap();
        let (guide, cfg) = tiny_guide();
        let feeds = [EpgFeedMeta {
            config: cfg,
            fetched_at: 1_700_002_000,
            etag: None,
            last_modified: None,
            bytes: 10,
        }];
        let counts = staged::replace_epg(&mut store, SRC, &guide, &feeds, 7).unwrap();
        assert_eq!((counts.channels, counts.programs), (1, 1));
        let stored = store.epg_guide(SRC).unwrap().unwrap();
        assert_eq!((stored.feeds.len(), stored.match_fingerprint), (1, Some(7)));
        let g = store
            .load_epg(SRC, &EpgWindow::from(1_700_000_000, 1))
            .unwrap();
        assert_eq!(g.bound_count(), 1, "v{}: old bindings replaced", v.version);
        check_provider_data(&store);
        if v.version >= 2 {
            check_overlays(&store, v.version);
        }
        if v.version >= 3 {
            check_organization(&store);
        }
        if v.version >= 4 {
            // The other source's failed attempt is untouched.
            let other = store.epg_guide(SourceId(2)).unwrap().unwrap();
            assert_eq!(other.last_error, Some(ImportErrorKind::Timeout));
        }
    }
}

#[test]
fn overlays_survive_the_first_refresh_after_upgrading() {
    for v in HISTORY.iter().filter(|v| v.version >= 2) {
        first_refresh_after_upgrading(v);
    }
}

fn first_refresh_after_upgrading(v: &Version) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    build_old_file(&path, v);
    let mut store = Store::open(&path).unwrap();
    let mut catalog = store
        .load_channels(
            SRC,
            &ChannelQuery {
                include_hidden: true,
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .map(|r| r.channel)
        .collect::<Vec<_>>();
    // The provider now also lists the channel that was gone.
    let mut back = catalog[0].clone();
    back.id = id("xt:live:999");
    back.name = "Returned".into();
    catalog.push(back);
    let groups = store.groups(SRC, None).unwrap();
    store
        .replace_catalog(SRC, &groups, &catalog, &[], &[])
        .unwrap();
    check_overlays_after_return(&store);
    if v.version >= 3 {
        let evening = &store.custom_groups(SRC).unwrap()[0];
        assert_eq!(
            evening.channel_count, 3,
            "the returning member counts again"
        );
        let order = store.load_channels(SRC, &ChannelQuery::default()).unwrap();
        assert_eq!(
            order
                .iter()
                .map(|r| r.channel.id.as_str())
                .collect::<Vec<_>>(),
            ["xt:live:102", "xt:live:999", "xt:live:101"],
            "the returning channel regains its manual place"
        );
    }
}

fn check_overlays_after_return(store: &Store) {
    assert!(
        store
            .orphaned_overlays(Some(SRC))
            .unwrap()
            .iter()
            .all(|o| o.channel_id != id("xt:live:999"))
    );
    let returned = store.channel(SRC, &id("xt:live:999")).unwrap().unwrap();
    assert_eq!(returned.name, "Gone For Now");
    assert!(returned.favorite);
    let favorites = store
        .load_channels(
            SRC,
            &ChannelQuery {
                favorites_only: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        favorites
            .iter()
            .map(|r| r.channel.id.as_str())
            .collect::<Vec<_>>(),
        ["xt:live:102", "xt:live:999", "xt:live:101"]
    );
}

/// After upgrading from any version, the guide sources screen can save a
/// source list and the shown source, and deleting a source takes them
/// along.
#[test]
fn playlist_settings_after_upgrading() {
    for v in HISTORY {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        build_old_file(&path, v);
        let mut store = Store::open(&path).unwrap();
        let mut sources = store.guide_sources(SRC).unwrap();
        sources.insert(
            0,
            GuideSource {
                feed: iptv_epg::FeedId(1002),
                provider_index: None,
                url: SecretUrl::new("http://epg.example.com/changed.xml"),
                name: Some("Changed".into()),
                enabled: true,
                shift_minutes: 120,
            },
        );
        store.set_guide_sources(SRC, &sources).unwrap();
        store.set_active_source(SRC).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.guide_sources(SRC).unwrap(), sources, "v{}", v.version);
        assert_eq!(store.active_source().unwrap(), Some(SRC));
        // The other source still has what it had.
        check_guide(&store, v.version);
        assert!(store.delete_source(SRC).unwrap());
        assert_eq!(store.active_source().unwrap(), None);
        assert!(store.guide_sources(SRC).is_err());
        assert_eq!(store.sources().unwrap().len(), 1);
        assert!(store.source(SourceId(2)).unwrap().is_some());
    }
}
