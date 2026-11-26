//! Exercises a real catalog import with the cooperative publication driver.

use super::{
    KEY, Loop, PLAYLIST_URL, reopen, setup_file_host, setup_with_network, synthetic_source,
};
use crate::{
    imports::{Job, Msg, Purpose, pipeline},
    network::{Body, Chunks, Client, NetFuture, Network, NetworkConfig, Transport},
    store_service::Service,
};
use iptv_core::add_playlist::{FailureKind, ImportStage};
use iptv_import::{BodyStream, CancelToken, FetchError, MemoryBody, StreamingResponse};
use iptv_providers::{
    HttpRequest, HttpResponse, SecretUrl,
    model::{Source, SourceConfig},
};
use iptv_store_sqlite::{ChannelQuery, SourceId, Store};
use std::{
    fmt::Write,
    sync::{Arc, atomic::Ordering},
    task::{Context, Waker},
    time::Duration,
};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

// The cooperative writer inserts at most 200 rows per turn.
const CHANNELS: usize = 2_000;
const RETAINED: &str = "Generated retained channel";
const GUIDE_URL: &str = "https://example.invalid/publish.xml";

#[derive(Clone)]
struct Playlist {
    playlist: Arc<[u8]>,
    guide: Option<Arc<[u8]>>,
}

impl Playlist {
    fn generated() -> Self {
        let mut playlist = String::from("#EXTM3U\n");
        for index in 0..CHANNELS {
            writeln!(
                playlist,
                "#EXTINF:-1 group-title=\"Generated\",Generated channel {index}\nhttps://example.invalid/generated/{index}.ts"
            )
            .unwrap();
        }
        Self {
            playlist: Arc::from(playlist.into_bytes()),
            guide: None,
        }
    }

    fn with_guide(mut self) -> Self {
        let now = chrono::Utc::now();
        let mut guide = String::from(
            "<tv><channel id=\"fixture\"><display-name>Generated channel 0</display-name></channel>",
        );
        for step in -1..399 {
            let start = now + chrono::TimeDelta::minutes(step);
            let next = start + chrono::TimeDelta::minutes(1);
            writeln!(
                guide,
                "<programme channel=\"fixture\" start=\"{}\" stop=\"{}\"><title>Generated {step}</title></programme>",
                start.format("%Y%m%d%H%M%S +0000"),
                next.format("%Y%m%d%H%M%S +0000"),
            )
            .unwrap();
        }
        guide.push_str("</tv>");
        self.guide = Some(Arc::from(guide.into_bytes()));
        self
    }
}

struct PlaylistBody(MemoryBody);

impl Chunks for PlaylistBody {
    fn next_chunk(&mut self) -> NetFuture<'_, Result<Option<Vec<u8>>, FetchError>> {
        Box::pin(self.0.next_chunk())
    }
}

impl Network for Playlist {
    fn client(&self, config: &NetworkConfig) -> Result<Client, FetchError> {
        Ok(Client::new(self.clone(), config.clone()))
    }
}

impl Transport for Playlist {
    fn elapsed(&self) -> Duration {
        Duration::ZERO
    }

    fn sleep(&self, _: Duration) -> NetFuture<'_, ()> {
        panic!("the generated playlist does not retry requests");
    }

    fn fetch(&self, _: HttpRequest) -> NetFuture<'_, Result<HttpResponse, FetchError>> {
        panic!("the generated playlist is read as a stream");
    }

    fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> NetFuture<'_, Result<StreamingResponse<Body>, FetchError>> {
        let bytes = match request.url.as_str() {
            PLAYLIST_URL => &self.playlist,
            GUIDE_URL => self.guide.as_ref().expect("the fixture includes a guide"),
            other => panic!("unexpected synthetic URL {other}"),
        };
        Box::pin(std::future::ready(Ok(StreamingResponse {
            status: 200,
            same_request_origin: false,
            content_length: Some(bytes.len() as u64),
            body: Body::new(PlaylistBody(MemoryBody::new(bytes, 8_192))),
        })))
    }
}

fn start(
    host: &Loop,
    service: &Arc<Service>,
    job: Job,
    cancel: &CancelToken,
) -> UnboundedReceiver<Msg> {
    start_with_revocation(host, service, job, cancel, &CancelToken::new())
}

fn start_with_revocation(
    host: &Loop,
    service: &Arc<Service>,
    job: Job,
    cancel: &CancelToken,
    revoked: &CancelToken,
) -> UnboundedReceiver<Msg> {
    use crate::execution::CooperativeHost;

    let authority = crate::Runtime::new(crate::Ports::for_tests()).parental_snapshot();
    let services = service.accept();
    let cancel = cancel.clone();
    let revoked = revoked.clone();
    let (sender, receiver) = unbounded_channel();
    host.spawn(Box::pin(async move {
        pipeline(&services, job, &sender, &cancel, &revoked, &authority).await;
    }));
    receiver
}

/// The mock StoreHost writes through a second WAL connection. A busy writer
/// acknowledges the replacement's transaction before the test cancels it.
fn advance_to_writing(host: &Loop, directory: &tempfile::TempDir) {
    let observer = writer_observer(&directory.path().join("synthetic.sqlite"));
    for _ in 0..20 {
        assert!(host.poll(), "the catalog import is still running");
        if writer_is_busy(&observer) {
            return;
        }
    }
    panic!("the catalog replacement should have opened a write transaction");
}

fn writer_observer(path: &std::path::Path) -> rusqlite::Connection {
    let observer = rusqlite::Connection::open(path).unwrap();
    let mut key = String::from("x'");
    for byte in KEY {
        write!(key, "{byte:02x}").unwrap();
    }
    key.push('\'');
    observer.pragma_update(None, "key", key).unwrap();
    observer.busy_timeout(Duration::ZERO).unwrap();
    observer
}

fn writer_is_busy(observer: &rusqlite::Connection) -> bool {
    match observer.execute_batch("BEGIN IMMEDIATE; ROLLBACK;") {
        Ok(()) => false,
        Err(error) if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy) => true,
        Err(error) => panic!("observe the replacement transaction: {error}"),
    }
}

fn finish(host: &Loop) {
    for _ in 0..200 {
        if !host.poll() {
            return;
        }
    }
    panic!("the import and store tasks should have finished");
}

fn assert_canceled(messages: &mut UnboundedReceiver<Msg>) {
    let mut failures = 0;
    while let Ok(message) = messages.try_recv() {
        match message {
            Msg::Playlist(Err(error)) => {
                assert_eq!(error.kind, FailureKind::Canceled);
                failures += 1;
            }
            Msg::Playlist(Ok(_)) | Msg::Guide(_) | Msg::FirstScreen(..) => {
                panic!("a canceled import published data");
            }
            _ => {}
        }
    }
    assert_eq!(failures, 1, "one canceled import result");
}

fn has_stage(directory: &tempfile::TempDir) -> bool {
    std::fs::read_dir(directory.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("catalog-stage-")
    })
}

fn assert_retained(store: &Store, source: SourceId, expected: &Source) {
    let retained = store.source(source).unwrap().unwrap().source;
    assert_eq!(retained.name, expected.name);
    assert_eq!(retained.config, expected.config);
    let channels = store
        .load_channels(source, &ChannelQuery::default())
        .unwrap();
    assert_eq!(channels.len(), 1, "the original catalog has one channel");
    assert_eq!(channels[0].name, RETAINED);
    assert!(store.import_status(source).unwrap().last_error.is_none());
}

fn seed_retained(service: &Arc<Service>) -> (SourceId, Source) {
    let original = synthetic_source();
    let stored = original.clone();
    let source = service
        .accept()
        .blocking(move |store| {
            let id = store.add_source(&stored)?;
            let playlist = iptv_providers::m3u::parse_m3u(&format!(
                "#EXTM3U\n#EXTINF:-1,{RETAINED}\nhttps://example.invalid/retained.ts\n"
            ));
            store.replace_catalog(id, &playlist.groups, &playlist.channels.items, &[], &[])?;
            Ok(id)
        })
        .unwrap();
    (source, original)
}

#[test]
fn canceling_a_sliced_edit_keeps_the_prior_catalog_readable_and_allows_a_retry() {
    let (directory, host, service, calls) = setup_with_network(Arc::new(Playlist::generated()));
    let (source, original) = seed_retained(&service);
    let mut edited = original.clone();
    edited.name = "Generated edited source".into();
    let SourceConfig::M3u(config) = &mut edited.config else {
        panic!("the fixture uses an M3U playlist");
    };
    config.user_agent = Some("GeneratedEdited/1".into());
    let job = Job {
        source: edited.clone(),
        stored: Some(source),
        epg_url: None,
        purpose: Purpose::Refresh,
        edit: true,
    };

    let cancel = CancelToken::new();
    let mut messages = start(&host, &service, job.clone(), &cancel);
    advance_to_writing(&host, &directory);
    assert_eq!(calls.begun.load(Ordering::SeqCst), 1);
    assert_eq!(calls.finished.load(Ordering::SeqCst), 0);
    let expected = original.clone();
    service
        .accept()
        .blocking_read(move |store| {
            assert_retained(store, source, &expected);
            Ok(())
        })
        .unwrap();
    cancel.cancel();
    finish(&host);
    assert_canceled(&mut messages);
    assert_eq!(calls.abandoned.load(Ordering::SeqCst), 1);
    assert_eq!(calls.finished.load(Ordering::SeqCst), 0);
    assert_retained(&reopen(&directory), source, &original);
    assert!(!has_stage(&directory), "cancellation removes the stage");

    let mut retry = start(&host, &service, job, &CancelToken::new());
    finish(&host);
    let mut completed = 0;
    while let Ok(message) = retry.try_recv() {
        if let Msg::Playlist(result) = message {
            let (counts, imported) = result.expect("a new import can publish after cancellation");
            assert_eq!(imported.source, source);
            assert_eq!(counts.channels as usize, CHANNELS);
            assert_eq!(imported.channels.len(), CHANNELS);
            completed += 1;
        }
    }
    assert_eq!(completed, 1, "one successful retry result");
    assert_eq!(calls.begun.load(Ordering::SeqCst), 2);
    assert_eq!(calls.finished.load(Ordering::SeqCst), 1);
    assert_eq!(calls.abandoned.load(Ordering::SeqCst), 1);
    assert!(service.drain_on_shutdown_with(|| {}));

    let store = reopen(&directory);
    let data = store.source(source).unwrap().unwrap().source;
    assert_eq!(data.name, edited.name);
    assert_eq!(data.config, edited.config);
    let channels = store
        .load_channels(source, &ChannelQuery::default())
        .unwrap();
    assert_eq!(channels.len(), CHANNELS, "the entire retry is durable");
    assert!(channels.iter().all(|channel| channel.name != RETAINED));
    assert!(
        !has_stage(&directory),
        "successful publication removes the stage"
    );
}

#[test]
fn canceling_a_sliced_add_removes_its_provisional_source() {
    let (directory, host, service, calls) = setup_with_network(Arc::new(Playlist::generated()));
    let services = service.accept();
    let existing = services
        .blocking(|store| store.add_source(&synthetic_source()))
        .unwrap();
    let mut source = synthetic_source();
    source.name = "Generated canceled source".into();
    let job = Job {
        source,
        stored: None,
        epg_url: None,
        purpose: Purpose::Add,
        edit: false,
    };
    let cancel = CancelToken::new();
    let mut messages = start(&host, &service, job, &cancel);
    advance_to_writing(&host, &directory);
    services
        .blocking_read(|store| {
            let sources = store.sources()?;
            let provisional = sources
                .iter()
                .find(|stored| stored.name == "Generated canceled source")
                .expect("preparation registered the tentative source");
            assert_eq!(
                store.channel_count(provisional.id, &ChannelQuery::default())?,
                0
            );
            Ok(())
        })
        .unwrap();
    cancel.cancel();
    finish(&host);
    assert_canceled(&mut messages);
    assert_eq!(calls.abandoned.load(Ordering::SeqCst), 1);
    assert_eq!(calls.finished.load(Ordering::SeqCst), 0);
    assert!(service.drain_on_shutdown_with(|| {}));
    let store = reopen(&directory);
    let sources = store.sources().unwrap();
    assert_eq!(sources.len(), 1, "only the already saved source remains");
    assert_eq!(sources[0].id, existing);
    assert!(!has_stage(&directory), "cancellation removes the stage");
}

fn typed_guide_add() -> Job {
    Job {
        source: synthetic_source(),
        stored: None,
        epg_url: Some(SecretUrl::new(GUIDE_URL)),
        purpose: Purpose::Add,
        edit: false,
    }
}

#[test]
fn stopping_the_required_guide_during_its_slices_discards_the_whole_add() {
    for revoke in [false, true] {
        let (directory, host, service, calls) =
            setup_file_host(Arc::new(Playlist::generated().with_guide()), true);
        let cancel = CancelToken::new();
        let revoked = CancelToken::new();
        let mut messages =
            start_with_revocation(&host, &service, typed_guide_add(), &cancel, &revoked);
        let copy_path = directory.path().join("synthetic.publish");
        let mut observer = None;
        let guide_writing = (0..100).any(|_| {
            assert!(host.poll(), "the guide is still saving");
            if !copy_path.exists() {
                return false;
            }
            let observer = observer.get_or_insert_with(|| writer_observer(&copy_path));
            let channels: i64 = observer
                .query_row("SELECT count(*) FROM channels", [], |row| row.get(0))
                .unwrap();
            channels == CHANNELS as i64 && writer_is_busy(observer)
        });
        assert!(
            guide_writing,
            "the guide reached its own sliced transaction"
        );
        assert_eq!(calls.begun.load(Ordering::SeqCst), 1, "one copy");
        assert_eq!(
            calls.finished.load(Ordering::SeqCst),
            0,
            "no early catalog swap"
        );
        assert!(
            reopen(&directory).sources().unwrap().is_empty(),
            "no provisional live source"
        );

        if revoke {
            revoked.cancel();
        } else {
            cancel.cancel();
        }
        finish(&host);
        assert_canceled(&mut messages);
        assert_eq!(calls.abandoned.load(Ordering::SeqCst), 1);
        assert_eq!(calls.finished.load(Ordering::SeqCst), 0);
        assert!(service.drain_on_shutdown_with(|| {}));
        assert!(reopen(&directory).sources().unwrap().is_empty());
        assert!(!has_stage(&directory));
    }
}

#[test]
fn cancellation_after_the_required_guide_swap_keeps_the_committed_add() {
    let (directory, host, service, calls) =
        setup_file_host(Arc::new(Playlist::generated().with_guide()), true);
    let cancel = CancelToken::new();
    *calls.cancel_after_swap.lock().unwrap() = Some(cancel.clone());
    let mut messages = start(&host, &service, typed_guide_add(), &cancel);
    finish(&host);
    assert!(
        cancel.is_canceled(),
        "the host canceled just after the swap"
    );
    let results: Vec<_> = std::iter::from_fn(|| messages.try_recv().ok())
        .filter_map(|message| match message {
            Msg::Playlist(result) => Some(result.expect("the completed add succeeded")),
            _ => None,
        })
        .collect();
    let [(counts, imported)] = results.as_slice() else {
        panic!("expected a single committed completion");
    };
    assert_eq!(counts.channels as usize, CHANNELS);
    assert_eq!(calls.begun.load(Ordering::SeqCst), 1);
    assert_eq!(calls.finished.load(Ordering::SeqCst), 1);
    assert_eq!(calls.abandoned.load(Ordering::SeqCst), 0);
    assert!(service.drain_on_shutdown_with(|| {}));
    let saved = Store::open_encrypted(
        directory.path().join("synthetic.publish"),
        &iptv_store_sqlite::DatabaseKey::from_bytes(KEY),
    )
    .unwrap();
    assert_eq!(saved.sources().unwrap().len(), 1);
    assert_eq!(
        saved
            .channel_count(imported.source, &ChannelQuery::default())
            .unwrap(),
        CHANNELS as u64,
    );
    let guide = saved.epg_guide(imported.source).unwrap().unwrap();
    assert_eq!(guide.bound_channels, 1);
    assert!(guide.ends_at.is_some(), "the guide contains saved programs");
    assert!(guide.match_fingerprint.is_some());
    let feed_results = saved.epg_feed_results(imported.source).unwrap();
    let [feed] = feed_results.as_slice() else {
        panic!("expected the required guide feed's saved result");
    };
    assert_eq!(feed.error, None);
    assert_eq!(feed.channel_count, 1);
    assert_eq!(feed.listed_count, 1);
    assert!(feed.program_count > 0);
    assert!(!has_stage(&directory));
}

#[test]
fn a_sliced_refresh_rejects_an_edit_committed_after_staging() {
    let (directory, host, service, calls) = setup_with_network(Arc::new(Playlist::generated()));
    let (source, original) = seed_retained(&service);
    finish(&host);
    let job = Job {
        source: original.clone(),
        stored: Some(source),
        epg_url: None,
        purpose: Purpose::Refresh,
        edit: false,
    };
    let mut messages = start(&host, &service, job, &CancelToken::new());
    // Check for Saving between rounds so the newly queued writer waits
    // until after the competing edit.
    let mut context = Context::from_waker(Waker::noop());
    let staged = (0..20).any(|_| {
        host.poll_round(&mut context);
        assert_eq!(calls.begun.load(Ordering::SeqCst), 0);
        std::iter::from_fn(|| messages.try_recv().ok()).any(|message| match message {
            Msg::Progress(progress) => progress.stage == ImportStage::Saving,
            Msg::Playlist(_) | Msg::Guide(_) | Msg::FirstScreen(..) => {
                panic!("the import reported a result before saving")
            }
            _ => false,
        })
    });
    assert!(staged, "the staged refresh is ready to save");
    assert!(has_stage(&directory));

    let mut newer = original;
    let SourceConfig::M3u(config) = &mut newer.config else {
        panic!("the fixture uses an M3U playlist");
    };
    config.user_agent = Some("GeneratedNewer/1".into());
    reopen(&directory).update_source(source, &newer).unwrap();
    finish(&host);
    assert_canceled(&mut messages);
    assert_eq!(
        calls.begun.load(Ordering::SeqCst),
        0,
        "the stale stage never starts writing"
    );
    assert_eq!(calls.finished.load(Ordering::SeqCst), 0);
    assert_eq!(calls.abandoned.load(Ordering::SeqCst), 0);
    assert!(service.drain_on_shutdown_with(|| {}));
    assert_retained(&reopen(&directory), source, &newer);
    assert!(!has_stage(&directory), "supersession removes the stage");
}
