//! Real encrypted staging and writer admission under acknowledged failures.
use super::*;
use crate::Runtime;
use iptv_providers::model::{M3uSourceConfig, SourceConfig};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    sync::mpsc,
    time::Duration,
};

const KEY: [u8; 32] = [0x67; 32];
const LIMIT: Duration = Duration::from_secs(5);
const PLAYLIST: &str = "#EXTM3U\n#EXTINF:-1,Generated incoming\nhttps://example.test/incoming.ts\n";

/// The acknowledgement is sent only after a request has received its headers.
/// The response can be held during body I/O without occupying a runtime worker.
fn server(
    block_body: bool,
) -> (
    String,
    mpsc::Receiver<()>,
    mpsc::Sender<()>,
    std::thread::JoinHandle<()>,
) {
    response_server(PLAYLIST, block_body)
}

fn response_server(
    body: &'static str,
    block_body: bool,
) -> (
    String,
    mpsc::Receiver<()>,
    mpsc::Sender<()>,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/generated.m3u", listener.local_addr().unwrap());
    let (entered, observed) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let task = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(LIMIT)).unwrap();
        stream.set_write_timeout(Some(LIMIT)).unwrap();
        let mut request = Vec::new();
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
            assert!(request.len() < 8192);
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        let _ = entered.send(());
        if block_body {
            let _ = resume.recv_timeout(LIMIT);
        }
        let _ = stream.write_all(body.as_bytes());
    });
    (url, observed, release, task)
}

fn catalog_and_guide_server(now: i64) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let playlist = "#EXTM3U\n#EXTINF:-1 tvg-id=\"amber.main\",Generated incoming\n\
https://media.example.test/incoming.ts\n";
    let guide = format!(
        "<tv><channel id=\"amber.main\"><display-name>Generated Amber</display-name></channel>\
<programme channel=\"amber.main\" start=\"{}\" stop=\"{}\"><title>Generated Program</title></programme></tv>",
        iptv_epg::format_xmltv_time(now - 60),
        iptv_epg::format_xmltv_time(now + 3600),
    );
    let task = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + LIMIT;
        for _ in 0..2 {
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "expected guide request"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("generated server failed: {error}"),
                }
            };
            // Windows can inherit nonblocking mode from the listener.
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(LIMIT)).unwrap();
            stream.set_write_timeout(Some(LIMIT)).unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 8192);
            }
            let body = if request.starts_with(b"GET /guide.xml ") {
                guide.as_str()
            } else {
                playlist
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    (format!("http://{address}/playlist.m3u"), task)
}

fn source(url: &str, name: &str) -> Source {
    Source {
        name: name.into(),
        config: SourceConfig::M3u(M3uSourceConfig {
            location: SecretUrl::new(url),
            user_agent: None,
            referrer: None,
        }),
    }
}

async fn fixture(root: &Path, url: &str) -> (Runtime, Job) {
    let owner = Runtime::new(crate::Ports::new(root.into(), |_| {
        Ok(crate::model::DatabaseKey::from_bytes(KEY))
    }));
    owner.start_services();
    let original = source(url, "Generated retained source");
    let copy = original.clone();
    let id = owner
        .services()
        .call(move |store| {
            let id = store.add_source(&copy)?;
            let parsed = iptv_providers::m3u::parse_m3u(
                "#EXTM3U\n#EXTINF:-1,Generated retained\nhttps://example.test/retained.ts\n",
            );
            store.replace_catalog(id, &parsed.groups, &parsed.channels.items, &[], &[])?;
            Ok(id)
        })
        .await
        .unwrap();
    let job = Job {
        source: original,
        stored: Some(id),
        epg_url: None,
        purpose: Purpose::Refresh,
        edit: false,
    };
    (owner, job)
}

fn start(
    owner: &Runtime,
    job: Job,
    cancel: &CancelToken,
) -> (
    crate::execution::Join<()>,
    tokio::sync::mpsc::UnboundedReceiver<Msg>,
) {
    let services = owner.services();
    let worker = services.clone();
    let authorization = owner.parental_snapshot();
    let cancel = cancel.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let task = services.spawn_blocking(move || {
        run(
            &worker,
            job,
            &tx,
            &cancel,
            &CancelToken::new(),
            authorization,
        )
    });
    (task, rx)
}

fn failure(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Msg>) -> FailureKind {
    let mut failures = Vec::new();
    while let Ok(message) = rx.try_recv() {
        match message {
            Msg::Playlist(Err(error)) => failures.push(error.kind),
            Msg::Playlist(Ok(_)) | Msg::Guide(_) | Msg::FirstScreen(..) => {
                panic!("stale work published a success")
            }
            _ => {}
        }
    }
    assert_eq!(failures.len(), 1);
    failures[0]
}

fn assert_clean_stage(root: &Path) {
    assert!(!std::fs::read_dir(root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("catalog-stage-")
    }));
}

async fn assert_retained(owner: &Runtime, id: SourceId, name: &str) {
    let name = name.to_owned();
    owner
        .services()
        .read(move |store| {
            assert_eq!(store.source(id)?.unwrap().source.name, name);
            let channels = store.load_channels(id, &Default::default())?;
            assert_eq!(channels.len(), 1);
            assert_eq!(channels[0].name, "Generated retained");
            assert!(store.import_status(id)?.last_error.is_none());
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn cancellation_during_blocked_body_preserves_catalog_and_cleans_encrypted_stage() {
    let directory = tempfile::tempdir().unwrap();
    let (url, entered, release, server) = server(true);
    let (owner, job) = fixture(directory.path(), &url).await;
    let id = job.stored.unwrap();
    let cancel = CancelToken::new();
    let (task, mut rx) = start(&owner, job, &cancel);
    entered.recv_timeout(LIMIT).unwrap();
    let stage = std::fs::read_dir(directory.path())
        .unwrap()
        .find_map(|entry| {
            let path = entry.unwrap().path();
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("catalog-stage-")
                .then_some(path)
        })
        .unwrap();
    assert!(
        !std::fs::read(stage)
            .unwrap()
            .starts_with(b"SQLite format 3\0")
    );
    cancel.cancel();
    // The HTTP fixture stays held until cancellation has actually completed.
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
    assert_eq!(failure(&mut rx), FailureKind::Canceled);
    assert_retained(&owner, id, "Generated retained source").await;
    assert_clean_stage(directory.path());
    drop(release);
    server.join().unwrap();
    assert!(owner.drain_services());
}

#[tokio::test]
async fn cancellation_after_staging_cannot_publish_or_mark_a_failed_refresh() {
    let directory = tempfile::tempdir().unwrap();
    let (url, _, _, server) = server(false);
    let (owner, job) = fixture(directory.path(), &url).await;
    let id = job.stored.unwrap();
    let retained_guide = iptv_library::GuideSource {
        feed: iptv_epg::FeedId(5000),
        provider_index: Some(0),
        url: SecretUrl::new("https://guide.example.test/retained.xml"),
        name: None,
        enabled: true,
        shift_minutes: 0,
    };
    let initial = retained_guide.clone();
    owner
        .services()
        .call(move |store| store.set_guide_sources(id, &[initial]))
        .await
        .unwrap();
    let gate = owner.host_handle().hold_import_publication().unwrap();
    let cancel = CancelToken::new();
    let (task, mut rx) = start(&owner, job, &cancel);
    assert!(gate.wait_entered(LIMIT));
    cancel.cancel();
    gate.release();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
    assert_eq!(failure(&mut rx), FailureKind::Canceled);
    assert_retained(&owner, id, "Generated retained source").await;
    owner
        .services()
        .read(move |store| {
            assert_eq!(store.guide_sources(id)?, [retained_guide]);
            assert_eq!(store.guide_repair_feed_high_water(id)?, 5000);
            Ok(())
        })
        .await
        .unwrap();
    assert_clean_stage(directory.path());
    server.join().unwrap();
    assert!(owner.drain_services());
}

#[tokio::test]
async fn an_old_staged_completion_cannot_replace_a_recreated_source_with_the_same_config() {
    let directory = tempfile::tempdir().unwrap();
    let (url, _, _, server) = server(false);
    let (owner, job) = fixture(directory.path(), &url).await;
    let id = job.stored.unwrap();
    let original = job.source.clone();
    let gate = owner.host_handle().hold_import_publication().unwrap();
    let (task, mut rx) = start(&owner, job, &CancelToken::new());
    assert!(gate.wait_entered(LIMIT));
    owner
        .services()
        .call(move |store| {
            let before = store.source_incarnation(id)?;
            store.delete_source(id)?;
            let mut replacement = original;
            replacement.name = "Generated successor source".into();
            assert_eq!(store.add_source(&replacement)?, id);
            assert_ne!(store.source_incarnation(id)?, before);
            let parsed = iptv_providers::m3u::parse_m3u(
                "#EXTM3U\n#EXTINF:-1,Generated retained\nhttps://example.test/retained.ts\n",
            );
            store.replace_catalog(id, &parsed.groups, &parsed.channels.items, &[], &[])
        })
        .await
        .unwrap();
    gate.release();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
    assert_eq!(failure(&mut rx), FailureKind::Canceled);
    assert_retained(&owner, id, "Generated successor source").await;
    assert_clean_stage(directory.path());
    server.join().unwrap();
    assert!(owner.drain_services());
    drop(owner);
    let store = Store::open_encrypted(
        directory.path().join("iptv.sqlite"),
        &crate::model::DatabaseKey::from_bytes(KEY),
    )
    .unwrap();
    assert_eq!(
        store.load_channels(id, &Default::default()).unwrap()[0].name,
        "Generated retained"
    );
}

#[tokio::test]
async fn a_failed_catalog_transaction_preserves_source_details_and_survives_restart() {
    let directory = tempfile::tempdir().unwrap();
    let (url, _, _, server) = server(false);
    let (owner, mut job) = fixture(directory.path(), &url).await;
    let id = job.stored.unwrap();
    job.edit = true;
    job.source.name = "Generated attempted edit".into();
    let gate = owner.host_handle().hold_import_publication().unwrap();
    let (task, mut rx) = start(&owner, job, &CancelToken::new());
    assert!(gate.wait_entered(LIMIT));
    let raw = rusqlite::Connection::open(directory.path().join("iptv.sqlite")).unwrap();
    raw.pragma_update(None, "key", format!("x'{}'", "67".repeat(32)))
        .unwrap();
    raw.execute_batch(
        "CREATE TRIGGER generated_write_failure BEFORE INSERT ON channels
        BEGIN SELECT RAISE(FAIL, 'generated storage failure'); END;",
    )
    .unwrap();
    gate.release();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
    assert_eq!(failure(&mut rx), FailureKind::Storage);
    assert_retained(&owner, id, "Generated retained source").await;
    assert_clean_stage(directory.path());
    raw.execute_batch("DROP TRIGGER generated_write_failure")
        .unwrap();
    drop(raw);
    server.join().unwrap();
    assert!(owner.drain_services());
    drop(owner);
    let store = Store::open_encrypted(
        directory.path().join("iptv.sqlite"),
        &crate::model::DatabaseKey::from_bytes(KEY),
    )
    .unwrap();
    assert_eq!(
        store.source(id).unwrap().unwrap().source.name,
        "Generated retained source"
    );
    assert_eq!(
        store.load_channels(id, &Default::default()).unwrap()[0].name,
        "Generated retained"
    );
}

#[tokio::test]
async fn a_queued_refresh_cannot_adopt_configuration_changed_before_it_started() {
    let directory = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/generated.m3u", listener.local_addr().unwrap());
    let (owner, job) = fixture(directory.path(), &url).await;
    let id = job.stored.unwrap();
    let mut changed = job.source.clone();
    let SourceConfig::M3u(config) = &mut changed.config else {
        unreachable!()
    };
    config.user_agent = Some("GeneratedUpdated/1".into());
    owner
        .services()
        .call(move |store| store.update_source(id, &changed))
        .await
        .unwrap();
    let (task, mut rx) = start(&owner, job, &CancelToken::new());
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
    assert_eq!(failure(&mut rx), FailureKind::Canceled);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_retained(&owner, id, "Generated retained source").await;
    assert_clean_stage(directory.path());
    assert!(owner.drain_services());
}

#[tokio::test]
async fn a_configuration_edit_fences_an_old_stage_without_reusing_the_source_id() {
    let directory = tempfile::tempdir().unwrap();
    let (url, _, _, server) = server(false);
    let (owner, job) = fixture(directory.path(), &url).await;
    let id = job.stored.unwrap();
    let mut changed = job.source.clone();
    let SourceConfig::M3u(config) = &mut changed.config else {
        unreachable!()
    };
    config.user_agent = Some("GeneratedChanged/1".into());
    let gate = owner.host_handle().hold_import_publication().unwrap();
    let (task, mut rx) = start(&owner, job, &CancelToken::new());
    assert!(gate.wait_entered(LIMIT));
    let expected = changed.clone();
    owner
        .services()
        .call(move |store| {
            let incarnation = store.source_incarnation(id)?;
            store.update_source(id, &changed)?;
            // A public configuration edit changes the source incarnation even
            // though the stable source id remains the same.
            assert_ne!(store.source_incarnation(id)?, incarnation);
            Ok(())
        })
        .await
        .unwrap();
    gate.release();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
    assert_eq!(failure(&mut rx), FailureKind::Canceled);
    owner
        .services()
        .read(move |store| {
            assert!(store.source(id)?.unwrap().source.config == expected.config);
            assert_eq!(
                store.load_channels(id, &Default::default())?[0].name,
                "Generated retained"
            );
            Ok(())
        })
        .await
        .unwrap();
    assert_clean_stage(directory.path());
    server.join().unwrap();
    assert!(owner.drain_services());
}

#[tokio::test]
async fn a_retired_owners_staged_import_cannot_mutate_a_restarted_owners_store() {
    let directory = tempfile::tempdir().unwrap();
    let (url, _, _, server) = server(false);
    let (owner, job) = fixture(directory.path(), &url).await;
    let id = job.stored.unwrap();
    let gate = owner.host_handle().hold_import_publication().unwrap();
    let (task, mut rx) = start(&owner, job, &CancelToken::new());
    assert!(gate.wait_entered(LIMIT));
    assert!(owner.drain_services());
    drop(owner);
    let newer = Runtime::new(crate::Ports::new(directory.path().into(), |_| {
        Ok(crate::model::DatabaseKey::from_bytes(KEY))
    }));
    newer.start_services();
    newer.bind(|_| panic!("old import delivered an event to the replacement owner"));
    assert_retained(&newer, id, "Generated retained source").await;
    gate.release();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
    assert_eq!(failure(&mut rx), FailureKind::Canceled);
    assert_retained(&newer, id, "Generated retained source").await;
    assert_clean_stage(directory.path());
    server.join().unwrap();
    assert!(newer.drain_services());
}

#[tokio::test]
async fn an_empty_refresh_records_a_failure_but_an_empty_edit_preserves_committed_status() {
    for edit in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let (url, _, _, server) = response_server("#EXTM3U\n", false);
        let (owner, mut job) = fixture(directory.path(), &url).await;
        let id = job.stored.unwrap();
        job.edit = edit;
        let (task, mut rx) = start(&owner, job, &CancelToken::new());
        tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
        assert_eq!(failure(&mut rx), FailureKind::EmptyPlaylist);
        owner
            .services()
            .read(move |store| {
                assert_eq!(
                    store.load_channels(id, &Default::default())?[0].name,
                    "Generated retained"
                );
                assert_eq!(store.import_status(id)?.last_error.is_some(), !edit);
                Ok(())
            })
            .await
            .unwrap();
        assert_clean_stage(directory.path());
        server.join().unwrap();
        assert!(owner.drain_services());
    }
}

#[tokio::test]
async fn added_and_edited_playback_contexts_retain_the_committed_source_authority() {
    for edit in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let (url, _, _, server) = server(false);
        let (owner, mut job) = fixture(directory.path(), &url).await;
        if !edit {
            job.stored = None;
        }
        job.edit = edit;
        if let SourceConfig::M3u(config) = &mut job.source.config {
            config.user_agent = Some("GeneratedContext/81".into());
        }
        let (task, mut messages) = start(&owner, job, &CancelToken::new());
        tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
        let mut checked = 0;
        while let Ok(message) = messages.try_recv() {
            if let Msg::Playlist(result) = message {
                let (_, imported) = result.unwrap();
                let Streams::New(Some(context), _, origin) = imported.playback else {
                    panic!("successful import carries its own playback context");
                };
                let channel = &imported.channels[0];
                let authority = origin.resolve(channel);
                let source = imported.source;
                let committed = owner
                    .services()
                    .read(move |store| store.source(source))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(authority.source(), committed.id.0);
                assert_eq!(authority.incarnation(), committed.incarnation);
                assert_eq!(authority.config(), &committed.source.config);
                assert_eq!(authority.channel(), channel);
                let iptv_guide::playback::Resolution::Play(stream) = iptv_guide::playback::resolve(
                    Some(&context),
                    channel,
                    None,
                    crate::playback::clock(now_unix(), None),
                ) else {
                    panic!("generated direct stream");
                };
                assert_eq!(stream.header("User-Agent"), Some("GeneratedContext/81"));
                checked += 1;
            }
        }
        assert_eq!(checked, 1, "one committed catalog context");
        server.join().unwrap();
        assert!(owner.drain_services());
    }
}

#[tokio::test]
async fn automatic_guide_completion_keeps_the_imported_source_playback_context() {
    let directory = tempfile::tempdir().unwrap();
    let now = now_unix();
    let (playlist_url, server) = catalog_and_guide_server(now);
    let (owner, job) = fixture(directory.path(), &playlist_url).await;
    let source = job.stored.unwrap();
    let guide = GuideSource {
        feed: iptv_epg::FeedId(81),
        provider_index: None,
        url: SecretUrl::new(format!(
            "{}/guide.xml",
            playlist_url.strip_suffix("/playlist.m3u").unwrap()
        )),
        name: Some("Generated EPG".into()),
        enabled: true,
        shift_minutes: 0,
    };
    owner
        .services()
        .call(move |store| store.set_guide_sources(source, &[guide]))
        .await
        .unwrap();

    let (task, mut messages) = start(&owner, job, &CancelToken::new());
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
    let mut guide_completion = None;
    while let Ok(message) = messages.try_recv() {
        if let Msg::Guide(imported) = message {
            guide_completion = Some(imported);
        }
    }
    let imported = guide_completion.expect("automatic guide refresh sends its completed guide");
    assert!(
        imported.resume,
        "guide completion retains import resume semantics"
    );
    let Streams::New(context, _, authority) = imported.playback else {
        panic!("guide completion retains the source playback context");
    };
    assert!(context.is_some());
    assert_eq!(authority.source(), imported.source.0);
    server.join().unwrap();
    assert!(owner.drain_services());
}

#[tokio::test]
async fn exhausted_guide_identity_planning_preserves_the_retained_catalog() {
    const WITH_GUIDE: &str = "#EXTM3U url-tvg=\"https://guide.example.test/new.xml\"\n#EXTINF:-1,Generated incoming\nhttps://media.example.test/incoming.ts\n";
    let directory = tempfile::tempdir().unwrap();
    let (url, _, _, server) = response_server(WITH_GUIDE, false);
    let (owner, job) = fixture(directory.path(), &url).await;
    let id = job.stored.unwrap();
    let retained_guide = guides::user_source(
        iptv_epg::FeedId(u32::MAX),
        SecretUrl::new("https://guide.example.test/retained.xml"),
        None,
    );
    let initial = retained_guide.clone();
    owner
        .services()
        .call(move |store| store.set_guide_sources(id, &[initial]))
        .await
        .unwrap();
    let (task, mut rx) = start(&owner, job, &CancelToken::new());
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
    assert_eq!(failure(&mut rx), FailureKind::Storage);
    owner
        .services()
        .read(move |store| {
            assert_eq!(
                store.source(id)?.unwrap().source.name,
                "Generated retained source"
            );
            assert_eq!(
                store.load_channels(id, &Default::default())?[0]
                    .channel
                    .name,
                "Generated retained"
            );
            assert_eq!(store.guide_sources(id)?, [retained_guide]);
            assert_eq!(store.guide_repair_feed_high_water(id)?, u32::MAX);
            Ok(())
        })
        .await
        .unwrap();
    assert_clean_stage(directory.path());
    server.join().unwrap();
    assert!(owner.drain_services());
}
