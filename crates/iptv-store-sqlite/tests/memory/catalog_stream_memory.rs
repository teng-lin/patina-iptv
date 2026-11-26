//! Whole import + private SQLite stage + publish allocation regression.
//! Generated transport chunks never retain the full input JSON document.
use crate::allocations::{LIVE, PEAK};
use crate::common;
use std::sync::atomic::Ordering;

use iptv_import::{
    BodyStream, CancelToken, Fetch, FetchError, ImportOptions, NoProgress, StreamingResponse,
    import_source_into,
};
use iptv_providers::{HttpRequest, HttpResponse};
use iptv_store_sqlite::{CatalogStage, Store};
use std::time::Instant;

struct Generated {
    rows: usize,
}
struct Body {
    small: Option<Vec<u8>>,
    next: usize,
    total: usize,
    started: bool,
    ended: bool,
}
impl BodyStream for Body {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, FetchError> {
        if let Some(bytes) = self.small.take() {
            self.ended = true;
            return Ok(Some(bytes));
        }
        if self.ended {
            return Ok(None);
        }
        let mut out = String::new();
        if !self.started {
            out.push('[');
            self.started = true;
        }
        while self.next < self.total && out.len() < 16_384 {
            if self.next > 0 {
                out.push(',');
            }
            let i = self.next + 1;
            use std::fmt::Write;
            write!(out,r#"{{"stream_id":{i},"name":"Synthetic item {i}","category_id":"1","container_extension":"mp4","ignored":{{"nested":[1,2,3]}}}}"#).unwrap();
            self.next += 1;
        }
        if self.next == self.total {
            out.push(']');
            self.ended = true;
        }
        Ok(Some(out.into_bytes()))
    }
}
impl Fetch for Generated {
    type Body = Body;
    async fn fetch(&self, _request: HttpRequest) -> Result<HttpResponse, FetchError> {
        Ok(HttpResponse::new(
            200,
            br#"{"user_info":{"auth":1,"status":"Active"}}"#.to_vec(),
        ))
    }
    async fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> Result<StreamingResponse<Body>, FetchError> {
        Ok(StreamingResponse {
            status: 200,
            same_request_origin: false,
            content_length: None,
            body: Body {
                small: if request.url.contains("get_account_info")
                    || !request.url.contains("action=")
                {
                    Some(br#"{"user_info":{"auth":1,"status":"Active"}}"#.to_vec())
                } else {
                    None
                },
                next: 0,
                total: if request.url.contains("categories") {
                    0
                } else {
                    self.rows
                },
                started: false,
                ended: false,
            },
        })
    }
}
fn run(n: usize) -> (usize, usize) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("store.sqlite")).unwrap();
    let source = common::xtream_source("Synthetic catalog");
    let id = store.add_source(&source).unwrap();
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    // SAFETY: SQLite memory counters take no pointers.
    unsafe { rusqlite::ffi::sqlite3_memory_highwater(1) };
    let started = Instant::now();
    let mut stage = CatalogStage::create_in(dir.path()).unwrap();
    let mut options = ImportOptions::new(1_800_000_000);
    options.include_vod = true;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let catalog = runtime
        .block_on(import_source_into(
            &Generated { rows: n },
            &source.config,
            &options,
            &mut NoProgress,
            &CancelToken::new(),
            &mut stage,
        ))
        .unwrap();
    assert!(catalog.channels.is_empty() && catalog.vod.is_empty());
    assert_eq!(stage.counts().channels, n as u32);
    assert_eq!(stage.counts().vod, n as u32);
    assert_eq!(catalog.stats.content_items, 2 * n as u64);
    if n == 100 {
        assert_eq!(stage.load_channels().unwrap().len(), n);
        assert_eq!(stage.load_groups().unwrap().len(), 2);
    }
    let counts = store.replace_catalog_stage(id, stage).unwrap();
    assert_eq!(counts.channels, n as u32);
    assert_eq!(counts.vod, n as u32);
    let heap = (PEAK.load(Ordering::Relaxed) - base).max(0) as usize;
    // SAFETY: SQLite memory counters take no pointers.
    let sqlite = unsafe { rusqlite::ffi::sqlite3_memory_highwater(0) } as usize;
    eprintln!(
        "{n} channels + {n} VOD: Rust {heap} bytes, SQLite {sqlite} bytes, {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed().as_secs() < 120,
        "catalog import time ceiling exceeded"
    );
    assert!(heap < 12 << 20, "Rust heap {heap}");
    assert!(sqlite < 64 << 20, "SQLite heap {sqlite}");
    assert_eq!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("catalog-stage"))
            .count(),
        0
    );
    (heap, sqlite)
}
#[test]
fn hundred_thousand_channels_and_vod_stay_bounded() {
    crate::isolated(
        "catalog_stream_memory::hundred_thousand_channels_and_vod_stay_bounded",
        || {
            let (small, _) = run(100);
            let (large, _) = run(100_000);
            assert!(
                large < small + (2 << 20),
                "heap grew with catalog: {small} -> {large}"
            );
        },
    );
}
