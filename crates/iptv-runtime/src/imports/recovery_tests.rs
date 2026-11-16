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
            PLAYLIST.len()
        )
        .unwrap();
        let _ = entered.send(());
        if block_body {
            let _ = resume.recv_timeout(LIMIT);
        }
        let _ = stream.write_all(PLAYLIST.as_bytes());
    });
    (url, observed, release, task)
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
    tokio::task::JoinHandle<()>,
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
            Msg::Playlist(Ok(_)) | Msg::Guide(_) | Msg::FirstScreen(_, _) => {
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
async fn cancellation_during_blocked_body_preserves_catalogue_and_cleans_encrypted_stage() {
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
    assert_eq!(failure(&mut rx), FailureKind::Cancelled);
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
    let gate = owner.host_handle().hold_import_publication().unwrap();
    let cancel = CancelToken::new();
    let (task, mut rx) = start(&owner, job, &cancel);
    assert!(gate.wait_entered(LIMIT));
    cancel.cancel();
    gate.release();
    tokio::time::timeout(LIMIT, task).await.unwrap().unwrap();
    assert_eq!(failure(&mut rx), FailureKind::Cancelled);
    assert_retained(&owner, id, "Generated retained source").await;
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
    assert_eq!(failure(&mut rx), FailureKind::Storage);
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
async fn a_failed_catalogue_transaction_preserves_source_details_and_survives_restart() {
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
