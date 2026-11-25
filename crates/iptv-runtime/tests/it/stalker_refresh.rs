//! A failed live page must never publish over the last complete catalog.
#![cfg(feature = "test-support")]

use crate::common;
use iptv_core::add_playlist::{FailureKind, ImportCounts, ImportFailure};
use iptv_providers::model::{Source, SourceConfig};
use iptv_runtime::{Ports, Runtime, data};
use iptv_settings::{Settings, Value};
use iptv_store_sqlite::{ChannelQuery, DatabaseKey, ImportErrorKind, SourceId, Store};
use std::{
    cell::RefCell,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

const KEY: [u8; 32] = [0x54; 32];

struct Portal {
    address: String,
    revision: Arc<AtomicUsize>,
    fail_second: Arc<AtomicBool>,
    failed_requests: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Portal {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let revision = Arc::new(AtomicUsize::new(1));
        let version = revision.clone();
        let fail_second = Arc::new(AtomicBool::new(false));
        let failing = fail_second.clone();
        let failed_requests = Arc::new(AtomicUsize::new(0));
        let failures = failed_requests.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let worker = thread::spawn(move || {
            for connection in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let mut stream = connection.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..n]);
                    assert!(request.len() < 16_384);
                }
                let request = String::from_utf8(request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap();
                let query = path.split_once('?').map_or("", |(_, query)| query);
                let param = |key: &str| {
                    query.split('&').find_map(|part| {
                        let (name, value) = part.split_once('=')?;
                        (name == key).then_some(value)
                    })
                };
                let (status, body) = if param("action") == Some("get_ordered_list") {
                    let page: usize = param("p").unwrap().parse().unwrap();
                    assert!((1..=3).contains(&page), "unexpected pagination");
                    if page == 2 && failing.load(Ordering::SeqCst) {
                        failures.fetch_add(1, Ordering::SeqCst);
                        (503, "Synthetic page unavailable".into())
                    } else {
                        let revision = version.load(Ordering::SeqCst);
                        (
                            200,
                            format!(
                                concat!(
                                    r#"{{"js":{{"data":[{{"id":{0},"name":"Generated revision {1} channel {0}","#,
                                    r#""tv_genre_id":"1","cmd":"ffmpeg https://media.example.test/{0}.ts"}}],"#,
                                    r#""total_items":3,"max_page_items":1}}}}"#,
                                ),
                                page, revision,
                            ),
                        )
                    }
                } else {
                    assert_eq!(param("action"), Some("get_genres"));
                    (
                        200,
                        r#"{"js":[{"id":"1","title":"Generated live"}]}"#.into(),
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status} Synthetic\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        Self {
            address,
            revision,
            fail_second,
            failed_requests,
            stopped,
            worker: Some(worker),
        }
    }

    fn source(&self) -> Source {
        let mut source = common::stalker_source("Generated paged portal");
        let SourceConfig::Stalker(config) = &mut source.config else {
            unreachable!()
        };
        config.portal_url = format!("http://{}/c/", self.address);
        source
    }
}

impl Drop for Portal {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(&self.address);
        let result = self.worker.take().unwrap().join();
        if !thread::panicking() {
            assert!(result.is_ok());
        }
    }
}

async fn refresh(
    owner: &Rc<Runtime>,
    source: &Source,
    id: SourceId,
) -> Result<ImportCounts, ImportFailure> {
    let (send, receive) = futures_channel::oneshot::channel();
    let send = RefCell::new(Some(send));
    data::start_import(
        owner,
        data::Job {
            source: source.clone(),
            epg_url: None,
            stored: Some(id),
            purpose: data::Purpose::Background,
            edit: false,
        },
        move |update| {
            if let data::Update::Done(result) = update
                && let Some(send) = send.borrow_mut().take()
            {
                let _ = send.send(result);
            }
        },
    );
    let result = tokio::time::timeout(Duration::from_secs(20), receive)
        .await
        .expect("synthetic refresh timed out")
        .unwrap();
    if let Some(finished) = owner.pending_import_completion(id) {
        tokio::time::timeout(Duration::from_secs(20), finished)
            .await
            .unwrap();
    }
    result
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_later_live_page_preserves_the_complete_catalog_on_disk_until_a_successful_retry()
{
    tokio::task::LocalSet::new()
        .run_until(async {
            let portal = Portal::new();
            let source = portal.source();
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("iptv.sqlite");
            let key = DatabaseKey::from_bytes(KEY);
            let mut store = Store::open_encrypted(&path, &key).unwrap();
            let id = store.add_source(&source).unwrap();
            drop(store);
            let mut settings = Settings::new();
            settings
                .set("playlists.include_vod", Value::Bool(false))
                .unwrap();
            settings
                .set("epg.update", Value::choice("on_request"))
                .unwrap();
            std::fs::write(
                directory.path().join("settings.json"),
                settings.to_json_string(),
            )
            .unwrap();
            let owner = Rc::new(Runtime::new(Ports::new(directory.path().into(), |_| {
                Ok(DatabaseKey::from_bytes(KEY))
            })));
            owner.bind_tasks(
                |task| drop(tokio::task::spawn_local(task)),
                |delay| Box::pin(tokio::time::sleep(delay)),
            );
            owner.bind(|_| {});
            owner.load_settings();
            owner.start_services();
            assert_eq!(refresh(&owner, &source, id).await.unwrap().channels, 3);
            let store = Store::open_encrypted(&path, &key).unwrap();
            let original = store.load_channels(id, &ChannelQuery::default()).unwrap();
            let status = store.import_status(id).unwrap();
            assert_eq!(original.len(), 3);
            drop(store);

            portal.revision.store(2, Ordering::SeqCst);
            portal.fail_second.store(true, Ordering::SeqCst);
            assert_eq!(
                refresh(&owner, &source, id).await.unwrap_err().kind,
                FailureKind::Network
            );
            assert_eq!(
                portal.failed_requests.load(Ordering::SeqCst),
                2,
                "page retry exhausted"
            );
            let store = Store::open_encrypted(&path, &key).unwrap();
            assert_eq!(
                store.load_channels(id, &ChannelQuery::default()).unwrap(),
                original
            );
            let failed = store.import_status(id).unwrap();
            assert_eq!(failed.counts, status.counts);
            assert_eq!(failed.last_success_at, status.last_success_at);
            assert_eq!(failed.last_error, Some(ImportErrorKind::HttpStatus));
            drop(store);

            portal.fail_second.store(false, Ordering::SeqCst);
            assert_eq!(refresh(&owner, &source, id).await.unwrap().channels, 3);
            let store = Store::open_encrypted(&path, &key).unwrap();
            let replaced = store.load_channels(id, &ChannelQuery::default()).unwrap();
            assert_eq!(replaced.len(), 3);
            assert!(
                replaced
                    .iter()
                    .all(|channel| channel.name.contains("revision 2"))
            );
            assert_eq!(store.import_status(id).unwrap().last_error, None);
            drop(store);
            assert!(owner.drain_services());
        })
        .await;
}
