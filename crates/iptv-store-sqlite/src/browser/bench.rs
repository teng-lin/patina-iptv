//! Measurements of the browser store in the wasm runner (not tests):
//! `node tools/wasm-test.mjs --call bench_guide_publish:<budget>:<secure_delete> LIB.wasm`
//! on the unit-test executable built with `--release`. A 248,000-program
//! guide replaces one of the same size on a publish copy in slices; the
//! runner prints the times and the Wasm memory (which only grows, so its
//! size after the publish is the peak).

use std::borrow::Cow;

use iptv_epg::{ChannelKey, EpgRef, FeedId, MatchKind, ProgramData, StoreOptions};
use iptv_import::{EpgImport, EpgSourceConfig, GuideSink, ListedChannel};
use iptv_providers::SecretUrl;
use iptv_providers::model::{M3uSourceConfig, Source, SourceConfig};

use crate::wasm_test::{now_ms, report};
use crate::{BrowserFiles, EpgFeedMeta, EpgReplacement, EpgStage, SourceId, Step, Store};

const NOW: i64 = 1_800_000_000;
const CHANNELS: usize = 3000;
const PER_CHANNEL: i64 = 83;

fn memory_bytes() -> f64 {
    (core::arch::wasm32::memory_size(0) * 65536) as f64
}

fn guide(store: &Store, tag: &str) -> (EpgStage, EpgImport) {
    let mut stage = EpgStage::create_in_store(store, "/bench").unwrap();
    let options = StoreOptions {
        store_descriptions: false,
        past_days_to_keep: None,
    };
    stage.begin_feed(FeedId(1), 1, options, NOW).unwrap();
    let mut listed = Vec::new();
    for c in 0..CHANNELS {
        let id = format!("c{c}.example");
        listed.push(ListedChannel {
            id: id.clone(),
            name: None,
        });
        for p in 0..PER_CHANNEL {
            let start = NOW + p * 1800;
            let title = format!("{tag} program {c}/{p}");
            let program = ProgramData {
                start,
                stop: Some(start + 1800),
                title: &title,
                category: Some("News"),
                ..ProgramData::default()
            };
            stage.program(&id, &program).unwrap();
        }
    }
    stage.finish_feed().unwrap();
    let mut bindings: Vec<(ChannelKey, EpgRef)> = (0..CHANNELS)
        .map(|c| {
            let epg = EpgRef {
                source: FeedId(1),
                epg_id: format!("c{c}.example"),
                kind: MatchKind::TvgId,
            };
            (ChannelKey(format!("k{c:05}")), epg)
        })
        .collect();
    bindings.sort_by(|a, b| a.0.cmp(&b.0));
    let import = EpgImport {
        bindings,
        listed: vec![(FeedId(1), listed)],
        ..EpgImport::default()
    };
    (stage, import)
}

fn feeds() -> Vec<EpgFeedMeta> {
    vec![EpgFeedMeta {
        config: EpgSourceConfig::new(
            FeedId(1),
            SecretUrl::new("https://guide.example.test/1.xml"),
        ),
        fetched_at: NOW,
        etag: None,
        last_modified: None,
        bytes: 1,
    }]
}

/// One measurement; `spec` is `budget:secure_delete` (e.g. `400:1`).
#[unsafe(no_mangle)]
pub extern "C" fn bench_guide_publish(budget: u32, secure_delete: u32) {
    let files = BrowserFiles::new().unwrap();
    let mut store = Store::open_browser(&files).unwrap();
    store.set_clock(|| NOW);
    store
        .conn
        .pragma_update(None, "secure_delete", secure_delete != 0)
        .unwrap();
    let source: SourceId = store
        .add_source(&Source {
            name: "Bench".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://playlist.example.test/list.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let t = now_ms();
    let (mut stage, import) = guide(&store, "Old");
    store
        .replace_epg(source, &mut stage, &import, &feeds(), 1)
        .unwrap();
    drop(stage);
    let initial = now_ms() - t;
    let take = store.take_changed_chunks().unwrap();
    let size = take.size();
    take.saved();
    let before = memory_bytes();

    let t = now_ms();
    let copy = store.begin_publish().unwrap();
    copy.conn
        .pragma_update(None, "secure_delete", secure_delete != 0)
        .unwrap();
    let (stage, import) = guide(&copy, "New");
    let staged = now_ms() - t;
    let t = now_ms();
    let mut job =
        EpgReplacement::with_stage(copy, source, stage, Cow::Owned(import), feeds(), 2).unwrap();
    let (mut slices, mut longest) = (0, 0f64);
    loop {
        let s = now_ms();
        let step = job.step(budget).unwrap();
        longest = longest.max(now_ms() - s);
        slices += 1;
        if let Step::Done(_) = step {
            break;
        }
    }
    let copy = job.into_store();
    store.finish_publish(copy).unwrap();
    let published = now_ms() - t;
    let take = store.take_changed_chunks().unwrap();
    let changed = take.len();
    take.saved();
    report(&format!(
        "budget {budget} secure_delete {secure_delete}: database {:.1} MB, initial guide {initial:.0} ms, \
         staging {staged:.0} ms, publish {published:.0} ms in {slices} slices (longest {longest:.1} ms), \
         {changed} chunks changed; Wasm memory {:.1} MB before the publish, {:.1} MB after (peak)",
        size as f64 / 1e6,
        before / 1e6,
        memory_bytes() / 1e6,
    ));
}
