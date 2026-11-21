//! Independent application consumer: generated catalogue/guide fixtures and
//! public runtime queries, without an NSH root, renderer or native key broker.
use crate::common;
use crate::support;

use iptv_epg::{
    ChannelKey, EpgRef, FeedId, GuideStore, MatchKind, SourceBuilder, StoreOptions, XmltvChannel,
    XmltvProgram,
};
use iptv_import::{EpgImport, EpgSourceConfig, GuideSink, ListedChannel};
use iptv_library::inspection::*;
use iptv_providers::{ContentId, SecretUrl};
use iptv_runtime::{Ports, Runtime, inspection::InspectionControl, model::DatabaseKey};
use iptv_store_sqlite::{
    EpgFeedMeta, EpgOverride, EpgStage, GuideSource, SourceId, SourcePolicy, Store,
};
use std::{cell::Cell, rc::Rc, sync::mpsc, time::Duration};
use support::block_on;

const NOW: i64 = 1_794_657_600; // 2026-11-14T12:00:00Z.
const HOUR: i64 = 3600;
const KEY: [u8; 32] = [0x71; 32];
const FEED: FeedId = FeedId(1);

struct Fixture {
    store: Store,
    sources: [SourceId; 2],
    catalog: common::Catalog,
    owner: Rc<Runtime>,
    _directory: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open_encrypted(
            directory.path().join("iptv.sqlite"),
            &DatabaseKey::from_bytes(KEY),
        )
        .unwrap();
        store.set_clock(|| NOW);
        let sources = ["Synthetic source A", "Synthetic source B"]
            .map(|name| store.add_source(&common::m3u_source(name)).unwrap());
        let mut catalog = common::catalog(4, 1, "synthetic-fixture-key");
        for (i, name) in [
            "Synthetic Amber",
            "Synthetic Amber +1",
            "Synthetic Amber East",
            "Synthetic Birch",
        ]
        .into_iter()
        .enumerate()
        {
            let channel = &mut catalog.channels[i];
            channel.id = ContentId::from_stored(format!("synthetic-{i:02}"));
            channel.name = name.into();
            channel.epg_name = None;
            channel.epg_id = Some(format!("synthetic.{i}"));
        }
        for (source, label) in sources.into_iter().zip(["A", "B"]) {
            common::replace(&mut store, source, &catalog);
            store.set_guide_sources(source, &[feed()]).unwrap();
            install_guide(&mut store, source, label, false);
        }
        let owner = Rc::new(Runtime::new(Ports::new(directory.path().into(), |_| {
            Ok(DatabaseKey::from_bytes(KEY))
        })));
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        Self {
            _directory: directory,
            store,
            sources,
            catalog,
            owner,
        }
    }
    fn channel(&self, source: SourceId, i: usize) -> ChannelRef {
        ChannelRef {
            source,
            content: self.catalog.channels[i].id.clone(),
        }
    }
    fn channels(&self, source: SourceId) -> Page<ChannelInfo> {
        block_on(
            self.owner
                .query_channels(Query::source(source), InspectionControl::default()),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self.owner.drain_services());
    }
}
fn feed() -> GuideSource {
    GuideSource {
        feed: FEED,
        provider_index: None,
        url: SecretUrl::new("https://synthetic.invalid/guide.xml"),
        name: Some("Synthetic primary feed".into()),
        enabled: true,
        shift_minutes: 0,
    }
}
fn window() -> TimeWindow {
    TimeWindow {
        from: NOW,
        to: NOW + 2 * HOUR,
    }
}
fn install_guide(store: &mut Store, source: SourceId, label: &str, ambiguous: bool) {
    let mut builder = SourceBuilder::new(FEED, 1, StoreOptions::default(), NOW);
    for (id, name, shift) in [
        ("synthetic.0", "Synthetic Amber", 0),
        ("synthetic.1", "Synthetic Amber +1", HOUR),
        ("synthetic.2", "Synthetic Amber East", -4 * HOUR),
        ("amber.shadow", "Synthetic Amber", 0),
    ] {
        builder.push_channel(XmltvChannel {
            id: id.into(),
            display_names: vec![name.into()],
            icon: None,
        });
        for (i, (from, to)) in [(0, HOUR / 2), (HOUR / 2, HOUR), (HOUR, 2 * HOUR)]
            .into_iter()
            .enumerate()
        {
            builder.push_program(XmltvProgram {
                channel: id.into(),
                start: NOW + from + shift,
                stop: Some(NOW + to + shift),
                title: format!("Synthetic {label} Segment {:02}", i + 1),
                ..XmltvProgram::default()
            });
        }
    }
    let mut guide = GuideStore::new();
    guide.replace_source(builder.finish());
    guide.set_matches(
        (0..3)
            .map(|i| {
                (
                    ChannelKey::new(format!("synthetic-{i:02}")),
                    vec![EpgRef {
                        source: FEED,
                        epg_id: format!("synthetic.{i}"),
                        kind: if i == 0 && ambiguous {
                            MatchKind::Name
                        } else {
                            MatchKind::TvgId
                        },
                    }],
                )
            })
            .collect(),
    );
    let meta = EpgFeedMeta {
        config: EpgSourceConfig::new(FEED, feed().url),
        fetched_at: NOW,
        etag: None,
        last_modified: None,
        bytes: 512,
    };
    let scratch = tempfile::tempdir().unwrap();
    let mut stage = EpgStage::create_in_store(store, scratch.path()).unwrap();
    let bindings: Vec<_> = guide
        .bindings()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let mut listed = Vec::new();
    for src in guide.sources() {
        stage
            .begin_feed(src.id(), src.priority(), StoreOptions::default(), NOW)
            .unwrap();
        for (_, binding) in &bindings {
            if let Some(schedule) = src.schedule(&binding.epg_id) {
                for program in schedule.iter() {
                    stage.program(&binding.epg_id, &program.data()).unwrap();
                }
            }
        }
        stage.finish_feed().unwrap();
        listed.push((
            src.id(),
            src.channels()
                .iter()
                .map(|c| ListedChannel {
                    id: c.id.clone(),
                    name: c.display_names.first().cloned(),
                })
                .collect(),
        ));
    }
    let import = EpgImport {
        bindings,
        listed,
        ..EpgImport::default()
    };
    store
        .replace_epg(source, &mut stage, &import, &[meta], 0)
        .unwrap();
}

#[test]
fn source_queries_leave_shown_schedule_browser_and_player_events_untouched() {
    let f = Fixture::new();
    let [a, b] = f.sources;
    iptv_runtime::stored::note_shown(&f.owner, Some(a));
    let observed = Rc::new(Cell::new(0));
    let count = observed.clone();
    f.owner.bind(move |_| count.set(count.get() + 1));
    f.owner
        .bind_browser(|| panic!("a query consulted browser/player observations"));
    let schedule = f.owner.schedule();
    let result = f.channels(b);
    assert_eq!(result.items.len(), 4);
    assert!(result.items.iter().all(|c| c.key.source == b));
    assert_eq!(f.owner.shown_source(), Some(a));
    assert!(Rc::ptr_eq(&schedule, &f.owner.schedule()));
    assert_eq!(
        observed.get(),
        0,
        "read query emitted a presentation/player event"
    );
    let programs = block_on(f.owner.query_programs(
        Query::source(b),
        window(),
        Some(f.channel(b, 0)),
        InspectionControl::default(),
    ))
    .unwrap();
    assert_eq!(programs.items.len(), 3);
    assert!(
        programs
            .items
            .iter()
            .all(|p| p.channel.source == b && p.title.starts_with("Synthetic B"))
    );
    assert_eq!(f.owner.shown_source(), Some(a));
    assert_eq!(observed.get(), 0);
}

#[test]
fn equal_content_ids_remain_scoped_and_disabled_sources_are_explicit() {
    let mut f = Fixture::new();
    let [a, b] = f.sources;
    let mut query = Query::source(a);
    query.scope = SourceScope::AllEnabled;
    let all = block_on(
        f.owner
            .query_channels(query.clone(), InspectionControl::default()),
    )
    .unwrap();
    assert_eq!(all.items.len(), 8);
    assert_eq!(
        all.items
            .iter()
            .filter(|c| c.key.content == f.channel(a, 0).content)
            .count(),
        2
    );
    f.store
        .set_source_policy(
            b,
            SourcePolicy {
                enabled: false,
                ..SourcePolicy::default()
            },
        )
        .unwrap();
    assert_eq!(
        block_on(
            f.owner
                .inspect_channel(f.channel(b, 0), InspectionControl::default())
        ),
        Err(InspectionError::SourceDisabled(b))
    );
    let enabled = block_on(f.owner.query_channels(query, InspectionControl::default())).unwrap();
    assert_eq!(enabled.items.len(), 4);
    assert!(enabled.items.iter().all(|c| c.key.source == a));
}

#[test]
fn empty_missing_failed_and_invalid_reads_are_distinct() {
    let f = Fixture::new();
    let mut empty = Query::source(f.sources[0]);
    empty.text = "synthetic missing token".into();
    assert!(
        block_on(f.owner.query_channels(empty, InspectionControl::default()))
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        block_on(
            f.owner
                .query_channels(Query::source(SourceId(9999)), InspectionControl::default())
        ),
        Err(InspectionError::SourceMissing(SourceId(9999)))
    );
    let mut invalid = Query::source(f.sources[0]);
    invalid.limit = 0;
    assert_eq!(
        block_on(
            f.owner
                .query_channels(invalid, InspectionControl::default())
        ),
        Err(InspectionError::InvalidRequest)
    );
    let failed_dir = tempfile::tempdir().unwrap();
    let failed = Runtime::new(Ports::new(failed_dir.path().into(), |_| {
        Err(iptv_runtime::Error::unavailable("synthetic broker failure"))
    }));
    failed.start_services();
    assert_eq!(
        block_on(failed.query_channels(Query::source(SourceId(1)), InspectionControl::default())),
        Err(InspectionError::Storage)
    );
    assert!(failed.drain_services());
}

#[test]
fn semantic_channel_pages_survive_physical_reimport_and_reordering() {
    let mut f = Fixture::new();
    let source = f.sources[1];
    let mut query = Query::source(source);
    query.limit = 1;
    let first = block_on(
        f.owner
            .query_channels(query.clone(), InspectionControl::default()),
    )
    .unwrap();
    let mut actual = vec![first.items[0].key.content.clone()];
    query.cursor = first.next;
    f.catalog.channels.reverse();
    common::replace(&mut f.store, source, &f.catalog);
    while query.cursor.is_some() {
        let page = block_on(
            f.owner
                .query_channels(query.clone(), InspectionControl::default()),
        )
        .unwrap();
        actual.extend(page.items.into_iter().map(|c| c.key.content));
        query.cursor = page.next;
    }
    let mut expected: Vec<_> = f.catalog.channels.iter().map(|c| c.id.clone()).collect();
    expected.sort();
    assert_eq!(actual, expected);
}

#[test]
fn cursors_refuse_other_owner_scope_filter_window_and_changed_catalog() {
    let mut f = Fixture::new();
    let [a, b] = f.sources;
    let mut query = Query::source(a);
    query.limit = 1;
    query.cursor = block_on(
        f.owner
            .query_channels(query.clone(), InspectionControl::default()),
    )
    .unwrap()
    .next;
    let mut changed = query.clone();
    changed.text = "Synthetic".into();
    assert_eq!(
        block_on(
            f.owner
                .query_channels(changed, InspectionControl::default())
        ),
        Err(InspectionError::StaleCursor)
    );
    let mut other = query.clone();
    other.scope = SourceScope::Source(b);
    assert_eq!(
        block_on(f.owner.query_channels(other, InspectionControl::default())),
        Err(InspectionError::StaleCursor)
    );
    let second = Fixture::new();
    assert_eq!(
        block_on(
            second
                .owner
                .query_channels(query.clone(), InspectionControl::default())
        ),
        Err(InspectionError::StaleCursor)
    );
    let mut audit = Query::source(a);
    audit.limit = 1;
    audit.cursor = block_on(f.owner.epg_audit(
        audit.clone(),
        window(),
        InspectionControl::default(),
    ))
    .unwrap()
    .next;
    assert_eq!(
        block_on(f.owner.epg_audit(
            audit,
            TimeWindow {
                from: NOW + HOUR,
                to: NOW + 3 * HOUR
            },
            InspectionControl::default()
        )),
        Err(InspectionError::StaleCursor)
    );
    f.catalog.channels[3].name = "Synthetic changed catalogue".into();
    common::replace(&mut f.store, a, &f.catalog);
    assert_eq!(
        block_on(f.owner.query_channels(query, InspectionControl::default())),
        Err(InspectionError::StaleCursor)
    );
}

#[test]
fn a_channel_filter_with_multiple_sources_uses_the_complete_identity() {
    let f = Fixture::new();
    let [a, b] = f.sources;
    let mut query = Query::source(a);
    query.scope = SourceScope::Selected(vec![a, b]);
    let programs = block_on(f.owner.query_programs(
        query,
        window(),
        Some(f.channel(a, 0)),
        InspectionControl::default(),
    ))
    .unwrap();
    assert_eq!(programs.items.len(), 3);
    assert!(programs.items.iter().all(|p| p.channel.source == a));
    assert!(
        programs
            .items
            .iter()
            .all(|p| p.title.starts_with("Synthetic A"))
    );
}

#[test]
fn pre_canceled_and_expired_queries_do_not_poison_the_reader() {
    let f = Fixture::new();
    let control = InspectionControl::default();
    control.cancel.cancel();
    assert_eq!(
        block_on(f.owner.query_channels(Query::source(f.sources[0]), control)),
        Err(InspectionError::Canceled)
    );
    let expired = InspectionControl {
        timeout: Duration::ZERO,
        ..InspectionControl::default()
    };
    assert_eq!(
        block_on(f.owner.query_channels(Query::source(f.sources[0]), expired)),
        Err(InspectionError::Deadline)
    );
    assert_eq!(f.channels(f.sources[0]).items.len(), 4);
}

#[test]
fn closing_an_owner_with_a_held_open_rejects_its_late_query() {
    let directory = tempfile::tempdir().unwrap();
    let (entered, opening) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    let resume = std::sync::Mutex::new(resume);
    let owner = Runtime::new(Ports::new(directory.path().into(), move |_| {
        entered.send(()).unwrap();
        resume.lock().unwrap().recv().unwrap();
        Ok(DatabaseKey::from_bytes(KEY))
    }));
    owner.start_services();
    let query = owner.query_channels(Query::source(SourceId(1)), InspectionControl::default());
    opening.recv_timeout(Duration::from_secs(10)).unwrap();
    // Release the credential broker only after the query acknowledges closure.
    // This lets disposal drain the worker without a sleep or a drain timeout
    // deciding which result wins. The query owns no Runtime reference.
    let closing = std::time::Instant::now();
    let completed = std::thread::spawn(move || {
        let result = block_on(query);
        let elapsed = closing.elapsed();
        release.send(()).unwrap();
        (result, elapsed)
    });
    drop(owner);
    let (result, elapsed) = completed.join().unwrap();
    assert_eq!(result, Err(InspectionError::OwnerClosed));
    assert!(
        elapsed < iptv_runtime::inspection::QUERY_DEADLINE,
        "owner closure must wake the query before its normal deadline"
    );
}

#[test]
fn hidden_content_and_blocked_access_have_different_query_semantics() {
    let mut f = Fixture::new();
    let a = f.sources[0];
    f.store
        .set_hidden(a, &f.channel(a, 1).content, true)
        .unwrap();
    f.store
        .set_blocked(a, &f.channel(a, 2).content, true)
        .unwrap();
    protect_live(&f);
    let visible = f.channels(a);
    assert_eq!(visible.items.len(), 2);
    let mut query = Query::source(a);
    query.include_hidden = true;
    let inclusive = block_on(f.owner.query_channels(query, InspectionControl::default())).unwrap();
    assert_eq!(inclusive.items.len(), 3);
    assert!(
        block_on(
            f.owner
                .inspect_channel(f.channel(a, 1), InspectionControl::default())
        )
        .is_ok()
    );
    assert_eq!(
        block_on(
            f.owner
                .inspect_channel(f.channel(a, 2), InspectionControl::default())
        ),
        Err(InspectionError::PermissionDenied)
    );
    assert_eq!(
        block_on(f.owner.epg_candidates(
            f.channel(a, 2),
            Query::source(a),
            InspectionControl::default()
        )),
        Err(InspectionError::PermissionDenied)
    );
}

#[test]
fn overlapping_stored_programs_count_separately_from_union_coverage() {
    let f = Fixture::new();
    let source = f.sources[0];
    // Model a retained/legacy cache with overlapping physical programme rows.
    let raw = rusqlite::Connection::open(f._directory.path().join("iptv.sqlite")).unwrap();
    raw.pragma_update(None, "key", format!("x'{}'", "71".repeat(32)))
        .unwrap();
    assert_eq!(
        raw.execute(
            "INSERT INTO epg_programs(channel_id,start,stop,title)
         SELECT id,?2,?3,'Synthetic overlapping segment' FROM epg_channels
         WHERE source_id=?1 AND feed_id=1 AND epg_channel='synthetic.0'",
            rusqlite::params![source.0, NOW + 100, NOW + 200],
        )
        .unwrap(),
        1
    );
    let audit = block_on(f.owner.epg_audit(
        Query::source(source),
        window(),
        InspectionControl::default(),
    ))
    .unwrap();
    let amber = audit
        .items
        .iter()
        .find(|row| row.channel.key == f.channel(source, 0))
        .unwrap();
    assert_eq!(amber.program_count, 4);
    assert_eq!(amber.covered_seconds, 2 * HOUR as u64);
}

#[test]
fn audit_distinguishes_coverage_partial_stale_unmatched_disabled_and_input_failure() {
    let mut f = Fixture::new();
    let [a, b] = f.sources;
    let audit = block_on(f.owner.epg_audit(
        Query::source(a),
        window(),
        InspectionControl::default(),
    ))
    .unwrap();
    assert_eq!(
        audit.items.iter().map(|c| c.diagnosis).collect::<Vec<_>>(),
        [
            Diagnosis::Covered,
            Diagnosis::Partial,
            Diagnosis::Stale,
            Diagnosis::Unmatched
        ]
    );
    assert_eq!(audit.items[0].covered_seconds, 2 * HOUR as u64);
    assert_eq!(audit.items[1].covered_seconds, HOUR as u64);
    assert!(audit.items.iter().all(|c| !c.identity_verified));
    f.store
        .set_epg_override(a, &f.channel(a, 3).content, Some(&EpgOverride::Disabled))
        .unwrap();
    let no_guide = block_on(f.owner.epg_audit(
        Query::source(a),
        window(),
        InspectionControl::default(),
    ))
    .unwrap();
    assert_eq!(no_guide.items[3].diagnosis, Diagnosis::Disabled);
    f.store
        .record_epg_failure(b, iptv_store_sqlite::ImportErrorKind::Network)
        .unwrap();
    let failure = block_on(f.owner.epg_audit(
        Query::source(b),
        window(),
        InspectionControl::default(),
    ))
    .unwrap();
    assert!(
        failure
            .items
            .iter()
            .all(|c| c.diagnosis == Diagnosis::InputFailure)
    );
    assert_eq!(
        block_on(
            f.owner
                .epg_audit(Query::source(a), window(), InspectionControl::default())
        )
        .unwrap()
        .items[0]
            .diagnosis,
        Diagnosis::Covered
    );
}

#[test]
fn candidates_keep_ambiguous_and_timeshift_alternatives_and_half_open_programs() {
    let mut f = Fixture::new();
    let a = f.sources[0];
    install_guide(&mut f.store, a, "A", true);
    let candidates = block_on(f.owner.epg_candidates(
        f.channel(a, 0),
        Query::source(a),
        InspectionControl::default(),
    ))
    .unwrap();
    assert_eq!(candidates.items.len(), 4);
    assert!(
        candidates
            .items
            .iter()
            .any(|c| c.name.as_deref() == Some("Synthetic Amber +1"))
    );
    assert_eq!(
        candidates
            .items
            .iter()
            .filter(|c| c.name.as_deref() == Some("Synthetic Amber"))
            .count(),
        2
    );
    let audit = block_on(f.owner.epg_audit(
        Query::source(a),
        window(),
        InspectionControl::default(),
    ))
    .unwrap();
    assert_eq!(audit.items[0].diagnosis, Diagnosis::Ambiguous);
    let program = block_on(f.owner.query_programs(
        Query::source(a),
        TimeWindow {
            from: NOW + HOUR / 2,
            to: NOW + HOUR,
        },
        Some(f.channel(a, 0)),
        InspectionControl::default(),
    ))
    .unwrap();
    assert_eq!(program.items.len(), 1);
    assert_eq!(program.items[0].title, "Synthetic A Segment 02");
    let mut config = feed();
    config.url = SecretUrl::new("https://synthetic.invalid/replaced.xml");
    f.store.set_guide_sources(a, &[config]).unwrap();
    assert_ne!(
        f.store.feed_incarnation(a, FEED).unwrap().unwrap(),
        candidates.items[0].key.incarnation
    );
}

#[test]
fn completing_one_query_does_not_cancel_a_shared_callers_control() {
    let f = Fixture::new();
    let control = InspectionControl::default();
    let first = f
        .owner
        .query_channels(Query::source(f.sources[0]), control.clone());
    let second = f
        .owner
        .query_channels(Query::source(f.sources[1]), control.clone());
    assert_eq!(block_on(first).unwrap().items.len(), 4);
    assert!(
        !control.cancel.is_canceled(),
        "successful completion cancelled the caller's token"
    );
    assert_eq!(block_on(second).unwrap().items.len(), 4);
}

#[test]
fn audit_cursor_cannot_mix_different_guide_snapshots() {
    let mut f = Fixture::new();
    let source = f.sources[0];
    let mut query = Query::source(source);
    query.limit = 1;
    query.cursor = block_on(f.owner.epg_audit(
        query.clone(),
        window(),
        InspectionControl::default(),
    ))
    .unwrap()
    .next;
    install_guide(&mut f.store, source, "Updated", false);
    assert_eq!(
        block_on(
            f.owner
                .epg_audit(query, window(), InspectionControl::default())
        ),
        Err(InspectionError::StaleCursor)
    );
}

#[test]
#[cfg(feature = "test-support")]
#[ignore = "50 ms display budget runs alone through tools/ci/check-plan.py"]
fn a_hundred_thousand_channel_analysis_keeps_display_reads_responsive() {
    let mut f = Fixture::new();
    let source = f.sources[0];
    let mut large = common::catalog(100_000, 1000, "synthetic-scale-key");
    large
        .groups
        .retain(|g| g.kind == iptv_providers::model::ContentKind::Live);
    large.vod.clear();
    large.series.clear();
    assert_eq!((large.channels.len(), large.groups.len()), (100_000, 1000));
    common::replace(&mut f.store, source, &large);
    // Warm the display connection before measuring contention, as the existing
    // latency gates do; key derivation is a different startup workload.
    block_on(f.owner.read_library_metadata()).unwrap();
    let mut query = Query::source(source);
    query.limit = MAX_PAGE;
    let (entered, release) = f.owner.hold_inspection_scan();
    let analysis = f.owner.query_channels(query, InspectionControl::default());
    let (done, result) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || done.send(block_on(analysis)).unwrap());
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    // The query has executed SQLite scan instructions and holds its real
    // snapshot. Display reads must still finish before that scan is released.
    let read = std::time::Instant::now();
    block_on(f.owner.read_library_metadata()).unwrap();
    let mut maximum = read.elapsed();
    release.send(()).unwrap();
    let mut reads = 1;
    let started = std::time::Instant::now();
    let page = loop {
        match result.try_recv() {
            Ok(page) => break page.unwrap(),
            Err(mpsc::TryRecvError::Disconnected) => panic!("analysis worker disappeared"),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        let read = std::time::Instant::now();
        block_on(f.owner.read_library_metadata()).unwrap();
        maximum = maximum.max(read.elapsed());
        reads += 1;
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "analysis exceeded its deadline"
        );
    };
    worker.join().unwrap();
    assert_eq!(page.items.len(), MAX_PAGE as usize);
    assert!(page.next.is_some());
    assert!(
        reads > 0,
        "display reader never exercised alongside analysis"
    );
    assert!(
        maximum < Duration::from_millis(50),
        "display reader queued behind analysis: {maximum:?}"
    );
}

fn protect_live(fixture: &Fixture) {
    use iptv_core::parental::Pin;
    block_on(fixture.owner.parental_refresh()).unwrap();
    block_on(
        fixture
            .owner
            .parental_change_pin(None, Pin::parse("2468").unwrap()),
    )
    .unwrap();
    fixture
        .owner
        .set_setting("parental.live_enabled", iptv_settings::Value::Bool(true))
        .unwrap();
    fixture.owner.parental_lock();
}

#[test]
fn inspection_filters_modern_and_legacy_blocks_before_limit_and_honors_policy_off() {
    use iptv_core::parental::{Pin, Target};
    let mut f = Fixture::new();
    let source = f.sources[0];
    let legacy = f.channel(source, 0);
    let modern = f.channel(source, 2);
    f.store.set_blocked(source, &legacy.content, true).unwrap();
    f.store
        .set_parental_blocked(&Target::live(source.0, modern.content.as_str()), true)
        .unwrap();
    // Raw adapters fail closed, even without a runtime access context.
    let raw = f
        .store
        .inspection_channels(&Query::source(source))
        .unwrap()
        .unwrap();
    assert_eq!(
        raw.items
            .iter()
            .map(|channel| channel.key.content.as_str())
            .collect::<Vec<_>>(),
        ["synthetic-01", "synthetic-03"]
    );
    assert_eq!(
        f.store.inspection_channel(&modern).unwrap(),
        Err(InspectionError::PermissionDenied)
    );
    // Runtime protection disabled is explicit authority to include blocked rows.
    assert_eq!(f.channels(source).items.len(), 4);
    protect_live(&f);
    let mut query = Query::source(source);
    query.limit = 2;
    let page = block_on(f.owner.query_channels(query, InspectionControl::default())).unwrap();
    assert_eq!(
        page.items
            .iter()
            .map(|channel| channel.key.content.as_str())
            .collect::<Vec<_>>(),
        ["synthetic-01", "synthetic-03"]
    );
    assert!(
        page.next.is_none(),
        "filtering happens before both limit and lookahead"
    );
    for key in [&legacy, &modern] {
        assert_eq!(
            block_on(
                f.owner
                    .inspect_channel(key.clone(), InspectionControl::default())
            ),
            Err(InspectionError::PermissionDenied)
        );
        assert_eq!(
            block_on(f.owner.epg_candidates(
                key.clone(),
                Query::source(source),
                InspectionControl::default()
            )),
            Err(InspectionError::PermissionDenied)
        );
        assert_eq!(
            block_on(f.owner.query_programs(
                Query::source(source),
                window(),
                Some(key.clone()),
                InspectionControl::default()
            )),
            Err(InspectionError::PermissionDenied)
        );
    }
    let programs = block_on(f.owner.query_programs(
        Query::source(source),
        window(),
        None,
        InspectionControl::default(),
    ))
    .unwrap();
    assert!(
        programs
            .items
            .iter()
            .all(|program| program.channel.content.as_str() == "synthetic-01")
    );
    let audit = block_on(f.owner.epg_audit(
        Query::source(source),
        window(),
        InspectionControl::default(),
    ))
    .unwrap();
    assert_eq!(
        audit
            .items
            .iter()
            .map(|coverage| coverage.channel.key.content.as_str())
            .collect::<Vec<_>>(),
        ["synthetic-01", "synthetic-03"]
    );
    block_on(f.owner.parental_authorize(
        Target::live(source.0, legacy.content.as_str()),
        Pin::parse("2468").unwrap(),
    ))
    .unwrap();
    assert!(
        block_on(
            f.owner
                .inspect_channel(legacy, InspectionControl::default())
        )
        .is_ok(),
        "a valid runtime grant overrides the legacy adapter exclusion"
    );
    assert_eq!(f.channels(source).items.len(), 3);
    block_on(
        f.owner
            .parental_authorize(Target::settings(), Pin::parse("2468").unwrap()),
    )
    .unwrap();
    f.owner
        .set_setting("parental.live_enabled", iptv_settings::Value::Bool(false))
        .unwrap();
    assert_eq!(f.channels(source).items.len(), 4);
}

#[test]
fn inspection_adult_and_custom_group_access_matches_exact_source_grants() {
    use iptv_core::parental::{ContentKind, Pin, Target};
    let mut f = Fixture::new();
    let source = f.sources[0];
    f.catalog.groups[0].adult = true;
    common::replace(&mut f.store, source, &f.catalog);
    install_guide(&mut f.store, source, "A", false);
    protect_live(&f);
    assert!(f.channels(source).items.is_empty());
    let group = Target {
        source: source.0,
        kind: ContentKind::Group,
        id: f.catalog.groups[0].id.as_str().into(),
    };
    block_on(
        f.owner
            .parental_authorize(group.clone(), Pin::parse("2468").unwrap()),
    )
    .unwrap();
    assert_eq!(f.channels(source).items.len(), 4);
    f.owner.parental_lock();
    assert!(f.channels(source).items.is_empty());
    // Explicit adult-group allow survives provider metadata and needs no grant.
    f.store.set_parental_blocked(&group, false).unwrap();
    block_on(f.owner.parental_refresh()).unwrap();
    assert_eq!(f.channels(source).items.len(), 4);
    let custom = f
        .store
        .create_custom_group(source, "Synthetic protected collection")
        .unwrap();
    for channel in [1, 2] {
        f.store
            .add_to_custom_group(custom, &f.catalog.channels[channel].id)
            .unwrap();
    }
    let target = Target {
        source: source.0,
        kind: ContentKind::Group,
        id: format!("custom:{}", custom.0),
    };
    f.store.set_parental_blocked(&target, true).unwrap();
    f.store
        .set_blocked(source, &f.catalog.channels[2].id, true)
        .unwrap();
    block_on(f.owner.parental_refresh()).unwrap();
    assert_eq!(
        f.channels(source)
            .items
            .iter()
            .map(|channel| channel.key.content.as_str())
            .collect::<Vec<_>>(),
        ["synthetic-00", "synthetic-03"]
    );
    block_on(
        f.owner
            .parental_authorize(target, Pin::parse("2468").unwrap()),
    )
    .unwrap();
    assert!(
        block_on(
            f.owner
                .inspect_channel(f.channel(source, 1), InspectionControl::default())
        )
        .is_ok()
    );
    assert_eq!(
        block_on(
            f.owner
                .inspect_channel(f.channel(source, 2), InspectionControl::default())
        ),
        Err(InspectionError::PermissionDenied),
        "a parent grant does not override a direct child block"
    );
    assert_eq!(f.channels(source).items.len(), 3);
    // Identical content IDs on another source have independent membership.
    assert_eq!(f.channels(f.sources[1]).items.len(), 4);
}

#[test]
fn inspection_cursors_bind_permissions_but_ignore_unprotected_zapping() {
    use iptv_core::parental::{Pin, Target};
    let mut f = Fixture::new();
    let source = f.sources[0];
    let key = f.channel(source, 0);
    let target = Target::live(source.0, key.content.as_str());
    f.store.set_blocked(source, &key.content, true).unwrap();
    protect_live(&f);
    let mut query = Query::source(source);
    query.limit = 1;
    let first = block_on(
        f.owner
            .query_channels(query.clone(), InspectionControl::default()),
    )
    .unwrap();
    assert_eq!(first.items[0].key.content.as_str(), "synthetic-01");
    let mut continuation = query.clone();
    continuation.cursor = first.next.clone();
    f.owner
        .parental_content_left(&Target::live(source.0, "synthetic-unrelated"));
    let second = block_on(
        f.owner
            .query_channels(continuation.clone(), InspectionControl::default()),
    )
    .unwrap();
    assert_eq!(second.items[0].key.content.as_str(), "synthetic-02");
    block_on(
        f.owner
            .parental_authorize(target, Pin::parse("2468").unwrap()),
    )
    .unwrap();
    assert_eq!(
        block_on(
            f.owner
                .query_channels(continuation, InspectionControl::default())
        ),
        Err(InspectionError::StaleCursor)
    );
    let mut unlocked = query.clone();
    unlocked.cursor = block_on(f.owner.query_channels(query, InspectionControl::default()))
        .unwrap()
        .next;
    f.owner.parental_lock();
    assert_eq!(
        block_on(
            f.owner
                .query_channels(unlocked, InspectionControl::default())
        ),
        Err(InspectionError::StaleCursor)
    );
}

#[test]
fn inspection_final_publication_rechecks_relock_even_for_an_empty_known_channel_window() {
    use iptv_core::parental::{Pin, Target};
    let mut f = Fixture::new();
    let source = f.sources[0];
    let protected = f.channel(source, 0);
    let target = Target::live(source.0, protected.content.as_str());
    f.store
        .set_blocked(source, &protected.content, true)
        .unwrap();
    protect_live(&f);
    for empty_window in [false, true] {
        block_on(
            f.owner
                .parental_authorize(target.clone(), Pin::parse("2468").unwrap()),
        )
        .unwrap();
        if empty_window {
            let read = f.owner.query_programs(
                Query::source(source),
                TimeWindow {
                    from: NOW + 20 * HOUR,
                    to: NOW + 21 * HOUR,
                },
                Some(protected.clone()),
                InspectionControl::default(),
            );
            // Query construction queues its analysis job immediately. A later
            // query on that same FIFO lane acknowledges completion of the
            // protected SQL read before its future is polled for publication.
            block_on(
                f.owner
                    .inspect_channel(f.channel(source, 3), InspectionControl::default()),
            )
            .unwrap();
            f.owner.parental_lock();
            assert_eq!(block_on(read), Err(InspectionError::PermissionDenied));
        } else {
            let read = f
                .owner
                .inspect_channel(protected.clone(), InspectionControl::default());
            block_on(
                f.owner
                    .inspect_channel(f.channel(source, 3), InspectionControl::default()),
            )
            .unwrap();
            f.owner.parental_lock();
            assert_eq!(block_on(read), Err(InspectionError::PermissionDenied));
        }
    }
}

#[test]
fn unprotected_inspection_survives_zapping_but_not_policy_revision_changes() {
    use iptv_core::parental::{Pin, Target};
    let mut f = Fixture::new();
    let source = f.sources[0];
    f.store
        .set_blocked(source, &f.catalog.channels[0].id, true)
        .unwrap();
    protect_live(&f);
    let public = f.channel(source, 3);
    let read = f
        .owner
        .inspect_channel(public.clone(), InspectionControl::default());
    block_on(
        f.owner
            .inspect_channel(public.clone(), InspectionControl::default()),
    )
    .unwrap();
    f.owner
        .parental_content_left(&Target::live(source.0, "synthetic-unrelated"));
    assert!(block_on(read).is_ok());
    let read = f
        .owner
        .inspect_channel(public.clone(), InspectionControl::default());
    block_on(
        f.owner
            .inspect_channel(public, InspectionControl::default()),
    )
    .unwrap();
    block_on(
        f.owner
            .parental_authorize(Target::settings(), Pin::parse("2468").unwrap()),
    )
    .unwrap();
    f.owner
        .set_setting("parental.live_enabled", iptv_settings::Value::Bool(false))
        .unwrap();
    assert_eq!(block_on(read), Err(InspectionError::PermissionDenied));
    assert!(
        block_on(
            f.owner
                .inspect_channel(f.channel(source, 0), InspectionControl::default())
        )
        .is_ok()
    );
}
