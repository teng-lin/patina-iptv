use super::{
    tests::{owner, seed, until},
    *,
};
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MASTER: &[u8] = b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1200000\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2400000\nhigh.m3u8\n";

struct Fixture {
    url: String,
    requests: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    generation: Arc<AtomicUsize>,
    held: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn start() -> Self {
        Self::starting(false).await
    }
    async fn starting(hold: bool) -> Self {
        Self::serving(hold, false).await
    }
    async fn master() -> Self {
        Self::serving(false, true).await
    }
    async fn serving(hold: bool, master: bool) -> Self {
        let held = Arc::new(AtomicBool::new(hold));
        let paused = held.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let path = if master {
            "generated.m3u8"
        } else {
            "generated.ts"
        };
        let url = format!("http://{}/{path}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let generation = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let count = active.clone();
        let high = peak.clone();
        let current = generation.clone();
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
                    connection = listener.accept() => {
                        let (mut socket, _) = connection.unwrap();
                        let seen = seen.clone(); let count = count.clone(); let high = high.clone(); let current = current.clone(); let paused = paused.clone();
                        tasks.spawn(async move {
                            let mut request = Vec::new();
                            while !request.ends_with(b"\r\n\r\n") { request.push(socket.read_u8().await?); }
                            seen.fetch_add(1, Ordering::SeqCst);
                            high.fetch_max(count.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                            struct Active(Arc<AtomicUsize>);
                            impl Drop for Active { fn drop(&mut self) { self.0.fetch_sub(1, Ordering::SeqCst); } }
                            let _active = Active(count);
                            let incarnation = current.load(Ordering::SeqCst);
                            if master && !request.starts_with(b"GET /replacement.ts ") {
                                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\n\r\n", MASTER.len()).as_bytes()).await?;
                                socket.write_all(MASTER).await?;
                                let _ = socket.read_u8().await;
                                return Ok::<(), std::io::Error>(());
                            }
                            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n").await?;
                            let (mut reader, mut writer) = socket.into_split();
                            let mut packet = [0u8; 188 * 8];
                            for part in packet.as_chunks_mut::<188>().0 { part[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]); }
                            let mut interval = tokio::time::interval(Duration::from_millis(10));
                            loop {
                                tokio::select! {
                                    byte = reader.read_u8() => { let _ = byte; break; },
                                    _ = interval.tick() => {
                                        if current.load(Ordering::SeqCst) != incarnation { break; }
                                        if !paused.load(Ordering::SeqCst) { writer.write_all(&packet).await?; }
                                    }
                                }
                            }
                            Ok::<(), std::io::Error>(())
                        });
                    }
                }
            }
        });
        Self {
            url,
            requests,
            active,
            peak,
            generation,
            held,
            task,
        }
    }
}
async fn source(owner: &Runtime, request: &Request) -> PlaySource {
    let id = SourceId(request.source);
    let channel = ContentId::from_stored(request.channel.clone());
    let (config, channel) = owner
        .services()
        .read(move |store| {
            Ok((
                store.source(id)?.unwrap().source.config,
                store.channel(id, &channel)?.unwrap().channel,
            ))
        })
        .await
        .unwrap();
    let context = crate::playback::stored_context(&config).unwrap();
    let iptv_guide::playback::Resolution::Play(source) = iptv_guide::playback::resolve(
        Some(&context),
        &channel,
        None,
        crate::playback::clock(now(), None),
    ) else {
        panic!("generated direct source")
    };
    source
}
// Test callers resolve from this real store snapshot, like an imported context.
impl Runtime {
    fn prepare_current_live_source(
        &self,
        surface: usize,
        id: SourceId,
        key: String,
        source: PlaySource,
    ) -> impl std::future::Future<Output = Result<PlaySource, crate::Error>> + use<> {
        let member = ContentId::from_stored(key.clone());
        let services = self.services();
        // Resolve fixture metadata on a worker before constructing the owned
        // preparation future; Runtime itself intentionally cannot cross threads.
        let authority = std::thread::spawn(move || {
            services.blocking(move |store| {
                let stored = store.source(id)?.unwrap();
                let channel = store.channel(id, &member)?.unwrap().channel;
                Ok(crate::playback::source_authority(&stored).resolve(&channel))
            })
        })
        .join()
        .unwrap()
        .unwrap();
        self.prepare_live_source(surface, id, key, source, authority)
    }
}
async fn open(owner: &Runtime, request: &Request) -> PlaySource {
    refresh_accounts(&owner.services(), &owner.recordings)
        .await
        .unwrap();
    assert!(owner.reserve_live_viewing(0, SourceId(request.source), &request.channel));
    owner
        .prepare_current_live_source(
            0,
            SourceId(request.source),
            request.channel.clone(),
            source(owner, request).await,
        )
        .await
        .unwrap()
}
async fn player(source: &PlaySource) -> tokio::net::TcpStream {
    let (address, path) = source
        .url
        .strip_prefix("http://")
        .unwrap()
        .split_once('/')
        .unwrap();
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket
        .write_all(format!("GET /{path} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        header.push(socket.read_u8().await.unwrap());
    }
    assert!(header.starts_with(b"HTTP/1.1 200"));
    let mut packet = [0; 188];
    socket.read_exact(&mut packet).await.unwrap();
    assert_eq!(packet[0], 0x47);
    socket
}
async fn manifest(source: &PlaySource) -> String {
    let (address, path) = source
        .url
        .strip_prefix("http://")
        .unwrap()
        .split_once('/')
        .unwrap();
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            format!("GET /{path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    String::from_utf8(response).unwrap()
}
async fn wait(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn record_now(owner: &Runtime, mut request: Request) -> JobId {
    request.start = now();
    request.end = request.start + 30;
    let id = owner.schedule_recording(request).await.unwrap();
    until(owner, id, |job| {
        job.status == Status::Recording && job.bytes >= 188
    })
    .await;
    id
}

#[tokio::test]
async fn live_capture_uses_one_incoming_and_keeps_it_after_playback_stops() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let request = seed(&owner, &fixture.url).await;
    let local = open(&owner, &request).await;
    assert_ne!(local.url, fixture.url);
    assert!(local.headers.is_empty());
    let playback = player(&local).await;
    let id = record_now(&owner, request).await;
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.active.load(Ordering::SeqCst), 1);
    drop(playback);
    owner.release_viewing(0);
    let before = owner
        .recording_jobs()
        .await
        .unwrap()
        .into_iter()
        .find(|job| job.id == id)
        .unwrap()
        .bytes;
    until(&owner, id, |job| job.bytes > before).await;
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    owner.stop_recording(id).await.unwrap();
    assert_eq!(
        until(&owner, id, |job| !job.status.active()).await.status,
        Status::Completed
    );
    owner.recordings.retire_unused().await;
    assert_eq!(fixture.peak.load(Ordering::SeqCst), 1);
    assert!(owner.recordings.lock().inputs.is_empty());
    assert!(owner.drain_services());
}

#[tokio::test]
async fn stopping_capture_preserves_local_playback_source_and_input() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let request = seed(&owner, &fixture.url).await;
    let local = open(&owner, &request).await;
    let mut playback = player(&local).await;
    let incoming = owner.recordings.lock().viewers[&0]
        .input
        .as_ref()
        .unwrap()
        .id;
    let id = record_now(&owner, request).await;
    owner.stop_recording(id).await.unwrap();
    until(&owner, id, |job| !job.status.active()).await;
    owner.recordings.retire_unused().await;
    assert_eq!(
        owner.recordings.lock().viewers[&0]
            .input
            .as_ref()
            .unwrap()
            .id,
        incoming
    );
    let mut packet = [0; 188];
    playback.read_exact(&mut packet).await.unwrap();
    assert_eq!(packet[0], 0x47);
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.peak.load(Ordering::SeqCst), 1);
    drop(playback);
    owner.release_viewing(0);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn immediate_replacement_joins_the_previous_incoming_before_opening() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let request = seed(&owner, &fixture.url).await;
    let first = open(&owner, &request).await;
    let playback = player(&first).await;
    // Change only the resolved authentication context, as a portal token refresh
    // can do. The old owner is solely viewing, so it must retire first.
    let mut replacement = source(&owner, &request).await;
    replacement.set_header("X-Generated-Auth", "generated-new-context");
    assert!(owner.reserve_live_viewing(0, SourceId(request.source), &request.channel));
    let second = owner
        .prepare_current_live_source(0, SourceId(request.source), request.channel, replacement)
        .await
        .unwrap();
    assert_ne!(first.url, second.url);
    let next = player(&second).await;
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture.peak.load(Ordering::SeqCst),
        1,
        "replacement cannot overlap its retiring provider owner"
    );
    drop(playback);
    drop(next);
    owner.release_viewing(0);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn changed_auth_context_cannot_spend_a_shared_capture_discount() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let request = seed(&owner, &fixture.url).await;
    let local = open(&owner, &request).await;
    let playback = player(&local).await;
    let id = record_now(&owner, request.clone()).await;
    let mut changed = source(&owner, &request).await;
    changed.set_header("X-Generated-Auth", "generated-changed");
    assert!(owner.reserve_live_viewing(0, SourceId(request.source), &request.channel));
    assert!(
        owner
            .prepare_current_live_source(0, SourceId(request.source), request.channel, changed)
            .await
            .is_err()
    );
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.peak.load(Ordering::SeqCst), 1);
    owner.stop_recording(id).await.unwrap();
    until(&owner, id, |job| !job.status.active()).await;
    drop(playback);
    owner.release_viewing(0);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn shared_transport_failure_retains_partial_media_and_frees_playback_retry() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let request = seed(&owner, &fixture.url).await;
    let first = open(&owner, &request).await;
    let playback = player(&first).await;
    let id = record_now(&owner, request.clone()).await;
    fixture.generation.fetch_add(1, Ordering::SeqCst);
    let job = until(&owner, id, |job| !job.status.active()).await;
    assert_eq!(job.status, Status::Partial);
    assert!(job.bytes >= 188);
    assert_eq!(job.bytes % 188, 0);
    let second = open(&owner, &request).await;
    assert_ne!(first.url, second.url);
    let next = player(&second).await;
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.peak.load(Ordering::SeqCst), 1);
    drop(playback);
    drop(next);
    owner.release_viewing(0);
    assert!(owner.drain_services());
}

async fn xtream_account(owner: &Runtime, request: &Request, fixture: &Fixture, username: &str) {
    let source = SourceId(request.source);
    let base = fixture.url.trim_end_matches("/generated.ts").to_owned();
    let username = username.to_owned();
    owner
        .services()
        .call(move |store| {
            let mut stored = store.source(source)?.unwrap().source;
            stored.config = SourceConfig::Xtream(iptv_providers::model::XtreamCredentials {
                server_url: base,
                username: iptv_providers::Secret::new(username),
                password: iptv_providers::Secret::new("generated-secret"),
                user_agent: None,
            });
            store.update_source(source, &stored)
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn pending_account_owner_stays_budgeted_when_its_surface_switches_accounts() {
    let a = Fixture::starting(true).await;
    let b = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let old = seed(&owner, &a.url).await;
    xtream_account(&owner, &old, &a, "generated-a").await;
    let sibling = seed(&owner, &a.url).await;
    xtream_account(&owner, &sibling, &a, "generated-a").await;
    let other = seed(&owner, &b.url).await;
    xtream_account(&owner, &other, &b, "generated-b").await;
    refresh_accounts(&owner.services(), &owner.recordings)
        .await
        .unwrap();
    assert!(owner.reserve_live_viewing(0, SourceId(old.source), &old.channel));
    let pending = tokio::spawn(owner.prepare_current_live_source(
        0,
        SourceId(old.source),
        old.channel.clone(),
        source(&owner, &old).await,
    ));
    wait(|| a.requests.load(Ordering::SeqCst) == 1).await;
    assert!(owner.reserve_live_viewing(0, SourceId(other.source), &other.channel));
    let switched = tokio::spawn(owner.prepare_current_live_source(
        0,
        SourceId(other.source),
        other.channel.clone(),
        source(&owner, &other).await,
    ));
    assert!(owner.reserve_live_viewing(1, SourceId(sibling.source), &sibling.channel));
    let returning = tokio::spawn(owner.prepare_current_live_source(
        1,
        SourceId(sibling.source),
        sibling.channel.clone(),
        source(&owner, &sibling).await,
    ));
    wait(|| a.requests.load(Ordering::SeqCst) == 2).await;
    a.held.store(false, Ordering::SeqCst);
    assert!(pending.await.unwrap().is_err());
    assert!(switched.await.unwrap().is_ok());
    assert!(returning.await.unwrap().is_ok());
    assert_eq!(
        a.peak.load(Ordering::SeqCst),
        1,
        "the original account's pending request must retire before another surface opens it"
    );
    assert_eq!(b.peak.load(Ordering::SeqCst), 1);
    owner.release_viewing(0);
    owner.release_viewing(1);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn reused_incarnation_cannot_attach_the_old_incoming_at_the_same_url() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let request = seed(&owner, &fixture.url).await;
    let local = open(&owner, &request).await;
    let playback = player(&local).await;
    let id = record_now(&owner, request.clone()).await;
    let source_id = SourceId(request.source);
    owner
        .services()
        .call(move |store| {
            let mut source = store.source(source_id)?.unwrap().source;
            source.config = SourceConfig::M3u(iptv_providers::model::M3uSourceConfig {
                location: iptv_providers::SecretUrl::new(
                    "https://example.test/generated-replacement.m3u",
                ),
                user_agent: None,
                referrer: None,
            });
            store.update_source(source_id, &source)
        })
        .await
        .unwrap();
    refresh_accounts(&owner.services(), &owner.recordings)
        .await
        .unwrap();
    assert!(!owner.reserve_live_viewing(1, source_id, &request.channel));
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    owner.stop_recording(id).await.unwrap();
    until(&owner, id, |job| !job.status.active()).await;
    drop(playback);
    owner.release_viewing(0);
    assert!(owner.drain_services());
}

fn background_owner(directory: &Path) -> Runtime {
    let mut ports = crate::Ports::new(directory.to_owned(), |_| {
        Ok(iptv_store_sqlite::DatabaseKey::from_bytes([17; 32]))
    });
    ports.background_recordings = true;
    let runtime = Runtime::new(ports);
    runtime.start_services();
    runtime
}

#[tokio::test]
async fn suspended_shared_viewer_retains_capture_budget_and_resumes_the_same_input() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = background_owner(dir.path());
    let request = seed(&owner, &fixture.url).await;
    let local = open(&owner, &request).await;
    let playback = player(&local).await;
    let id = record_now(&owner, request.clone()).await;
    drop(playback);
    host::suspend_viewing(dir.path());
    assert!(owner.recordings.lock().viewers.is_empty());
    assert!(!owner.recordings.lock().capacity(request.source, None));
    assert!(host::resume_viewing(dir.path()));
    assert!(owner.recordings.lock().viewers[&0].input.is_some());
    let resumed = player(&local).await;
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    owner.stop_recording(id).await.unwrap();
    until(&owner, id, |job| !job.status.active()).await;
    drop(resumed);
    owner.release_viewing(0);
    assert!(owner.shutdown_recording_host());
    assert!(owner.drain_services());
}

#[tokio::test]
async fn background_shared_input_outlives_the_ui_executor_and_finishes_once() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    let owner = background_owner(&root);
    let mut request = seed(&owner, &fixture.url).await;
    let local = open(&owner, &request).await;
    let playback = player(&local).await;
    request.start = now();
    request.end = request.start + 4;
    let id = owner.schedule_recording(request).await.unwrap();
    until(&owner, id, |job| {
        job.status == Status::Recording && job.bytes >= 188
    })
    .await;
    let state = owner.recordings.clone();
    drop(playback);
    assert!(owner.drain_services());
    drop(owner);
    let services = state.host.lock().unwrap().as_ref().unwrap().accept();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let job = loop {
        let job = services
            .read(move |store| store.recording_job(id))
            .await
            .unwrap()
            .unwrap();
        if !job.status.active() {
            break job;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(job.status, Status::Completed);
    assert!(job.bytes >= 188);
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    assert!(state.stop());
    assert!(state.lock().inputs.is_empty());
}

#[tokio::test]
async fn adaptive_master_retires_probe_then_preserves_native_handoff_and_capture_budget() {
    let fixture = Fixture::master().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let mut request = seed(&owner, &fixture.url).await;
    let fallback = open(&owner, &request).await;
    assert_ne!(fallback.url, fixture.url);
    assert!(fallback.headers.is_empty());
    assert_eq!(owner.recordings.lock().inputs.len(), 1);
    assert!(
        owner
            .recordings
            .lock()
            .shared_for(request.source, &request.channel)
            .is_none()
    );
    // Immediately open native adaptive playback through its owned capability.
    // Its manifest still offers both variants; the probe selected neither.
    let manifest = manifest(&fallback).await;
    assert!(manifest.contains("BANDWIDTH=1200000"));
    assert!(manifest.contains("BANDWIDTH=2400000"));
    assert!(manifest.contains("low.m3u8"));
    assert!(manifest.contains("high.m3u8"));
    assert_eq!(
        fixture.requests.load(Ordering::SeqCst),
        2,
        "the probe must not select or fetch a variant"
    );
    assert_eq!(
        fixture.peak.load(Ordering::SeqCst),
        1,
        "native handoff must not overlap its unsupported probe"
    );
    request.start = now();
    request.end = request.start + 30;
    assert!(
        owner.schedule_recording(request).await.is_err(),
        "independent capture still needs a spare permitted connection"
    );
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 2);
    owner.release_viewing(0);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn actual_app_source_without_a_user_agent_shares_with_recording_defaults() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let request = seed(&owner, &fixture.url).await;
    refresh_accounts(&owner.services(), &owner.recordings)
        .await
        .unwrap();
    assert!(owner.reserve_live_viewing(0, SourceId(request.source), &request.channel));
    let original = PlaySource::new(&fixture.url);
    assert!(original.header("User-Agent").is_none());
    let local = owner
        .prepare_current_live_source(
            0,
            SourceId(request.source),
            request.channel.clone(),
            original,
        )
        .await
        .unwrap();
    let playback = player(&local).await;
    let id = record_now(&owner, request).await;
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.peak.load(Ordering::SeqCst), 1);
    owner.stop_recording(id).await.unwrap();
    until(&owner, id, |job| !job.status.active()).await;
    drop(playback);
    owner.release_viewing(0);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn per_channel_user_agent_precedes_playlist_and_network_defaults_when_sharing() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let request = seed(&owner, &fixture.url).await;
    let source_id = SourceId(request.source);
    let channel_id = ContentId::from_stored(request.channel.clone());
    owner
        .services()
        .call(move |store| {
            let mut stored = store.source(source_id)?.unwrap().source;
            if let SourceConfig::M3u(config) = &mut stored.config {
                config.user_agent = Some("GeneratedPlaylist/1".into());
            }
            store.update_source(source_id, &stored)?;
            let mut channel = store.channel(source_id, &channel_id)?.unwrap().channel;
            if let iptv_providers::model::StreamLocator::Url { headers, .. } = &mut channel.stream {
                headers.push(("user-agent".into(), "GeneratedChannel/1".into()));
            }
            store.replace_catalog(source_id, &[], &[channel], &[], &[])
        })
        .await
        .unwrap();
    assert_eq!(
        source(&owner, &request).await.header("User-Agent"),
        Some("GeneratedChannel/1")
    );
    let local = open(&owner, &request).await;
    let playback = player(&local).await;
    assert_eq!(
        owner.recordings.lock().viewers[&0]
            .input
            .as_ref()
            .unwrap()
            .source
            .header("User-Agent"),
        Some("GeneratedChannel/1")
    );
    let id = record_now(&owner, request).await;
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    owner.stop_recording(id).await.unwrap();
    until(&owner, id, |job| !job.status.active()).await;
    drop(playback);
    owner.release_viewing(0);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn a_new_incarnation_uses_fresh_provider_limit_after_a_stale_view_reservation() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let request = seed(&owner, &fixture.url).await;
    let first = open(&owner, &request).await;
    let playback = player(&first).await;
    let source_id = SourceId(request.source);
    owner
        .services()
        .call(move |store| {
            let mut source = store.source(source_id)?.unwrap().source;
            if let SourceConfig::M3u(config) = &mut source.config {
                config.location =
                    iptv_providers::SecretUrl::new("https://example.test/generated-new.m3u");
            }
            store.update_source(source_id, &source)
        })
        .await
        .unwrap();
    assert!(owner.reserve_live_viewing(0, source_id, &request.channel));
    // Model a provisional reservation made before the source-incarnation poll
    // discarded the old provider's observed hard allowance.
    owner.recordings.lock().viewers.get_mut(&0).unwrap().limit = 1;
    let prepared = owner
        .prepare_current_live_source(
            0,
            source_id,
            request.channel.clone(),
            source(&owner, &request).await,
        )
        .await
        .unwrap();
    assert_ne!(first.url, prepared.url);
    owner
        .recordings
        .lock()
        .provider_limits
        .insert(source_id.0, 3);
    refresh_accounts(&owner.services(), &owner.recordings)
        .await
        .unwrap();
    assert!(
        owner.recordings.lock().capacity(source_id.0, None),
        "the newly registered owner must not retain the old incarnation's hard allowance"
    );
    assert_eq!(fixture.peak.load(Ordering::SeqCst), 1);
    drop(playback);
    owner.release_viewing(0);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn owned_native_fallback_and_on_demand_retire_before_next_live_probe() {
    for context in [None, Some("vod:generated"), Some("catch-up:generated")] {
        let fixture = Fixture::master().await;
        let dir = tempfile::tempdir().unwrap();
        let owner = owner(dir.path());
        let request = seed(&owner, &fixture.url).await;
        refresh_accounts(&owner.services(), &owner.recordings)
            .await
            .unwrap();
        let fallback = if let Some(context) = context {
            assert!(owner.reserve_viewing(0, SourceId(request.source)));
            owner
                .prepare_independent_source(
                    0,
                    Some((SourceId(request.source), context.into())),
                    source(&owner, &request).await,
                    None,
                )
                .await
                .unwrap()
        } else {
            open(&owner, &request).await
        };
        assert!(manifest(&fallback).await.contains("BANDWIDTH=2400000"));
        wait(|| fixture.active.load(Ordering::SeqCst) == 1).await;
        let replacement = fixture.url.replace("generated.m3u8", "replacement.ts");
        let next = seed(&owner, &replacement).await;
        let local = open(&owner, &next).await;
        let playback = player(&local).await;
        assert_eq!(
            fixture.peak.load(Ordering::SeqCst),
            1,
            "the old adaptive/VOD/catch-up HTTP owner must close before probing live"
        );
        assert_eq!(
            fixture.requests.load(Ordering::SeqCst),
            if context.is_none() { 3 } else { 2 }
        );
        drop(playback);
        owner.release_viewing(0);
        assert!(owner.drain_services());
    }
}

#[tokio::test]
async fn rejected_live_and_independent_zaps_preserve_current_pending_preparation() {
    let capture = Fixture::start().await;
    let pending = Fixture::starting(true).await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let recording = seed(&owner, &capture.url).await;
    xtream_account(&owner, &recording, &capture, "generated-capture").await;
    let viewing = seed(&owner, &pending.url).await;
    xtream_account(&owner, &viewing, &pending, "generated-viewing").await;
    refresh_accounts(&owner.services(), &owner.recordings)
        .await
        .unwrap();
    let id = record_now(&owner, recording.clone()).await;
    assert!(owner.reserve_live_viewing(0, SourceId(viewing.source), &viewing.channel));
    let prepared = tokio::spawn(owner.prepare_current_live_source(
        0,
        SourceId(viewing.source),
        viewing.channel.clone(),
        source(&owner, &viewing).await,
    ));
    wait(|| pending.requests.load(Ordering::SeqCst) == 1).await;
    let revision = owner.recordings.lock().viewer_revision[&0];
    assert!(!owner.reserve_viewing(0, SourceId(recording.source)));
    assert!(!owner.reserve_live_viewing(0, SourceId(recording.source), &recording.channel));
    assert_eq!(owner.recordings.lock().viewer_revision[&0], revision);
    pending.held.store(false, Ordering::SeqCst);
    let local = prepared.await.unwrap().unwrap();
    let playback = player(&local).await;
    assert_eq!(pending.requests.load(Ordering::SeqCst), 1);
    assert_eq!(pending.peak.load(Ordering::SeqCst), 1);
    owner.stop_recording(id).await.unwrap();
    until(&owner, id, |job| !job.status.active()).await;
    drop(playback);
    owner.release_viewing(0);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn live_preparation_revalidates_enabled_membership_and_channel_authority_after_media_wait() {
    for mutation in ["disabled", "removed", "blocked", "stream"] {
        let fixture = Fixture::starting(true).await;
        let dir = tempfile::tempdir().unwrap();
        let owner = owner(dir.path());
        let request = seed(&owner, &fixture.url).await;
        refresh_accounts(&owner.services(), &owner.recordings)
            .await
            .unwrap();
        let source_id = SourceId(request.source);
        let channel_id = ContentId::from_stored(request.channel.clone());
        let original_incarnation = owner
            .services()
            .read(move |store| store.source_incarnation(source_id))
            .await
            .unwrap();
        assert!(owner.reserve_live_viewing(0, source_id, &request.channel));
        let preparation = tokio::spawn(owner.prepare_current_live_source(
            0,
            source_id,
            request.channel.clone(),
            source(&owner, &request).await,
        ));
        wait(|| fixture.requests.load(Ordering::SeqCst) == 1).await;
        owner
            .services()
            .call(move |store| {
                match mutation {
                    "disabled" => {
                        let mut policy = store.source_policy(source_id)?;
                        policy.enabled = false;
                        store.set_source_policy(source_id, policy)?;
                    }
                    "removed" => {
                        store.replace_catalog(source_id, &[], &[], &[], &[])?;
                    }
                    "blocked" => store.set_blocked(source_id, &channel_id, true)?,
                    "stream" => {
                        let mut channel = store.channel(source_id, &channel_id)?.unwrap().channel;
                        if let iptv_providers::model::StreamLocator::Url { headers, .. } =
                            &mut channel.stream
                        {
                            headers.push((
                                "Authorization".into(),
                                "Generated changed identity".into(),
                            ));
                        }
                        store.replace_catalog(source_id, &[], &[channel], &[], &[])?;
                    }
                    _ => unreachable!(),
                }
                assert_eq!(store.source_incarnation(source_id)?, original_incarnation);
                Ok(())
            })
            .await
            .unwrap();
        fixture.held.store(false, Ordering::SeqCst);
        assert!(
            preparation.await.unwrap().is_err(),
            "late {mutation} invalidates the original preparation"
        );
        assert!(owner.recordings.lock().inputs.is_empty());
        wait(|| fixture.active.load(Ordering::SeqCst) == 0).await;
        assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.peak.load(Ordering::SeqCst), 1);
        owner.release_viewing(0);
        assert!(owner.drain_services());
    }
}

#[tokio::test]
async fn unchanged_protected_live_member_reaches_the_app_grant_boundary() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let request = seed(&owner, &fixture.url).await;
    let source_id = SourceId(request.source);
    let channel = ContentId::from_stored(request.channel.clone());
    owner
        .services()
        .call(move |store| store.set_blocked(source_id, &channel, true))
        .await
        .unwrap();
    // The runtime preserves authority; the app's final synchronous parental
    // grant check decides whether the native source may be published.
    let local = open(&owner, &request).await;
    let playback = player(&local).await;
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    drop(playback);
    owner.release_viewing(0);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn rejected_reused_viewer_releases_only_its_own_capture_input_reference() {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let request = seed(&owner, &fixture.url).await;
    let local = open(&owner, &request).await;
    let playback = player(&local).await;
    let id = record_now(&owner, request.clone()).await;
    drop(playback);
    owner.release_viewing(0);
    let source_id = SourceId(request.source);
    assert!(owner.reserve_live_viewing(1, source_id, &request.channel));
    let resolved = source(&owner, &request).await;
    owner
        .services()
        .call(move |store| {
            let mut policy = store.source_policy(source_id)?;
            policy.enabled = false;
            store.set_source_policy(source_id, policy)
        })
        .await
        .unwrap();
    assert!(
        owner
            .prepare_current_live_source(1, source_id, request.channel, resolved)
            .await
            .is_err()
    );
    assert!(owner.recordings.lock().viewers[&1].input.is_none());
    assert_eq!(
        fixture.active.load(Ordering::SeqCst),
        1,
        "capture retains its own owner"
    );
    owner.stop_recording(id).await.unwrap();
    until(&owner, id, |job| !job.status.active()).await;
    // Durable completion and the remote socket closing can precede the
    // capture task's lease release and the registry's acknowledged shutdown.
    // Observe the actual owner release, then join retirement before inspecting
    // accounting; a closing entry deliberately remains counted until that join.
    wait(|| !owner.recordings.lock().active.contains_key(&id)).await;
    tokio::time::timeout(Duration::from_secs(5), owner.retire_viewing_inputs(1))
        .await
        .expect("capture input retirement acknowledged");
    wait(|| fixture.active.load(Ordering::SeqCst) == 0).await;
    assert!(owner.recordings.lock().inputs.is_empty());
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.peak.load(Ordering::SeqCst), 1);
    owner.release_viewing(1);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn recording_link_resolution_revalidates_member_authority_before_shared_attachment() {
    use iptv_providers::{
        Secret,
        model::{MacAddress, StalkerCredentials, StreamLocator},
        stalker::{PortalMode, StalkerPortal, StalkerSession},
    };
    for mutation in ["disabled", "removed", "blocked", "stream"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let portal_url = format!("http://{}/c/", listener.local_addr().unwrap());
        let (entered, seen) = tokio::sync::oneshot::channel();
        let (release, resumed) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            assert!(
                String::from_utf8(request)
                    .unwrap()
                    .contains("action=create_link")
            );
            entered.send(()).unwrap();
            resumed.await.unwrap();
            let body = r#"{"js":{"cmd":"ffmpeg http://127.0.0.1:9/generated.ts"}}"#;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let owner = owner(dir.path());
        let request = seed(&owner, "http://127.0.0.1:9/generated.ts").await;
        let source_id = SourceId(request.source);
        let channel_id = ContentId::from_stored(request.channel.clone());
        let expected_member = channel_id.clone();
        let credentials = StalkerCredentials {
            portal_url,
            mac: MacAddress::parse("00:1A:79:00:00:81").unwrap(),
            serial: None,
            device_id: None,
            device_id2: None,
            signature: None,
            signature2: None,
            login: None,
            password: None,
        };
        let config = SourceConfig::Stalker(credentials.clone());
        let stored_config = config.clone();
        let job = owner
            .services()
            .call(move |store| {
                let mut source = store.source(source_id)?.unwrap().source;
                source.config = stored_config;
                store.update_source(source_id, &source)?;
                let mut channel = store.channel(source_id, &channel_id)?.unwrap().channel;
                channel.stream = StreamLocator::Stalker {
                    cmd: "ffmpeg /generated-live".into(),
                    link: Default::default(),
                };
                store.replace_catalog(source_id, &[], &[channel], &[], &[])?;
                store.schedule_recording(JobId(81), &request, now())?;
                Ok(store.recording_job(JobId(81))?.unwrap())
            })
            .await
            .unwrap();
        let portal = StalkerPortal::from_credentials(
            &credentials,
            &iptv_providers::stalker::endpoint_candidates(&credentials.portal_url).unwrap()[0],
            PortalMode::Simple,
        )
        .unwrap();
        let session = StalkerSession::resume(portal, Secret::new("generated-session-token"), None);
        owner.recordings.contexts.lock().unwrap().insert(
            source_id.0,
            ContextEntry {
                pointer: 0,
                config,
                value: iptv_guide::playback::PlaybackContext::stalker(session),
            },
        );
        let services = owner.services();
        let work = services.clone();
        let state = owner.recordings.clone();
        let resolving =
            services.spawn(async move { resolve(&work, &state, &job, &CancelToken::new()).await });
        tokio::time::timeout(Duration::from_secs(5), seen)
            .await
            .unwrap()
            .unwrap();
        owner
            .services()
            .call(move |store| {
                match mutation {
                    "disabled" => {
                        let mut policy = store.source_policy(source_id)?;
                        policy.enabled = false;
                        store.set_source_policy(source_id, policy)?;
                    }
                    "removed" => {
                        store.replace_catalog(source_id, &[], &[], &[], &[])?;
                    }
                    "blocked" => store.set_blocked(source_id, &expected_member, true)?,
                    "stream" => {
                        let mut channel =
                            store.channel(source_id, &expected_member)?.unwrap().channel;
                        channel.stream = StreamLocator::Stalker {
                            cmd: "ffmpeg /changed-live".into(),
                            link: Default::default(),
                        };
                        store.replace_catalog(source_id, &[], &[channel], &[], &[])?;
                    }
                    _ => unreachable!(),
                }
                Ok(())
            })
            .await
            .unwrap();
        release.send(()).unwrap();
        assert_eq!(
            resolving.await.unwrap().is_err(),
            mutation != "blocked",
            "accepted intent survives a viewing block; late {mutation} still validates operational authority"
        );
        server.await.unwrap();
        assert!(owner.drain_services());
    }
}

#[tokio::test]
async fn viewing_searches_past_an_owner_with_different_resolved_headers() {
    matching_owner_at_capacity(false).await;
}

#[tokio::test]
async fn capture_reselects_the_matching_owner_after_resolution_at_capacity() {
    matching_owner_at_capacity(true).await;
}

async fn matching_owner_at_capacity(capture: bool) {
    let fixture = Fixture::start().await;
    let dir = tempfile::tempdir().unwrap();
    let mut settings = iptv_settings::Settings::new();
    settings
        .set_in(
            "playlists.max_connections",
            &iptv_settings::Target::playlist("1"),
            iptv_settings::Value::Int(2),
        )
        .unwrap();
    std::fs::write(dir.path().join("settings.json"), settings.to_json_string()).unwrap();
    let owner = owner(dir.path());
    crate::settings::load(&owner);
    let request = seed(&owner, &fixture.url).await;
    refresh_accounts(&owner.services(), &owner.recordings)
        .await
        .unwrap();
    let mut sockets = Vec::new();
    for surface in 0..2 {
        let mut stream = source(&owner, &request).await;
        stream.set_header("X-Generated-Auth", &format!("generated-{surface}"));
        assert!(owner.reserve_live_viewing(surface, SourceId(request.source), &request.channel));
        let local = owner
            .prepare_current_live_source(
                surface,
                SourceId(request.source),
                request.channel.clone(),
                stream,
            )
            .await
            .unwrap();
        sockets.push(player(&local).await);
    }
    // Hash iteration order is irrelevant: choose the other actual ready owner
    // after observing which owner the provisional identity-only search picks.
    let wanted = {
        let data = owner.recordings.lock();
        let first = data.shared_for(request.source, &request.channel).unwrap();
        data.viewers
            .values()
            .filter_map(|lease| lease.input.as_ref())
            .find(|incoming| incoming.id != first.id)
            .unwrap()
            .clone()
    };
    if capture {
        let id = SourceId(request.source);
        let key = ContentId::from_stored(request.channel.clone());
        let headers = wanted.source.headers.clone();
        owner
            .services()
            .call(move |store| {
                let mut channel = store.channel(id, &key)?.unwrap().channel;
                if let iptv_providers::model::StreamLocator::Url {
                    headers: current, ..
                } = &mut channel.stream
                {
                    *current = headers;
                } else {
                    panic!("generated URL channel");
                }
                store.replace_catalog(id, &[], &[channel], &[], &[])
            })
            .await
            .unwrap();
        let job = record_now(&owner, request.clone()).await;
        assert_eq!(
            owner.recordings.lock().active[&job]
                .0
                .input
                .as_ref()
                .unwrap()
                .id,
            wanted.id
        );
        owner.stop_recording(job).await.unwrap();
        until(&owner, job, |job| !job.status.active()).await;
    } else {
        assert!(owner.reserve_live_viewing(2, SourceId(request.source), &request.channel));
        let local = owner
            .prepare_current_live_source(
                2,
                SourceId(request.source),
                request.channel.clone(),
                wanted.source.clone(),
            )
            .await
            .unwrap();
        assert_eq!(local.url, wanted.input.shared().unwrap().url());
        owner.release_viewing(2);
    }
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.peak.load(Ordering::SeqCst), 2);
    drop(sockets);
    owner.release_viewing(0);
    owner.release_viewing(1);
    assert!(owner.drain_services());
}

#[tokio::test]
async fn resolved_authority_cannot_be_rebound_to_an_edited_account_or_channel() {
    for timing in ["queued", "before-entry"] {
        for mutation in ["account", "channel-headers"] {
            let fixture = Fixture::start().await;
            let dir = tempfile::tempdir().unwrap();
            let owner = owner(dir.path());
            let request = seed(&owner, &fixture.url).await;
            xtream_account(&owner, &request, &fixture, "generated-account-a").await;
            refresh_accounts(&owner.services(), &owner.recordings)
                .await
                .unwrap();
            let id = SourceId(request.source);
            let member = ContentId::from_stored(request.channel.clone());
            let expected_member = member.clone();
            let authority = owner
                .services()
                .read(move |store| {
                    let stored = store.source(id)?.unwrap();
                    let row = store.channel(id, &member)?.unwrap();
                    Ok(crate::playback::source_authority(&stored).resolve(&row.channel))
                })
                .await
                .unwrap();
            let stream = source(&owner, &request).await;
            assert!(owner.reserve_live_viewing(0, id, &request.channel));
            let gate = {
                let mut slots = owner.recordings.preparations.lock().unwrap();
                slots
                    .entry(0)
                    .or_insert_with(|| (CancelToken::new(), Arc::default()))
                    .1
                    .clone()
            };
            let held = gate.lock().await;
            let prepare = || {
                owner.prepare_live_source(
                    0,
                    id,
                    request.channel.clone(),
                    stream.clone(),
                    authority.clone(),
                )
            };
            let queued = if timing == "queued" {
                let (arrived, arrival) = tokio::sync::oneshot::channel();
                owner
                    .recordings
                    .preparation_arrivals
                    .lock()
                    .unwrap()
                    .insert(0, arrived);
                let queued = tokio::spawn(prepare());
                // A spawn alone says nothing about the worker's progress. The
                // actual task must reach the still-held surface gate before
                // the edit, so this differs from the before-entry case.
                tokio::time::timeout(Duration::from_secs(5), arrival)
                    .await
                    .expect("preparation did not reach the held surface gate")
                    .expect("preparation exited before reaching the held surface gate");
                Some(queued)
            } else {
                None
            };
            if mutation == "account" {
                xtream_account(&owner, &request, &fixture, "generated-account-b").await;
            } else {
                owner
                    .services()
                    .call(move |store| {
                        let mut channel = store.channel(id, &expected_member)?.unwrap().channel;
                        if let iptv_providers::model::StreamLocator::Url { headers, .. } =
                            &mut channel.stream
                        {
                            headers
                                .push(("X-Generated-Auth".into(), "generated-new-channel".into()));
                        }
                        store.replace_catalog(id, &[], &[channel], &[], &[])
                    })
                    .await
                    .unwrap();
            }
            let previous_incarnation = authority.incarnation().to_owned();
            let current_incarnation = owner
                .services()
                .read(move |store| store.source_incarnation(id))
                .await
                .unwrap();
            assert_eq!(
                current_incarnation != previous_incarnation,
                mutation == "account"
            );
            // Even an updated account cache cannot relabel the caller's older
            // credentials/channel snapshot while it waits at this surface.
            refresh_accounts(&owner.services(), &owner.recordings)
                .await
                .unwrap();
            let prepared = queued.unwrap_or_else(|| tokio::spawn(prepare()));
            drop(held);
            assert!(prepared.await.unwrap().is_err(), "{timing}/{mutation}");
            assert_eq!(fixture.requests.load(Ordering::SeqCst), 0);
            assert!(owner.recordings.lock().inputs.is_empty());
            owner.release_viewing(0);
            // A fresh resolution using the new, supported public store state
            // remains usable; rejection does not poison the numeric source ID.
            let current = open(&owner, &request).await;
            let socket = player(&current).await;
            assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
            drop(socket);
            owner.release_viewing(0);
            assert!(owner.drain_services());
        }
    }
}

/// A live HLS stream whose first segment starts as TS but loses sync after
/// the player already opened its first packets. Each resolve hands out a
/// new tokenized URL, as Stalker links and expiring M3U URLs do. The player
/// retries as soon as its playback ends: the second failure sends the
/// stream to the provider's own transport, recorded before the player saw
/// the end, whatever its URL's token.
#[tokio::test]
async fn a_first_segment_failing_after_its_first_packets_falls_back_on_retry() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/generated.m3u8", listener.local_addr().unwrap());
    let segments = Arc::new(AtomicUsize::new(0));
    let requested = segments.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let requested = requested.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await?);
                }
                if request.starts_with(b"GET /generated.m3u8") {
                    let body = b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n\
                                 #EXTINF:1,\nsegment.ts\n";
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                    socket.write_all(head.as_bytes()).await?;
                    return socket.write_all(body).await;
                }
                requested.fetch_add(1, Ordering::SeqCst);
                let mut packets = [[0x47u8; 188]; 4];
                packets[0][..4].copy_from_slice(&[0x47, 0x40, 0, 0x10]);
                packets[3][0] = 0;
                let body = packets.concat();
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                socket.write_all(head.as_bytes()).await?;
                socket.write_all(&body[..2 * 188]).await?;
                socket.flush().await?;
                // The rest, with its lost sync, once the player has joined.
                tokio::time::sleep(Duration::from_millis(200)).await;
                socket.write_all(&body[2 * 188..]).await
            });
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(dir.path());
    let request = seed(&owner, &url).await;
    refresh_accounts(&owner.services(), &owner.recordings)
        .await
        .unwrap();
    let mut attempt = 0;
    let mut resolve = || {
        attempt += 1;
        let request = request.clone();
        let (owner, url) = (&owner, url.clone());
        async move {
            let mut source = source(owner, &request).await;
            source.url = format!("{url}?token=generated-{attempt}");
            assert!(owner.reserve_live_viewing(0, SourceId(request.source), &request.channel));
            owner
                .prepare_current_live_source(
                    0,
                    SourceId(request.source),
                    request.channel.clone(),
                    source,
                )
                .await
                .unwrap()
        }
    };
    for _ in 0..2 {
        let shared = resolve().await;
        assert!(
            !shared.url.contains("token"),
            "the shared input's own address"
        );
        let mut playback = player(&shared).await;
        let mut rest = Vec::new();
        let _ = playback.read_to_end(&mut rest).await;
    }
    let fallback = resolve().await;
    assert!(
        manifest(&fallback).await.contains("segment.ts"),
        "the third attempt plays the provider's own transport"
    );
    assert_eq!(segments.load(Ordering::SeqCst), 2, "no third shared ingest");
    owner.release_viewing(0);
    assert!(owner.drain_services());
    server.abort();
}
