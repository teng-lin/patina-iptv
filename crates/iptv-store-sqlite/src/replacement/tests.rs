//! A replacement run in slices leaves exactly what the one-shot form
//! leaves, and an abandoned one leaves nothing.

use std::borrow::Cow;

use iptv_epg::{FeedId, MatchKind, ProgramData, StoreOptions};
use iptv_import::{CatalogRow, CatalogSink, EpgImport, EpgSourceConfig, GuideSink, ListedChannel};
use iptv_providers::SecretUrl;
use iptv_providers::model::{
    Channel, Group, M3uSourceConfig, Series, Source, SourceConfig, VodItem,
};

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

/// A catalog with every kind of row: groups, channels in one to three
/// groups (one in 300), movies and series in groups, and a repeated
/// channel, movie and series (duplicates the replacement drops).
struct Catalog {
    groups: Vec<Group>,
    channels: Vec<Channel>,
    vod: Vec<VodItem>,
    series: Vec<Series>,
}

fn full_catalog(channels: usize, tag: &str) -> Catalog {
    use iptv_providers::model::{ContentKind, SourceKind, StreamLocator};
    use iptv_providers::{ContentId, GroupId};
    let (mut groups, mut channels) = catalog(channels, tag);
    let many: Vec<Group> = (0..300)
        .map(|i| Group {
            id: GroupId::from_m3u_title(&format!("{tag} extra {i}")),
            name: format!("{tag} extra {i}"),
            kind: ContentKind::Live,
            provider_id: None,
            adult: false,
        })
        .collect();
    for (i, channel) in channels.iter_mut().enumerate() {
        channel
            .groups
            .extend(groups.iter().take(i % 3).map(|g| g.id.clone()));
    }
    channels[5].groups.extend(many.iter().map(|g| g.id.clone()));
    groups.extend(many);
    let movies = Group {
        id: GroupId::from_provider(SourceKind::Xtream, ContentKind::Movie, "5"),
        name: "Movies".into(),
        kind: ContentKind::Movie,
        provider_id: Some("5".into()),
        adult: false,
    };
    let vod: Vec<VodItem> = (0..40)
        .map(|i| VodItem {
            id: ContentId::from_provider(
                SourceKind::Xtream,
                ContentKind::Movie,
                &format!("{tag}{i}"),
            ),
            provider_id: Some(i.to_string()),
            name: format!("{tag} movie {i}"),
            groups: vec![movies.id.clone()],
            poster: None,
            plot: Some("A synthetic plot.".into()),
            year: Some(2020),
            rating_x10: Some(70),
            added: Some(1_700_000_000 + i),
            duration_secs: Some(5400),
            stream: StreamLocator::Xtream {
                stream_id: i as u64,
                container_extension: Some("mkv".into()),
            },
            is_series: false,
            episode_numbers: Vec::new(),
        })
        .collect();
    let series: Vec<Series> = (0..15)
        .map(|i| Series {
            id: ContentId::from_provider(
                SourceKind::Xtream,
                ContentKind::Series,
                &format!("{tag}{i}"),
            ),
            provider_id: Some(i.to_string()),
            name: format!("{tag} show {i}"),
            groups: vec![movies.id.clone()],
            poster: None,
            plot: None,
            year: Some(2021),
            rating_x10: None,
            last_modified: Some(1_700_000_500),
        })
        .collect();
    groups.push(movies);
    let mut catalog = Catalog {
        groups,
        channels,
        vod,
        series,
    };
    let (c, v, s) = (
        catalog.channels[3].clone(),
        catalog.vod[2].clone(),
        catalog.series[1].clone(),
    );
    catalog.channels.push(c);
    catalog.vod.push(v);
    catalog.series.push(s);
    catalog
}

/// A store with an old catalog and overlays on it; `fts: false` stores
/// without a search index (an SQLite without FTS5).
fn prepared(fts: bool) -> (Store, SourceId) {
    let (mut store, source) = store();
    if !fts {
        store.fts_index = false;
        store.fts = false;
    }
    let old = full_catalog(300, "Old");
    store
        .reference_replace_catalog(source, &old.groups, &old.channels, &old.vod, &old.series)
        .unwrap();
    // Overlays survive by stable id.
    store
        .set_favorite(source, &old.channels[3].id, true)
        .unwrap();
    store
        .rename_channel(source, &old.channels[4].id, Some("Renamed"))
        .unwrap();
    (store, source)
}

#[test]
fn a_catalog_replacement_writes_what_master_wrote() {
    let new = full_catalog(500, "New");
    for fts in [true, false] {
        let (mut expected, source) = prepared(fts);
        let counts = expected
            .reference_replace_catalog(source, &new.groups, &new.channels, &new.vod, &new.series)
            .unwrap();
        assert_eq!(counts.duplicates, 3);
        let expected = dump(&expected);
        // One call (the one-shot method), slices, and slices then `finish`.
        let (mut store, source) = prepared(fts);
        assert_eq!(
            store
                .replace_catalog(source, &new.groups, &new.channels, &new.vod, &new.series)
                .unwrap(),
            counts
        );
        assert_same(&dump(&store), &expected, &format!("one call, fts {fts}"));
        for budget in BUDGETS {
            let (mut store, source) = prepared(fts);
            let mut job = CatalogReplacement::new(
                &mut store,
                source,
                &new.groups[..],
                &new.channels[..],
                &new.vod[..],
                &new.series[..],
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
            assert_same(
                &dump(&store),
                &expected,
                &format!("budget {budget}, fts {fts}"),
            );
        }
        for steps in [1, 5, 20, 400, 2000] {
            let (mut store, source) = prepared(fts);
            let mut job = CatalogReplacement::new(
                &mut store,
                source,
                &new.groups[..],
                &new.channels[..],
                &new.vod[..],
                &new.series[..],
            );
            for _ in 0..steps {
                if let Step::Done(_) = job.step(7).unwrap() {
                    break;
                }
            }
            if job.is_done() {
                drop(job);
            } else {
                assert_eq!(job.finish().unwrap().counts, counts);
            }
            assert_same(
                &dump(&store),
                &expected,
                &format!("{steps} steps then finish, fts {fts}"),
            );
        }
    }
}

/// The budget counts every row a step writes, group memberships included:
/// a channel in 300 groups is written over many steps. (Without the search
/// index: SQLite counts an FTS5 row as several changes.)
#[test]
fn a_step_writes_no_more_rows_than_its_budget() {
    let new = full_catalog(200, "New");
    for budget in [1, 7, 50] {
        let (mut store, source) = prepared(false);
        let mut job = CatalogReplacement::new(
            &mut store,
            source,
            &new.groups[..],
            &new.channels[..],
            &new.vod[..],
            &new.series[..],
        );
        let mut deltas = Vec::new();
        loop {
            let before = job.store().conn.total_changes();
            let step = job.step(budget).unwrap();
            deltas.push(job.store().conn.total_changes() - before);
            if let Step::Done(_) = step {
                break;
            }
        }
        // The first step also checks the source; the last records the import.
        let inner = &deltas[1..deltas.len() - 1];
        let most = inner.iter().copied().max().unwrap();
        assert!(
            most <= u64::from(budget),
            "budget {budget}: a step wrote {most} rows"
        );
    }
}

/// Where stages go: a temporary directory natively, a name in the browser.
fn stages() -> &'static std::path::Path {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    DIR.get_or_init(|| tempfile::tempdir().unwrap()).path()
}

fn stage_of(store: &Store, catalog: &Catalog) -> CatalogStage {
    let mut stage =
        CatalogStage::create_in_with(stages(), |path| store.open_staging_connection(path)).unwrap();
    let rows = (catalog.groups.iter().cloned().map(CatalogRow::Group))
        .chain(catalog.channels.iter().cloned().map(CatalogRow::Channel))
        .chain(catalog.vod.iter().cloned().map(CatalogRow::Vod))
        .chain(catalog.series.iter().cloned().map(CatalogRow::Series));
    for row in rows {
        stage.push(row).unwrap();
    }
    stage.finish().unwrap();
    stage
}

#[test]
fn a_stage_publication_writes_what_master_wrote() {
    let new = full_catalog(700, "New");
    let urls = vec![SecretUrl::new("https://guide.example.test/provider.xml")];
    let details = Source {
        name: "Edited".into(),
        config: SourceConfig::M3u(M3uSourceConfig {
            location: SecretUrl::new("https://playlist.example.test/edited.m3u"),
            user_agent: Some("Synthetic/2".into()),
            referrer: None,
        }),
    };
    // Only the catalog; with details and plain guides; with provider guides.
    type Variant = (
        Option<Source>,
        Option<Vec<GuideSource>>,
        Option<Vec<SecretUrl>>,
    );
    let variants: [Variant; 3] = [
        (None, None, None),
        (Some(details.clone()), Some(guide_sources()), None),
        (Some(details), None, Some(urls)),
    ];
    for (index, (details, guides, urls)) in variants.into_iter().enumerate() {
        for fts in [true, false] {
            let (mut expected, source) = prepared(fts);
            let stage = stage_of(&expected, &new);
            let (counts, published) = expected
                .reference_publish_stage(
                    source,
                    stage,
                    details.as_ref(),
                    guides.as_deref(),
                    urls.as_deref(),
                )
                .unwrap();
            let expected = dump(&expected);
            let publication = || {
                let mut publication = StagePublication::new();
                if let Some(details) = &details {
                    publication = publication.details(details.clone());
                }
                if let Some(urls) = &urls {
                    publication = publication.provider_guides(urls.clone(), guides.clone());
                } else if let Some(guides) = &guides {
                    publication = publication.guides(guides.clone());
                }
                publication
            };
            for budget in [None, Some(1), Some(7), Some(61), Some(400)] {
                let (mut store, source) = prepared(fts);
                let stage = stage_of(&store, &new);
                let mut job =
                    CatalogReplacement::from_stage(&mut store, source, stage, publication())
                        .unwrap();
                let (result, steps): (CatalogPublished, _) = match budget {
                    Some(budget) => {
                        let done = drive(|| job.step(budget));
                        drop(job);
                        done
                    }
                    None => (job.finish().unwrap(), 1),
                };
                assert_eq!(result.counts, counts);
                assert_eq!(result.guides, published);
                assert!(
                    budget.is_none_or(|b| b >= 400) || steps > 2,
                    "budget {budget:?}: {steps} steps"
                );
                assert_same(
                    &dump(&store),
                    &expected,
                    &format!("variant {index}, budget {budget:?}, fts {fts}"),
                );
            }
            // Slices, then the rest in one call, from every phase.
            for steps in [10, 60, 150, 250, 350, 1000] {
                let (mut store, source) = prepared(fts);
                let stage = stage_of(&store, &new);
                let mut job =
                    CatalogReplacement::from_stage(&mut store, source, stage, publication())
                        .unwrap();
                for _ in 0..steps {
                    if let Step::Done(_) = job.step(13).unwrap() {
                        break;
                    }
                }
                if job.is_done() {
                    drop(job);
                } else {
                    job.finish().unwrap();
                }
                assert_same(
                    &dump(&store),
                    &expected,
                    &format!("variant {index}, {steps} slices then finish, fts {fts}"),
                );
            }
        }
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

    let staged = Catalog {
        groups: groups.clone(),
        channels: new.clone(),
        vod: Vec::new(),
        series: Vec::new(),
    };
    let stage = stage_of(&store, &staged);
    let mut job =
        CatalogReplacement::from_stage(&mut store, source, stage, StagePublication::new()).unwrap();
    assert_eq!(job.step(10).unwrap(), Step::Pending);
    let store = job.into_store();
    store.set_favorite(source, &old[0].id, false).unwrap();
    assert_same(&dump(store), &before, "abandoned stage");
}

/// A staged guide of `channels` EPG channels with `per_channel` programs
/// each (every other one described), bound to playlist keys: feed 1's
/// twice, feed 2's once, and keys bound to a feed 3 that is never stored.
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
                    epg(2, MatchKind::Override),
                ),
                (
                    iptv_epg::ChannelKey(format!("d{c:04}")),
                    epg(3, MatchKind::TvgId),
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
fn a_guide_replacement_writes_what_master_wrote() {
    let prepare = || {
        let (mut store, source) = store();
        store.set_guide_sources(source, &guide_sources()).unwrap();
        let (mut stage, import) = guide(&store, 30, 20, "Old");
        store
            .reference_replace_epg(source, &mut stage, &import, &feeds(), 1)
            .unwrap();
        (store, source)
    };
    // Every feed stored, and one feed missing (its bindings skipped).
    for stored in [2, 1] {
        let feeds = feeds()[..stored].to_vec();
        let (mut expected, source) = prepare();
        let (mut stage, import) = guide(&expected, 40, 25, "New");
        let counts = expected
            .reference_replace_epg(source, &mut stage, &import, &feeds, 2)
            .unwrap();
        drop(stage);
        assert_eq!(counts.channels, 40 * stored as u64);
        let expected = dump(&expected);
        for budget in [None, Some(1), Some(7), Some(61), Some(400)] {
            let (mut store, source) = prepare();
            let (mut stage, import) = guide(&store, 40, 25, "New");
            let mut job = EpgReplacement::new(
                &mut store,
                source,
                &mut stage,
                Cow::Borrowed(&import),
                &feeds[..],
                2,
            )
            .unwrap();
            let (sliced, steps) = match budget {
                Some(budget) => {
                    let done = drive(|| job.step(budget));
                    drop(job);
                    done
                }
                None => (job.finish().unwrap(), 1),
            };
            assert_eq!(sliced, counts);
            assert!(
                budget.is_none_or(|b| b >= 400) || steps > 2,
                "budget {budget:?}: {steps} steps"
            );
            assert_same(
                &dump(&store),
                &expected,
                &format!("{stored} feeds, budget {budget:?}"),
            );
        }
        // Slices, then the rest in one call; and an owned stage.
        let (mut store, source) = prepare();
        let (stage, import) = guide(&store, 40, 25, "New");
        let mut job = EpgReplacement::with_stage(
            &mut store,
            source,
            stage,
            Cow::Owned(import),
            feeds.clone(),
            2,
        )
        .unwrap();
        for _ in 0..40 {
            let _ = job.step(17).unwrap();
        }
        if job.is_done() {
            drop(job);
        } else {
            assert_eq!(job.finish().unwrap(), counts);
        }
        assert_same(
            &dump(&store),
            &expected,
            &format!("{stored} feeds, slices then finish"),
        );
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

/// Steps `job` 7 rows at a time until `reached(phase)` holds; whether it
/// did before the job finished.
macro_rules! step_until {
    ($job:expr, $reached:expr) => {{
        let mut hit = false;
        for _ in 0..100_000 {
            if $reached(&$job.phase()) {
                hit = true;
                break;
            }
            if let Step::Done(_) = $job.step(7).unwrap() {
                break;
            }
        }
        hit
    }};
}

/// A phase to finish from: its name and how to recognize it.
type Phase = (&'static str, fn(&str) -> bool);

/// `finish` after steps continues correctly from inside every phase that
/// keeps a cursor: the search index (rows and stage), an item's
/// memberships, a feed's listed channels, a binding's schedule copy.
#[test]
fn finish_resumes_from_inside_every_phase() {
    let new = full_catalog(300, "New");
    let rows_phases: [Phase; 3] = [
        ("memberships", |p| p.contains("member: Some")),
        ("index", |p| {
            p.starts_with("Index") && p.contains("after: Some")
        }),
        ("forget", |p| {
            p.starts_with("Forget") && p.contains("after: Some")
        }),
    ];
    let (mut expected, source) = prepared(true);
    expected
        .reference_replace_catalog(source, &new.groups, &new.channels, &new.vod, &new.series)
        .unwrap();
    let expected_rows = dump(&expected);
    for (name, reached) in rows_phases {
        let (mut store, source) = prepared(true);
        let mut job = CatalogReplacement::new(
            &mut store,
            source,
            &new.groups[..],
            &new.channels[..],
            &new.vod[..],
            &new.series[..],
        );
        let hit = step_until!(job, reached);
        assert!(hit, "rows never reached {name}");
        job.finish().unwrap();
        assert_same(
            &dump(&store),
            &expected_rows,
            &format!("rows, finish from {name}"),
        );
    }

    let (mut expected, source) = prepared(true);
    let stage = stage_of(&expected, &new);
    expected
        .reference_publish_stage(source, stage, None, None, None)
        .unwrap();
    let expected_stage = dump(&expected);
    let stage_phases: [Phase; 2] = [
        ("table copy", |p| {
            p.starts_with("Insert") && p.contains("after: Some")
        }),
        ("index", |p| {
            p.starts_with("Index") && p.contains("after: Some")
        }),
    ];
    for (name, reached) in stage_phases {
        let (mut store, source) = prepared(true);
        let stage = stage_of(&store, &new);
        let mut job =
            CatalogReplacement::from_stage(&mut store, source, stage, StagePublication::new())
                .unwrap();
        let hit = step_until!(job, reached);
        assert!(hit, "stage never reached {name}");
        job.finish().unwrap();
        assert_same(
            &dump(&store),
            &expected_stage,
            &format!("stage, finish from {name}"),
        );
    }

    let prepare = || {
        let (mut store, source) = store();
        store.set_guide_sources(source, &guide_sources()).unwrap();
        let (mut stage, import) = guide(&store, 30, 20, "Old");
        store
            .reference_replace_epg(source, &mut stage, &import, &feeds(), 1)
            .unwrap();
        (store, source)
    };
    let (mut expected, source) = prepare();
    let (mut stage, import) = guide(&expected, 40, 25, "New");
    expected
        .reference_replace_epg(source, &mut stage, &import, &feeds(), 2)
        .unwrap();
    drop(stage);
    let expected_guide = dump(&expected);
    let guide_phases: [Phase; 3] = [
        ("schedule delete", |p| p.contains("current: Some")),
        ("listed channels", |p| {
            p.contains("listed: Some(") && !p.contains("listed: Some(0)")
        }),
        ("schedule copy", |p| p.contains("copy: Some")),
    ];
    for (name, reached) in guide_phases {
        let (mut store, source) = prepare();
        let (mut stage, import) = guide(&store, 40, 25, "New");
        let mut job = EpgReplacement::new(
            &mut store,
            source,
            &mut stage,
            Cow::Borrowed(&import),
            feeds(),
            2,
        )
        .unwrap();
        let hit = step_until!(job, reached);
        assert!(hit, "guide never reached {name}");
        job.finish().unwrap();
        drop(stage);
        assert_same(
            &dump(&store),
            &expected_guide,
            &format!("guide, finish from {name}"),
        );
    }
}

/// A cleanup failure after the commit (detaching the stage, restoring the
/// connection) is returned, as master returned it; only on a browser
/// publish copy, where an error would make the caller abandon the copy, is
/// it logged and the step Done. Either way the job stays committed.
#[test]
fn a_cleanup_failure_after_the_commit_keeps_the_commit() {
    let fail = || Some(crate::StoreError::Unavailable("synthetic cleanup failure"));
    let returned = |finished: &crate::Result<Step<_>>| {
        matches!(finished, Err(crate::StoreError::Unavailable(_)))
    };
    let new = full_catalog(50, "New");
    let (mut catalog_store, source) = prepared(true);
    let stage = stage_of(&catalog_store, &new);
    let mut job =
        CatalogReplacement::from_stage(&mut catalog_store, source, stage, StagePublication::new())
            .unwrap();
    job.fail_cleanup = fail();
    assert!(returned(&job.step(u32::MAX)));
    assert!(job.is_done(), "the catalog committed");
    drop(job);

    let (mut store, source) = store();
    store.set_guide_sources(source, &guide_sources()).unwrap();
    let (mut stage, import) = guide(&store, 5, 5, "New");
    let mut job = EpgReplacement::new(
        &mut store,
        source,
        &mut stage,
        Cow::Borrowed(&import),
        feeds(),
        2,
    )
    .unwrap();
    job.fail_cleanup = fail();
    let finished = job.step(u32::MAX);
    assert!(matches!(finished, Err(crate::StoreError::Unavailable(_))));
    assert!(job.is_done(), "the guide committed");
    drop(job);
    drop(stage);

    #[cfg(target_family = "wasm")]
    {
        let mut copy = store.begin_publish().unwrap();
        let (mut stage, import) = guide(&copy, 5, 5, "Newer");
        let mut job = EpgReplacement::new(
            &mut copy,
            source,
            &mut stage,
            Cow::Borrowed(&import),
            feeds(),
            3,
        )
        .unwrap();
        job.fail_cleanup = fail();
        assert!(matches!(job.step(u32::MAX), Ok(Step::Done(_))));
        drop(job);
        drop(stage);
        store.finish_publish(copy).unwrap();
    }
}

/// Runs `job` on a publish copy in 7-row steps until `reached(phase)`,
/// checking that every pending step left the copy between transactions
/// (each slice commits there), then ends it in one call (`finish`) or in
/// more steps; whether the phase was reached.
#[cfg(target_family = "wasm")]
macro_rules! on_copy {
    ($job:expr, $reached:expr, $finish:expr) => {{
        let mut hit = false;
        let mut done = false;
        for _ in 0..100_000 {
            if !hit && $reached(&$job.phase()) {
                hit = true;
                if $finish {
                    break;
                }
            }
            match $job.step(7).unwrap() {
                Step::Pending => assert!($job.store().conn.is_autocommit()),
                Step::Done(_) => {
                    done = true;
                    break;
                }
            }
        }
        if done {
            drop($job);
        } else {
            $job.finish().unwrap();
        }
        hit
    }};
}

/// The same phases on a browser publish copy, where each slice commits and
/// the next begins again: finishing from each phase, or stepping on to the
/// end, then swapping the copy in, leaves what master's code leaves.
#[cfg(target_family = "wasm")]
#[test]
fn jobs_on_a_publish_copy_resume_from_every_phase() {
    let new = full_catalog(300, "New");
    let rows_phases: [Phase; 3] = [
        ("memberships", |p| p.contains("member: Some")),
        ("index", |p| {
            p.starts_with("Index") && p.contains("after: Some")
        }),
        ("forget", |p| {
            p.starts_with("Forget") && p.contains("after: Some")
        }),
    ];
    let (mut expected, source) = prepared(true);
    expected
        .reference_replace_catalog(source, &new.groups, &new.channels, &new.vod, &new.series)
        .unwrap();
    let expected_rows = dump(&expected);
    for (name, reached) in rows_phases {
        for finish in [true, false] {
            let (mut store, source) = prepared(true);
            let mut copy = store.begin_publish().unwrap();
            let mut job = CatalogReplacement::new(
                &mut copy,
                source,
                &new.groups[..],
                &new.channels[..],
                &new.vod[..],
                &new.series[..],
            );
            assert!(on_copy!(job, reached, finish), "rows never reached {name}");
            store.finish_publish(copy).unwrap();
            assert_same(
                &dump(&store),
                &expected_rows,
                &format!("rows on a copy, {name}, {finish}"),
            );
        }
    }

    let (mut expected, source) = prepared(true);
    let stage = stage_of(&expected, &new);
    expected
        .reference_publish_stage(source, stage, None, None, None)
        .unwrap();
    let expected_stage = dump(&expected);
    let stage_phases: [Phase; 2] = [
        ("table copy", |p| {
            p.starts_with("Insert") && p.contains("after: Some")
        }),
        ("index", |p| {
            p.starts_with("Index") && p.contains("after: Some")
        }),
    ];
    for (name, reached) in stage_phases {
        for finish in [true, false] {
            let (mut store, source) = prepared(true);
            let mut copy = store.begin_publish().unwrap();
            let stage = stage_of(&copy, &new);
            let mut job =
                CatalogReplacement::from_stage(&mut copy, source, stage, StagePublication::new())
                    .unwrap();
            assert!(on_copy!(job, reached, finish), "stage never reached {name}");
            store.finish_publish(copy).unwrap();
            assert_same(
                &dump(&store),
                &expected_stage,
                &format!("stage on a copy, {name}, {finish}"),
            );
        }
    }

    let prepare = || {
        let (mut store, source) = store();
        store.set_guide_sources(source, &guide_sources()).unwrap();
        let (mut stage, import) = guide(&store, 30, 20, "Old");
        store
            .reference_replace_epg(source, &mut stage, &import, &feeds(), 1)
            .unwrap();
        (store, source)
    };
    let (mut expected, source) = prepare();
    let (mut stage, import) = guide(&expected, 40, 25, "New");
    expected
        .reference_replace_epg(source, &mut stage, &import, &feeds(), 2)
        .unwrap();
    drop(stage);
    let expected_guide = dump(&expected);
    let guide_phases: [Phase; 3] = [
        ("schedule delete", |p| p.contains("current: Some")),
        ("listed channels", |p| {
            p.contains("listed: Some(") && !p.contains("listed: Some(0)")
        }),
        ("schedule copy", |p| {
            p.contains("copy: Some") && p.contains("after: Some")
        }),
    ];
    for (name, reached) in guide_phases {
        for finish in [true, false] {
            let (mut store, source) = prepare();
            let mut copy = store.begin_publish().unwrap();
            let (mut stage, import) = guide(&copy, 40, 25, "New");
            let mut job = EpgReplacement::new(
                &mut copy,
                source,
                &mut stage,
                Cow::Borrowed(&import),
                feeds(),
                2,
            )
            .unwrap();
            assert!(on_copy!(job, reached, finish), "guide never reached {name}");
            drop(stage);
            store.finish_publish(copy).unwrap();
            assert_same(
                &dump(&store),
                &expected_guide,
                &format!("guide on a copy, {name}, {finish}"),
            );
        }
    }
}

/// A job that committed slices on a publish copy and ended without its last
/// commit breaks the copy for good: another job completing on it, before
/// or after, does not make it publishable.
#[cfg(target_family = "wasm")]
#[test]
fn an_unfinished_job_on_a_copy_cannot_be_published_by_a_later_one() {
    let new = full_catalog(200, "New");
    for unfinished_first in [true, false] {
        let (mut store, source) = prepared(true);
        let other = store
            .add_source(&Source {
                name: "Other".into(),
                config: SourceConfig::M3u(M3uSourceConfig {
                    location: SecretUrl::new("https://playlist.example.test/other.m3u"),
                    user_agent: None,
                    referrer: None,
                }),
            })
            .unwrap();
        let mut copy = store.begin_publish().unwrap();
        let unfinished = |copy: &mut Store| {
            let mut job = CatalogReplacement::new(
                copy,
                source,
                &new.groups[..],
                &new.channels[..],
                &new.vod[..],
                &new.series[..],
            );
            for _ in 0..5 {
                assert_eq!(job.step(50).unwrap(), Step::Pending);
            }
        };
        let completed = |copy: &mut Store| {
            CatalogReplacement::new(
                copy,
                other,
                &new.groups[..],
                &new.channels[..10],
                &[][..],
                &[][..],
            )
            .finish()
            .unwrap();
        };
        if unfinished_first {
            unfinished(&mut copy);
            completed(&mut copy);
        } else {
            completed(&mut copy);
            unfinished(&mut copy);
        }
        assert!(
            store.finish_publish(copy).is_err(),
            "unfinished first: {unfinished_first}"
        );
        assert!(!store.is_publishing());
        // The host abandons it and begins a new publish, which works.
        let mut copy = store.begin_publish().unwrap();
        completed(&mut copy);
        store.finish_publish(copy).unwrap();
    }
}

/// A schedule copy's description pass: skipped for a channel without
/// staged descriptions (the browser's default), and in bounded slices of
/// program keys, each step paying for the keys it covered, when
/// descriptions are sparse (instead of one step scanning the schedule for
/// the few described rows).
#[test]
fn description_passes_are_skipped_or_bounded() {
    for sparse in [false, true] {
        let (mut store, source) = store();
        store.set_guide_sources(source, &guide_sources()).unwrap();
        let mut stage = EpgStage::create_in_store(&store, stages()).unwrap();
        let opts = StoreOptions {
            store_descriptions: sparse,
            past_days_to_keep: None,
        };
        stage.begin_feed(FeedId(1), 1, opts, NOW).unwrap();
        for p in 0..700 {
            let program = ProgramData {
                start: NOW + p * 1800,
                stop: Some(NOW + (p + 1) * 1800),
                title: "Synthetic",
                desc: (p == 350).then_some("The one described program"),
                ..ProgramData::default()
            };
            stage.program("only.example", &program).unwrap();
        }
        stage.finish_feed().unwrap();
        let import = EpgImport {
            bindings: vec![(
                iptv_epg::ChannelKey("k".into()),
                iptv_epg::EpgRef {
                    source: FeedId(1),
                    epg_id: "only.example".into(),
                    kind: MatchKind::TvgId,
                },
            )],
            ..EpgImport::default()
        };
        let mut job = EpgReplacement::new(
            &mut store,
            source,
            &mut stage,
            Cow::Borrowed(&import),
            feeds(),
            2,
        )
        .unwrap();
        let mut description_steps = 0;
        loop {
            let describing = job.phase().contains("descriptions: true");
            let step = job.step(7).unwrap();
            description_steps += usize::from(describing);
            if let Step::Done(counts) = step {
                assert_eq!(
                    (counts.programs, counts.descriptions),
                    (700, u64::from(sparse))
                );
                break;
            }
        }
        if sparse {
            assert!(description_steps >= 90, "sparse: {description_steps} steps");
        } else {
            assert_eq!(description_steps, 0, "no descriptions: no pass");
        }
    }
}

/// A catalog slice over the search index seeks the source's channels in
/// `channels_by_position` (it never walks another source's rows).
#[test]
fn catalog_index_slices_seek_the_source_s_channels() {
    let (store, _) = store();
    for after in [None, Some((0, 0))] {
        let filter = super::channels_after("", after);
        let sql = format!(
            "SELECT position, rowid FROM channels WHERE {filter} ORDER BY position, rowid LIMIT ?2"
        );
        let mut stmt = store
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let n = stmt.parameter_count();
        let plan: Vec<String> = stmt
            .query_map(
                rusqlite::params_from_iter(std::iter::repeat_n(0i64, n)),
                |r| r.get(3),
            )
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let plan = plan.join("; ");
        assert!(
            plan.contains("USING") && plan.contains("channels_by_position"),
            "{plan}"
        );
        assert!(plan.contains("source_id=?"), "{plan}");
    }
    // Measured: the slice after channel 4,000 of a source reads a few index
    // entries, not the earlier channels nor another source's.
    store
        .conn
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000)
             INSERT INTO channels (source_id, id, position, name, group_ids, stream)
             SELECT s, printf('c%d', i), i, 'Synthetic', '[]', '{}'
             FROM n, (SELECT 1 AS s UNION ALL SELECT 2);",
        )
        .unwrap();
    let rowid: i64 = store
        .conn
        .query_row(
            "SELECT rowid FROM channels WHERE source_id = 2 AND position = 4000",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let filter = super::channels_after("", Some((4000, rowid)));
    let mut stmt = store
        .conn
        .prepare(&format!(
            "SELECT position FROM channels WHERE {filter} ORDER BY position, rowid LIMIT ?2"
        ))
        .unwrap();
    let positions: Vec<i64> = stmt
        .query_map(rusqlite::params![2, 3, 4000, rowid], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(positions, [4001, 4002, 4003]);
    let steps = stmt.get_status(rusqlite::StatementStatus::VmStep);
    assert!(steps < 200, "{steps} VM steps");
}
