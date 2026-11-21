//! A replacement run in slices leaves exactly what the one-shot form
//! leaves, and an abandoned one leaves nothing.

use std::borrow::Cow;

use iptv_epg::{FeedId, MatchKind, ProgramData, StoreOptions};
use iptv_import::{CatalogRow, CatalogSink, EpgImport, EpgSourceConfig, GuideSink, ListedChannel};
use iptv_providers::SecretUrl;
use iptv_providers::model::{Channel, Group, M3uSourceConfig, Source, SourceConfig};

use super::Step;
use crate::{
    CatalogPublished, CatalogReplacement, CatalogStage, EpgFeedMeta, EpgReplacement, EpgStage,
    GuideSource, SourceId, StagePublication, Store,
};

const NOW: i64 = 1_800_000_000;
/// Row budgets: one row at a time, odd sizes across statement and batch
/// boundaries, and the browser's slice.
const BUDGETS: [u32; 4] = [1, 7, 61, 400];

fn store() -> (Store, SourceId) {
    let mut store = Store::open_in_memory().unwrap();
    store.set_clock(|| NOW);
    let source = store
        .add_source(&Source {
            name: "Synthetic".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://playlist.example.test/list.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    (store, source)
}

/// `channels` channels in four groups, named after `tag`.
fn catalog(channels: usize, tag: &str) -> (Vec<Group>, Vec<Channel>) {
    let mut text = String::from("#EXTM3U\n");
    for i in 0..channels {
        text.push_str(&format!(
            "#EXTINF:-1 tvg-id=\"ch{i}.example\" group-title=\"Group {}\",{tag} {i}\n\
             https://media.example.test/{tag}/{i}.ts\n",
            i % 4
        ));
    }
    let parsed = iptv_providers::m3u::parse_m3u(&text);
    (parsed.groups, parsed.channels.items)
}

/// Random identities (revisions, feed incarnations: 32 hex digits) differ
/// between two stores built the same way.
fn mask_random(text: String) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let run = bytes[i..]
            .iter()
            .take_while(|b| b.is_ascii_hexdigit())
            .count();
        if run >= 32 {
            out.push_str("<random>");
            i += run;
        } else {
            let n = run.max(1);
            out.push_str(&text[i..i + n]);
            i += n;
        }
    }
    out
}

/// Every table of the store, row by row, and the search index's rows (its
/// shadow tables' layout depends on how its rows were inserted). Migration
/// times come from the system clock.
fn dump(store: &Store) -> Vec<String> {
    let tables: Vec<String> = store
        .conn
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table'
             AND name NOT LIKE 'channel_fts%' AND name NOT LIKE 'sqlite_%'
             AND name != 'schema_migrations' ORDER BY name",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut out = Vec::new();
    let mut rows = |table: &str, sql: &str| {
        let mut stmt = store.conn.prepare(sql).unwrap();
        let n = stmt.column_count();
        let mut q = stmt.query([]).unwrap();
        while let Some(row) = q.next().unwrap() {
            let values: Vec<String> = (0..n)
                .map(|i| format!("{:?}", row.get::<_, rusqlite::types::Value>(i).unwrap()))
                .collect();
            out.push(mask_random(format!("{table}: {}", values.join(" | "))));
        }
    };
    for table in &tables {
        let has_rowid = store
            .conn
            .prepare(&format!("SELECT rowid FROM \"{table}\" LIMIT 0"))
            .is_ok();
        let select = if has_rowid { "rowid, *" } else { "*" };
        let sql = format!("SELECT {select} FROM \"{table}\"");
        let columns = store.conn.prepare(&sql).unwrap().column_count();
        let order: Vec<String> = (1..=columns).map(|i| i.to_string()).collect();
        rows(table, &format!("{sql} ORDER BY {}", order.join(",")));
    }
    if store.fts_index {
        rows(
            "channel_fts",
            "SELECT rowid, name, original FROM channel_fts ORDER BY rowid",
        );
    }
    out
}

/// The first difference of two dumps, with its neighbours.
fn assert_same(actual: &[String], expected: &[String], label: &str) {
    if actual == expected {
        return;
    }
    let at = actual
        .iter()
        .zip(expected)
        .position(|(a, b)| a != b)
        .unwrap_or(actual.len().min(expected.len()));
    let window = |rows: &[String]| rows[at.saturating_sub(2)..(at + 3).min(rows.len())].to_vec();
    panic!(
        "{label}: {} rows against {}; first difference at {at}:\n  got      {:#?}\n  expected {:#?}",
        actual.len(),
        expected.len(),
        window(actual),
        window(expected)
    );
}

/// Steps `job` with `budget` until it commits; returns the result and how
/// many steps it took.
fn drive<T>(mut step: impl FnMut() -> crate::Result<Step<T>>) -> (T, usize) {
    let mut steps = 0;
    loop {
        steps += 1;
        if let Step::Done(result) = step().unwrap() {
            return (result, steps);
        }
        assert!(steps < 1_000_000, "a replacement must end");
    }
}

#[test]
fn a_sliced_catalog_replacement_writes_what_the_one_shot_writes() {
    let (old_groups, old) = catalog(300, "Old");
    let (groups, new) = catalog(500, "New");
    let prepare = || {
        let (mut store, source) = store();
        store
            .replace_catalog(source, &old_groups, &old, &[], &[])
            .unwrap();
        // Overlays survive by stable id.
        store.set_favorite(source, &old[3].id, true).unwrap();
        store
            .rename_channel(source, &old[4].id, Some("Renamed"))
            .unwrap();
        (store, source)
    };
    let (mut expected, source) = prepare();
    let counts = expected
        .replace_catalog(source, &groups, &new, &[], &[])
        .unwrap();
    for budget in BUDGETS {
        let (mut store, source) = prepare();
        let mut job = CatalogReplacement::new(
            &mut store,
            source,
            groups.clone(),
            new.clone(),
            Vec::new(),
            Vec::new(),
        );
        let (published, steps) = drive(|| job.step(budget));
        assert!(job.is_done());
        assert!(
            job.step(budget).is_err(),
            "a finished job does nothing more"
        );
        drop(job);
        assert_eq!(published.counts, counts);
        assert!(budget >= 400 || steps > 2, "budget {budget}: {steps} steps");
        assert_same(&dump(&store), &dump(&expected), &format!("budget {budget}"));
    }
}

/// Where stages go: a temporary directory natively, a name in the browser.
fn stages() -> &'static std::path::Path {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    DIR.get_or_init(|| tempfile::tempdir().unwrap()).path()
}

fn stage_of(store: &Store, groups: &[Group], channels: &[Channel]) -> CatalogStage {
    let mut stage =
        CatalogStage::create_in_with(stages(), |path| store.open_staging_connection(path)).unwrap();
    for group in groups {
        stage.push(CatalogRow::Group(group.clone())).unwrap();
    }
    for channel in channels {
        stage.push(CatalogRow::Channel(channel.clone())).unwrap();
    }
    stage.finish().unwrap();
    stage
}

#[test]
fn a_sliced_stage_publication_writes_what_the_one_shot_writes() {
    let (old_groups, old) = catalog(200, "Old");
    let (groups, new) = catalog(700, "New");
    let urls = vec![SecretUrl::new("https://guide.example.test/provider.xml")];
    let prepare = || {
        let (mut store, source) = store();
        store
            .replace_catalog(source, &old_groups, &old, &[], &[])
            .unwrap();
        store.set_favorite(source, &old[7].id, true).unwrap();
        (store, source)
    };
    let (mut expected, source) = prepare();
    let stage = stage_of(&expected, &groups, &new);
    let (counts, guides) = expected
        .publish_catalog_stage_with_provider_guides(source, stage, None, &urls, None)
        .unwrap();
    assert_eq!(guides.len(), 1);
    for budget in BUDGETS {
        let (mut store, source) = prepare();
        let stage = stage_of(&store, &groups, &new);
        let publication = StagePublication::new().provider_guides(urls.clone(), None);
        let mut job =
            CatalogReplacement::from_stage(&mut store, source, stage, publication).unwrap();
        let (published, steps): (CatalogPublished, _) = drive(|| job.step(budget));
        drop(job);
        assert_eq!(published.counts, counts);
        assert_eq!(published.guides.as_deref(), Some(guides.as_slice()));
        assert!(budget >= 400 || steps > 2, "budget {budget}: {steps} steps");
        assert_same(&dump(&store), &dump(&expected), &format!("budget {budget}"));
    }
}

#[test]
fn an_abandoned_catalog_replacement_leaves_the_store_unchanged() {
    let (old_groups, old) = catalog(100, "Old");
    let (groups, new) = catalog(400, "New");
    let (mut store, source) = store();
    store
        .replace_catalog(source, &old_groups, &old, &[], &[])
        .unwrap();
    let before = dump(&store);
    let mut job =
        CatalogReplacement::new(&mut store, source, &groups[..], &new[..], &[][..], &[][..]);
    for _ in 0..5 {
        assert_eq!(job.step(50).unwrap(), Step::Pending);
    }
    drop(job);
    assert_same(&dump(&store), &before, "abandoned");
    // No transaction was left open.
    store.set_favorite(source, &old[0].id, true).unwrap();

    let stage = stage_of(&store, &groups, &new);
    let mut job =
        CatalogReplacement::from_stage(&mut store, source, stage, StagePublication::new()).unwrap();
    assert_eq!(job.step(10).unwrap(), Step::Pending);
    let store = job.into_store();
    store.set_favorite(source, &old[0].id, false).unwrap();
    assert_same(&dump(store), &before, "abandoned stage");
}

/// A staged guide of `channels` EPG channels with `per_channel` programs
/// each (every other one described), bound to playlist keys, some bound
/// twice and one bound to a feed that is not stored.
fn guide(store: &Store, channels: usize, per_channel: i64, tag: &str) -> (EpgStage, EpgImport) {
    let mut stage = EpgStage::create_in_store(store, stages()).unwrap();
    let mut listed = Vec::new();
    for feed in [1, 2] {
        let opts = StoreOptions {
            store_descriptions: true,
            past_days_to_keep: None,
        };
        stage
            .begin_feed(FeedId(feed), feed as i32, opts, NOW)
            .unwrap();
        let mut names = Vec::new();
        for c in 0..channels {
            let id = format!("f{feed}c{c}.example");
            names.push(ListedChannel {
                id: id.clone(),
                name: Some(format!("{tag} channel {c}")),
            });
            for p in 0..per_channel {
                let start = NOW + p * 1800;
                let title = format!("{tag} {c}/{p}");
                let desc = format!("About {tag} {c}/{p}");
                let program = ProgramData {
                    start,
                    stop: Some(start + 1800),
                    title: &title,
                    desc: (p % 2 == 0).then_some(desc.as_str()),
                    ..ProgramData::default()
                };
                stage.program(&id, &program).unwrap();
            }
        }
        stage.finish_feed().unwrap();
        listed.push((FeedId(feed), names));
    }
    let mut bindings: Vec<_> = (0..channels)
        .flat_map(|c| {
            let epg = |feed: u32, kind| iptv_epg::EpgRef {
                source: FeedId(feed),
                epg_id: format!("f{feed}c{c}.example"),
                kind,
            };
            [
                (
                    iptv_epg::ChannelKey(format!("a{c:04}")),
                    epg(1, MatchKind::TvgId),
                ),
                (
                    iptv_epg::ChannelKey(format!("b{c:04}")),
                    epg(1, MatchKind::Name),
                ),
                (
                    iptv_epg::ChannelKey(format!("c{c:04}")),
                    epg(3, MatchKind::Override),
                ),
            ]
        })
        .collect();
    bindings.sort_by(|a, b| a.0.cmp(&b.0));
    let import = EpgImport {
        bindings,
        listed,
        ..EpgImport::default()
    };
    (stage, import)
}

fn feeds() -> Vec<EpgFeedMeta> {
    [1, 2]
        .map(|feed| EpgFeedMeta {
            config: EpgSourceConfig::new(
                FeedId(feed),
                SecretUrl::new(format!("https://guide.example.test/{feed}.xml")),
            ),
            fetched_at: NOW,
            etag: None,
            last_modified: None,
            bytes: 1000,
        })
        .to_vec()
}

fn guide_sources() -> Vec<GuideSource> {
    feeds()
        .into_iter()
        .map(|f| GuideSource {
            feed: f.config.id,
            provider_index: None,
            url: f.config.url,
            name: None,
            enabled: true,
            shift_minutes: 0,
        })
        .collect()
}

#[test]
fn a_sliced_guide_replacement_writes_what_the_one_shot_writes() {
    let prepare = || {
        let (mut store, source) = store();
        store.set_guide_sources(source, &guide_sources()).unwrap();
        let (mut stage, import) = guide(&store, 30, 20, "Old");
        store
            .replace_epg(source, &mut stage, &import, &feeds(), 1)
            .unwrap();
        (store, source)
    };
    let (mut expected, source) = prepare();
    let (mut stage, import) = guide(&expected, 40, 25, "New");
    let counts = expected
        .replace_epg(source, &mut stage, &import, &feeds()[..1], 2)
        .unwrap();
    drop(stage);
    assert_eq!(counts.channels, 40);
    assert_eq!(counts.skipped_bindings, 40);
    for budget in BUDGETS {
        let (mut store, source) = prepare();
        let (mut stage, import) = guide(&store, 40, 25, "New");
        let mut job = EpgReplacement::new(
            &mut store,
            source,
            &mut stage,
            Cow::Owned(import),
            feeds()[..1].to_vec(),
            2,
        )
        .unwrap();
        let (sliced, steps) = drive(|| job.step(budget));
        drop(job);
        drop(stage);
        assert_eq!(sliced, counts);
        assert!(budget >= 400 || steps > 2, "budget {budget}: {steps} steps");
        assert_same(&dump(&store), &dump(&expected), &format!("budget {budget}"));
    }
}

#[test]
fn an_abandoned_guide_replacement_leaves_the_store_unchanged() {
    let (mut store, source) = store();
    store.set_guide_sources(source, &guide_sources()).unwrap();
    let (mut stage, import) = guide(&store, 10, 10, "Old");
    store
        .replace_epg(source, &mut stage, &import, &feeds(), 1)
        .unwrap();
    let before = dump(&store);
    let (mut stage, import) = guide(&store, 20, 10, "New");
    let mut job = EpgReplacement::new(
        &mut store,
        source,
        &mut stage,
        Cow::Borrowed(&import),
        feeds(),
        2,
    )
    .unwrap();
    for _ in 0..10 {
        assert_eq!(job.step(30).unwrap(), Step::Pending);
    }
    drop(job);
    assert_same(&dump(&store), &before, "abandoned");
    let fk: bool = store
        .conn
        .pragma_query_value(None, "foreign_keys", |r| r.get(0))
        .unwrap();
    assert!(fk, "the connection's settings are restored");
    // The stage is detached and the store writable: the replacement runs.
    store
        .replace_epg(source, &mut stage, &import, &feeds(), 2)
        .unwrap();
}
