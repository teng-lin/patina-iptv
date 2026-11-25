//! Guide repair authority across writers and ordinary store restarts.

use crate::common::{XT_PASSWORD, catalog, m3u_source, replace};
use iptv_epg::{FeedId, GuideStore, SourceBuilder, StoreOptions, XmltvChannel};
use iptv_import::EpgSourceConfig;
use iptv_library::inspection::{CandidateRef, ChannelRef, Query};
use iptv_providers::{ContentId, SecretUrl};
use iptv_store_sqlite::{
    DatabaseKey, EpgFeedMeta, EpgOverride, GuideRevisions, GuideSource, RepairBatch,
    RepairCandidate, RepairChoice, RepairCommit, RepairFeed, RepairMapping, RepairResult, SourceId,
    Store,
};

#[derive(Debug, PartialEq, Eq)]
struct GuideState {
    feeds: Vec<GuideSource>,
    mappings: [Option<EpgOverride>; 3],
    revisions: GuideRevisions,
    commit: Option<RepairCommit>,
}

struct Fixture {
    directory: tempfile::TempDir,
    source: SourceId,
    channels: [ContentId; 3],
}

impl Fixture {
    fn new() -> (Self, Store) {
        let directory = tempfile::tempdir().unwrap();
        let mut store = connect(&directory);
        let source = store
            .add_source(&m3u_source("Synthetic guide repair"))
            .unwrap();
        let library = catalog(3, 1, XT_PASSWORD);
        let channels = std::array::from_fn(|index| library.channels[index].id.clone());
        replace(&mut store, source, &library);
        store
            .set_guide_sources(source, &[feed(1), feed(2), feed(3)])
            .unwrap();
        (
            Self {
                directory,
                source,
                channels,
            },
            store,
        )
    }

    fn connect(&self) -> Store {
        connect(&self.directory)
    }

    fn preview(&self, store: &Store) -> RepairBatch {
        RepairBatch {
            source: self.source,
            source_incarnation: store.source_incarnation(self.source).unwrap(),
            mappings: self.channels[..2]
                .iter()
                .map(|channel| RepairMapping {
                    channel: channel.clone(),
                    before: None,
                    after: RepairChoice::Disabled,
                    replace_explicit: false,
                })
                .collect(),
            feeds: Vec::new(),
            additions: Vec::new(),
            candidates: Vec::new(),
            preview_expires_at: i64::MAX,
            order: None,
        }
    }

    fn mappings(&self, store: &Store) -> [Option<EpgOverride>; 3] {
        std::array::from_fn(|index| {
            store
                .repair_mapping(self.source, &self.channels[index])
                .unwrap()
                .expect("generated catalog channel")
        })
    }

    fn state(&self, store: &Store, journal: &str) -> GuideState {
        GuideState {
            feeds: store.guide_sources(self.source).unwrap(),
            mappings: self.mappings(store),
            revisions: store.guide_revisions(self.source).unwrap(),
            commit: store.guide_repair_commit(journal).unwrap(),
        }
    }

    fn preview_candidate(&self, store: &Store) -> (RepairBatch, CandidateRef) {
        let subject = ChannelRef {
            source: self.source,
            content: self.channels[0].clone(),
        };
        let candidates = store
            .inspection_candidates(&subject, &Query::source(self.source))
            .unwrap()
            .unwrap()
            .items;
        let [candidate] = candidates.as_slice() else {
            panic!("expected one cached guide candidate, got {candidates:?}");
        };
        let mut preview = self.preview(store);
        let key = &candidate.key;
        preview.mappings[0].after = RepairChoice::Existing {
            feed: key.feed,
            epg_channel: key.epg_channel.clone(),
        };
        preview.candidates.push(RepairCandidate {
            feed: key.feed,
            incarnation: key.incarnation.clone(),
            epg_channel: key.epg_channel.clone(),
            staged: false,
        });
        (preview, key.clone())
    }
}

fn connect(directory: &tempfile::TempDir) -> Store {
    let key = DatabaseKey::from_bytes([0x42; 32]);
    Store::open_encrypted(directory.path().join("synthetic.sqlite"), &key).unwrap()
}

fn feed(id: u32) -> GuideSource {
    GuideSource {
        feed: FeedId(id),
        provider_index: None,
        url: SecretUrl::new(format!("https://guide.example.test/feed-{id}.xml")),
        name: Some(format!("Synthetic feed {id}")),
        enabled: true,
        shift_minutes: 0,
    }
}

fn order(ids: [u32; 3]) -> Vec<FeedId> {
    ids.into_iter().map(FeedId).collect()
}

fn publish_guide(store: &mut Store, source: SourceId, listed_channel: &str) {
    const NOW: i64 = 1_800_000_000;
    let config = feed(1);
    let mut builder = SourceBuilder::new(config.feed, 0, StoreOptions::default(), NOW);
    builder.push_channel(XmltvChannel {
        id: listed_channel.into(),
        display_names: vec!["Synthetic guide channel".into()],
        icon: None,
    });
    let mut guide = GuideStore::new();
    guide.replace_source(builder.finish());
    let meta = EpgFeedMeta {
        config: EpgSourceConfig::new(config.feed, config.url),
        fetched_at: NOW,
        etag: None,
        last_modified: None,
        bytes: 10,
    };
    crate::staged::replace_epg(store, source, &guide, &[meta], 0).unwrap();
}

fn committed(result: RepairResult) -> RepairCommit {
    let RepairResult::Committed(commit) = result else {
        panic!("expected a committed repair, got {result:?}");
    };
    commit
}

#[test]
fn a_stale_reorder_preview_preserves_the_newer_order_and_can_be_refreshed() {
    let (fixture, mut repair_store) = Fixture::new();
    let source = fixture.source;
    let mut preview = fixture.preview(&repair_store);
    preview.order = Some((order([1, 2, 3]), order([3, 1, 2])));
    let journal = "generated-reorder";

    let mut other_writer = fixture.connect();
    other_writer
        .set_guide_sources(source, &[feed(2), feed(3), feed(1)])
        .unwrap();
    other_writer
        .set_epg_override(source, &fixture.channels[2], Some(&EpgOverride::Disabled))
        .unwrap();
    let newer = fixture.state(&other_writer, journal);
    drop(other_writer);

    assert_eq!(
        repair_store
            .apply_guide_repair(journal, "stale-preview", &preview)
            .unwrap(),
        RepairResult::Conflict(vec!["feed_order".into()])
    );
    drop(repair_store);

    let mut reopened = fixture.connect();
    assert_eq!(fixture.state(&reopened, journal), newer);

    preview.order = Some((order([2, 3, 1]), order([3, 1, 2])));
    let commit = committed(
        reopened
            .apply_guide_repair(journal, "refreshed-preview", &preview)
            .unwrap(),
    );
    drop(reopened);

    let reopened = fixture.connect();
    assert_eq!(
        reopened.guide_sources(source).unwrap(),
        [feed(3), feed(1), feed(2)]
    );
    assert_eq!(
        fixture.mappings(&reopened),
        [const { Some(EpgOverride::Disabled) }; 3]
    );
    assert_eq!(reopened.guide_repair_commit(journal).unwrap(), Some(commit));
}

#[test]
fn undo_conflicts_when_reordered_feeds_move_and_preserves_unrelated_later_edits() {
    let (fixture, mut repair_store) = Fixture::new();
    let source = fixture.source;
    let mut batch = fixture.preview(&repair_store);
    let shifted = GuideSource {
        shift_minutes: 60,
        ..feed(1)
    };
    batch.feeds.push(RepairFeed {
        before: Some(feed(1)),
        after: Some(shifted.clone()),
    });
    // Additions receive an ID from the store and begin at lowest precedence.
    batch.additions.push(feed(0));
    batch.order = Some((order([1, 2, 3]), order([3, 1, 2])));
    let journal = "generated-inverse";
    let commit = committed(
        repair_store
            .apply_guide_repair(journal, "generated-preview", &batch)
            .unwrap(),
    );
    assert_eq!(commit.added_feeds, [FeedId(4)]);
    let added = GuideSource {
        feed: FeedId(4),
        ..feed(0)
    };
    assert_eq!(
        repair_store.guide_sources(source).unwrap(),
        [feed(3), shifted.clone(), feed(2), added.clone()]
    );

    let unrelated = feed(9);
    let newer_mapping = EpgOverride::for_feed(unrelated.feed, "synthetic.later");
    let mut other_writer = fixture.connect();
    other_writer
        .set_guide_sources(
            source,
            &[
                shifted.clone(),
                unrelated.clone(),
                feed(3),
                feed(2),
                added.clone(),
            ],
        )
        .unwrap();
    other_writer
        .set_epg_override(source, &fixture.channels[2], Some(&newer_mapping))
        .unwrap();
    let newer = fixture.state(&other_writer, journal);
    assert_eq!(newer.commit, Some(commit));
    drop(other_writer);
    drop(repair_store);

    let mut reopened = fixture.connect();
    assert_eq!(
        reopened.undo_guide_repair(journal).unwrap(),
        RepairResult::Conflict(vec!["feed_order".into()])
    );
    drop(reopened);

    let mut reopened = fixture.connect();
    assert_eq!(fixture.state(&reopened, journal), newer);

    // Restore the committed relative order. The new feed stays in its slot.
    reopened
        .set_guide_sources(
            source,
            &[feed(3), unrelated.clone(), shifted, feed(2), added],
        )
        .unwrap();
    let inverse = committed(reopened.undo_guide_repair(journal).unwrap());
    assert!(inverse.undone);
    drop(reopened);

    let reopened = fixture.connect();
    assert_eq!(
        reopened.guide_sources(source).unwrap(),
        [feed(1), unrelated, feed(2), feed(3)]
    );
    assert_eq!(
        fixture.mappings(&reopened),
        [None, None, Some(newer_mapping)]
    );
    assert_eq!(
        reopened.guide_repair_commit(journal).unwrap(),
        Some(inverse)
    );
}

#[test]
fn refreshed_cache_with_the_same_feed_identity_rejects_a_removed_candidate() {
    let (fixture, mut repair_store) = Fixture::new();
    let source = fixture.source;
    publish_guide(&mut repair_store, source, "synthetic.original");
    let (preview, original) = fixture.preview_candidate(&repair_store);
    assert_eq!(original.epg_channel, "synthetic.original");
    let journal = "generated-candidate";

    let mut other_writer = fixture.connect();
    publish_guide(&mut other_writer, source, "synthetic.replacement");
    let (fresh, replacement) = fixture.preview_candidate(&other_writer);
    assert_eq!(replacement.epg_channel, "synthetic.replacement");
    assert_eq!(replacement.incarnation, original.incarnation);
    let newer = fixture.state(&other_writer, journal);
    drop(other_writer);

    assert_eq!(
        repair_store
            .apply_guide_repair(journal, "original-guide", &preview)
            .unwrap(),
        RepairResult::Conflict(vec![format!("candidate:{}", original.feed.0)])
    );
    drop(repair_store);

    let mut reopened = fixture.connect();
    assert_eq!(fixture.state(&reopened, journal), newer);
    let commit = committed(
        reopened
            .apply_guide_repair(journal, "replacement-guide", &fresh)
            .unwrap(),
    );
    drop(reopened);

    let reopened = fixture.connect();
    assert_eq!(
        fixture.mappings(&reopened),
        [
            Some(EpgOverride::for_feed(
                replacement.feed,
                replacement.epg_channel
            )),
            Some(EpgOverride::Disabled),
            None
        ]
    );
    assert_eq!(reopened.guide_repair_commit(journal).unwrap(), Some(commit));
}
