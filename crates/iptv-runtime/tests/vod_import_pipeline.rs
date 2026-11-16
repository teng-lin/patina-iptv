//! Normal native imports must make movies/series discoverable without seeded
//! catalogue rows. Generated HTTP responses exercise download, stage and publish.
#![cfg(feature = "test-support")]

use iptv_core::wizard::ImportCounts;
use iptv_providers::{
    Secret,
    model::{Source, SourceConfig, XtreamCredentials},
};
use iptv_runtime::{Ports, Runtime, RuntimeEvent, data};
use iptv_settings::{Settings, Target, Value};
use iptv_store_sqlite::{ChannelQuery, DatabaseKey, SourceId, Store};
use std::{
    cell::{Cell, RefCell},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const KEY: [u8; 32] = [0x68; 32];
const INCLUDE: &str = "playlists.include_vod";
const LIVE: &[&str] = &["get_live_categories", "get_live_streams"];
const VOD: &[&str] = &[
    "get_vod_categories",
    "get_vod_streams",
    "get_series_categories",
    "get_series",
];

struct Provider {
    address: String,
    actions: Arc<Mutex<Vec<String>>>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Provider {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let actions = Arc::new(Mutex::new(Vec::new()));
        let requests = actions.clone();
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
                let mut bytes = Vec::new();
                let mut buffer = [0; 1024];
                while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                    assert!(bytes.len() < 16_384, "generated request exceeded its limit");
                }
                let request = String::from_utf8(bytes).unwrap();
                let path = request.split_whitespace().nth(1).unwrap();
                let action = path.split_once('?').and_then(|(_, query)| {
                    query
                        .split('&')
                        .find_map(|part| part.strip_prefix("action="))
                });
                if let Some(action) = action {
                    requests.lock().unwrap().push(action.to_owned());
                }
                let body = response(path, action);
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                );
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        Self {
            address,
            actions,
            stopped,
            worker: Some(worker),
        }
    }
    fn source(&self) -> Source {
        Source {
            name: "Generated full catalogue".into(),
            config: SourceConfig::Xtream(XtreamCredentials {
                server_url: format!("http://{}", self.address),
                username: Secret::new("generated-viewer"),
                password: Secret::new("generated-password"),
                user_agent: None,
            }),
        }
    }
    fn assert_requests(&self, include: bool) {
        let mut actual = std::mem::take(&mut *self.actions.lock().unwrap());
        actual.retain(|action| action != "get_account_info");
        actual.sort();
        let mut expected: Vec<_> = LIVE
            .iter()
            .chain(if include { VOD } else { &[] })
            .map(|action| (*action).to_owned())
            .collect();
        expected.sort();
        assert_eq!(
            actual, expected,
            "normal import requested the wrong content lists"
        );
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(&self.address);
        let joined = self.worker.take().unwrap().join();
        if !thread::panicking() {
            assert!(joined.is_ok(), "generated provider worker failed");
        }
    }
}
fn response(path: &str, action: Option<&str>) -> &'static str {
    if path.starts_with("/xmltv.php") {
        return "<tv/>";
    }
    match action {
        None | Some("get_account_info") => r#"{"user_info":{"auth":1,"status":"Active"}}"#,
        Some("get_live_categories") => r#"[{"category_id":"1","category_name":"Generated live"}]"#,
        Some("get_vod_categories") => r#"[{"category_id":"2","category_name":"Generated movies"}]"#,
        Some("get_series_categories") => {
            r#"[{"category_id":"3","category_name":"Generated series"}]"#
        }
        Some("get_live_streams") => {
            r#"[{"stream_id":101,"name":"Generated Live","category_id":"1"}]"#
        }
        Some("get_vod_streams") => concat!(
            "[{\"stream_id\":201,\"name\":\"Generated Movie A\",\"category_id\":\"2\",\"container_extension\":\"mp4\"},",
            "{\"stream_id\":202,\"name\":\"Generated Movie B\",\"category_id\":\"2\",\"container_extension\":\"mp4\"}]",
        ),
        Some("get_series") => r#"[{"series_id":301,"name":"Generated Show","category_id":"3"}]"#,
        Some(action) => panic!("unexpected generated provider action: {action}"),
    }
}

struct Host {
    owner: Rc<Runtime>,
    directory: tempfile::TempDir,
    publications: Rc<Cell<u64>>,
    has_vod: Rc<Cell<Option<bool>>>,
    any_vod: Rc<Cell<Option<bool>>>,
}
impl Host {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let owner = Rc::new(Runtime::new(Ports::new(directory.path().into(), |_| {
            Ok(DatabaseKey::from_bytes(KEY))
        })));
        owner.bind_tasks(
            |task| {
                drop(tokio::task::spawn_local(task));
            },
            |duration| Box::pin(tokio::time::sleep(duration)),
        );
        let publications = Rc::new(Cell::new(0));
        let presented = publications.clone();
        let has_vod = Rc::new(Cell::new(None));
        let discovery = has_vod.clone();
        let any_vod = Rc::new(Cell::new(None));
        let shelves = any_vod.clone();
        let weak = Rc::downgrade(&owner);
        owner.bind(move |event| match event {
            RuntimeEvent::ScheduleReady(schedule) => {
                let owner = weak.upgrade().unwrap();
                owner.set_schedule(schedule.clone());
                // The real shell reports the presented catalogue through this
                // public port; that publishes its live/VOD navigation state.
                owner.favorites_catalog_changed(schedule.as_ref());
                presented.set(presented.get() + 1);
            }
            RuntimeEvent::FavoritesChanged {
                has_vod, any_vod, ..
            } => {
                discovery.set(Some(has_vod));
                shelves.set(Some(any_vod));
            }
            _ => {}
        });
        let mut settings = Settings::new();
        settings
            .set("epg.update_on_start", Value::Bool(false))
            .unwrap();
        settings
            .set("epg.update_on_playlist_change", Value::Bool(false))
            .unwrap();
        std::fs::write(
            directory.path().join("settings.json"),
            settings.to_json_string(),
        )
        .unwrap();
        owner.load_settings();
        owner.start_services();
        Self {
            owner,
            directory,
            publications,
            has_vod,
            any_vod,
        }
    }
    /// Waits for a publication after `previous` reporting `shown` (the
    /// playlist on screen has movies or shows) and `any` (some enabled
    /// playlist has).
    async fn availability_after(&self, previous: u64, shown: bool, any: bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.publications.get() <= previous
                || self.has_vod.get() != Some(shown)
                || self.any_vod.get() != Some(any)
            {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("the shown playlist did not publish its on-demand availability");
    }
    async fn discovery_after(&self, previous: u64, expected: bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.publications.get() <= previous || self.has_vod.get() != Some(expected) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("All playlists did not publish its expected VOD discovery state");
        assert!(data::combined::active(&self.owner));
    }
    fn settings(&self, settings: &Settings) {
        std::fs::write(
            self.directory.path().join("settings.json"),
            settings.to_json_string(),
        )
        .unwrap();
        self.owner.load_settings();
    }
    async fn import(&self, source: &Source, stored: Option<SourceId>, include: bool) -> SourceId {
        let (send, receive) = futures_channel::oneshot::channel();
        let send = RefCell::new(Some(send));
        data::start_import(
            &self.owner,
            data::Job {
                source: source.clone(),
                epg_url: None,
                stored,
                purpose: if stored.is_some() {
                    data::Purpose::Refresh
                } else {
                    data::Purpose::Add
                },
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
        let counts = tokio::time::timeout(Duration::from_secs(20), receive)
            .await
            .expect("normal import timed out")
            .unwrap()
            .expect("generated import failed");
        let expected = ImportCounts {
            channels: 1,
            groups: if include { 3 } else { 1 },
            movies: if include { 2 } else { 0 },
            series: u32::from(include),
        };
        assert_eq!(
            counts, expected,
            "discovery/wizard counts must expose imported movies and series"
        );
        let source = stored.unwrap_or_else(|| self.owner.shown_source().unwrap());
        if let Some(finished) = self.owner.pending_import_completion(source) {
            tokio::time::timeout(Duration::from_secs(20), finished)
                .await
                .unwrap();
        }
        let store = Store::open_encrypted(
            self.directory.path().join("iptv.sqlite"),
            &DatabaseKey::from_bytes(KEY),
        )
        .unwrap();
        assert_eq!(
            store
                .load_channels(source, &ChannelQuery::default())
                .unwrap()
                .len(),
            1
        );
        let movies = store.load_vod(source, None, 0, None).unwrap();
        let series = store.load_series(source, None, 0, None).unwrap();
        assert_eq!(movies.len(), counts.movies as usize);
        assert_eq!(series.len(), counts.series as usize);
        if include {
            assert_eq!(movies[0].name, "Generated Movie A");
            assert_eq!(series[0].name, "Generated Show");
        }
        let recorded = store.import_status(source).unwrap().counts;
        assert_eq!(
            (recorded.channels, recorded.vod, recorded.series),
            (counts.channels, counts.movies, counts.series)
        );
        source
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let drained = self.owner.drain_services();
        if !thread::panicking() {
            assert!(drained);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn normal_xtream_import_discovers_vod_and_respects_global_and_playlist_preferences() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let provider = Provider::new();
            let host = Host::new();
            let source = provider.source();
            // No VOD preference is seeded: a normal first import must request all lists.
            let id = host.import(&source, None, true).await;
            provider.assert_requests(true);
            let mut settings = (*host.owner.settings()).clone();
            let playlist = Target::playlist(id.0.to_string());
            settings
                .set_in(INCLUDE, &playlist, Value::Bool(false))
                .unwrap();
            host.settings(&settings);
            host.import(&source, Some(id), false).await;
            provider.assert_requests(false);
            // Removing the override makes the same source inherit the global disable.
            settings.clear_in(INCLUDE, &playlist).unwrap();
            settings.set(INCLUDE, Value::Bool(false)).unwrap();
            host.settings(&settings);
            host.import(&source, Some(id), false).await;
            provider.assert_requests(false);
            // A playlist's explicit enable must win over the global disable too.
            settings
                .set_in(INCLUDE, &playlist, Value::Bool(true))
                .unwrap();
            host.settings(&settings);
            host.import(&source, Some(id), true).await;
            provider.assert_requests(true);
            let enabled = iptv_library::SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            };
            data::set_policy(&host.owner, id, enabled).await.unwrap();
            let previous = host.publications.get();
            data::switch_all(&host.owner);
            host.discovery_after(previous, true).await;
            if let Some(finished) = host.owner.pending_import_completion(id) {
                tokio::time::timeout(Duration::from_secs(20), finished)
                    .await
                    .unwrap();
            }
            let previous = host.publications.get();
            data::set_policy(
                &host.owner,
                id,
                iptv_library::SourcePolicy {
                    enabled: false,
                    ..enabled
                },
            )
            .await
            .unwrap();
            host.discovery_after(previous, false).await;
            // Disabling hides discoverability without deleting the imported rows.
            let store = Store::open_encrypted(
                host.directory.path().join("iptv.sqlite"),
                &DatabaseKey::from_bytes(KEY),
            )
            .unwrap();
            assert_eq!(store.import_status(id).unwrap().counts.vod, 2);
            drop(store);
            let previous = host.publications.get();
            data::set_policy(&host.owner, id, enabled).await.unwrap();
            host.discovery_after(previous, true).await;
            if let Some(finished) = host.owner.pending_import_completion(id) {
                tokio::time::timeout(Duration::from_secs(20), finished)
                    .await
                    .unwrap();
            }
            let previous = host.publications.get();
            data::delete(&host.owner, id).await.unwrap();
            host.discovery_after(previous, false).await;
            assert!(host.owner.playlists().unwrap().is_empty());
        })
        .await;
}

/// The rail's cross-source shelves follow every enabled playlist: a
/// live-only playlist on screen still reaches another playlist's films,
/// while its own Movies and Shows entries stay hidden.
#[tokio::test(flavor = "current_thread")]
async fn a_live_only_playlist_on_screen_still_offers_the_cross_source_shelves() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let provider = Provider::new();
            let host = Host::new();
            let films = provider.source();
            let previous = host.publications.get();
            let with_films = host.import(&films, None, true).await;
            provider.assert_requests(true);
            host.availability_after(previous, true, true).await;
            // A second playlist imports live channels only and is shown alone.
            let mut settings = (*host.owner.settings()).clone();
            settings.set(INCLUDE, Value::Bool(false)).unwrap();
            host.settings(&settings);
            let live_only = Source {
                name: "Generated live only".into(),
                ..provider.source()
            };
            let previous = host.publications.get();
            let shown = host.import(&live_only, None, false).await;
            provider.assert_requests(false);
            assert_ne!(shown, with_films);
            assert!(!data::combined::active(&host.owner));
            assert_eq!(host.owner.shown_source(), Some(shown));
            host.availability_after(previous, false, true).await;
            // The films' playlist itself still lists all four entries (shown
            // from storage: no refresh with the now global live-only
            // preference).
            data::set_policy(
                &host.owner,
                with_films,
                iptv_library::SourcePolicy {
                    refresh_on_start: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            let previous = host.publications.get();
            data::library::switch_to(&host.owner, with_films);
            host.availability_after(previous, true, true).await;
            if let Some(finished) = host.owner.pending_import_completion(with_films) {
                tokio::time::timeout(Duration::from_secs(20), finished)
                    .await
                    .unwrap();
            }
        })
        .await;
}

/// Hidden-source policy and background imports update only the shelves: the
/// live-only guide on screen retains its source, catalogue and channel identity.
#[tokio::test(flavor = "current_thread")]
async fn hidden_playlist_mutations_update_shelves_without_republishing_the_guide() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let provider = Provider::new();
            let host = Host::new();
            let previous = host.publications.get();
            let hidden = host.import(&provider.source(), None, true).await;
            provider.assert_requests(true);
            host.availability_after(previous, true, true).await;
            let enabled = iptv_library::SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            };
            data::set_policy(&host.owner, hidden, enabled)
                .await
                .unwrap();
            let mut settings = (*host.owner.settings()).clone();
            settings.set(INCLUDE, Value::Bool(false)).unwrap();
            host.settings(&settings);
            let previous = host.publications.get();
            let shown = host
                .import(
                    &Source {
                        name: "Generated live-only anchor".into(),
                        ..provider.source()
                    },
                    None,
                    false,
                )
                .await;
            provider.assert_requests(false);
            // Adding a second enabled playlist automatically joins both. Wait
            // for that real transition, then explicitly choose the live-only
            // playlist before observing hidden-source-only mutations.
            host.discovery_after(previous, true).await;
            data::set_policy(&host.owner, shown, enabled).await.unwrap();
            let previous = host.publications.get();
            data::library::switch_to(&host.owner, shown);
            host.availability_after(previous, false, true).await;
            assert_ne!(hidden, shown);
            assert!(!data::combined::active(&host.owner));
            assert_eq!(host.owner.shown_source(), Some(shown));
            assert_eq!(host.has_vod.get(), Some(false));
            assert_eq!(host.any_vod.get(), Some(true));
            let catalogue = host.owner.schedule();
            let channel = catalogue.channel_key(0);
            let publications = host.publications.get();
            let unchanged = || {
                assert_eq!(host.owner.shown_source(), Some(shown));
                assert!(!data::combined::active(&host.owner));
                assert!(Rc::ptr_eq(&catalogue, &host.owner.schedule()));
                assert_eq!(host.owner.schedule().channel_key(0), channel);
                assert_eq!(host.publications.get(), publications);
                assert_eq!(host.has_vod.get(), Some(false));
            };
            data::set_policy(
                &host.owner,
                hidden,
                iptv_library::SourcePolicy {
                    enabled: false,
                    ..enabled
                },
            )
            .await
            .unwrap();
            assert_eq!(host.any_vod.get(), Some(false), "disable is awaited");
            unchanged();
            data::set_policy(&host.owner, hidden, enabled)
                .await
                .unwrap();
            assert_eq!(host.any_vod.get(), Some(true), "enable is awaited");
            unchanged();
            // A normal hidden-source import removes its old on-demand catalogue.
            data::library::refresh(&host.owner, hidden);
            tokio::time::timeout(Duration::from_secs(20), async {
                while host.any_vod.get() != Some(false) {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .expect("completed background import did not remove shelves");
            provider.assert_requests(false);
            unchanged();
            // A later background import adds films without switching the guide.
            settings
                .set_in(
                    INCLUDE,
                    &Target::playlist(hidden.0.to_string()),
                    Value::Bool(true),
                )
                .unwrap();
            host.settings(&settings);
            data::library::refresh(&host.owner, hidden);
            tokio::time::timeout(Duration::from_secs(20), async {
                while host.any_vod.get() != Some(true) {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .expect("completed background import did not add shelves");
            provider.assert_requests(true);
            unchanged();
            data::delete(&host.owner, hidden).await.unwrap();
            assert_eq!(host.any_vod.get(), Some(false), "delete is awaited");
            unchanged();
        })
        .await;
}
