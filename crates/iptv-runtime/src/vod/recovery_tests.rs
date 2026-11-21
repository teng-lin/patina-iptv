//! Native portal episode coordinates, bounded discovery and stale detail admission.
use super::*;
use iptv_providers::{
    ContentId,
    model::{MacAddress, Source, StalkerCredentials, VodItem},
    stalker::{PortalMode, StalkerPortal, StalkerSession},
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

struct Provider {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: Option<std::thread::JoinHandle<()>>,
    finished: std::sync::mpsc::Receiver<()>,
}
impl Provider {
    fn new(responses: Vec<&'static str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let (done, finished) = std::sync::mpsc::channel();
        let task = std::thread::spawn(move || {
            for body in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 16_384);
                }
                seen.lock().unwrap().push(
                    String::from_utf8(request)
                        .unwrap()
                        .lines()
                        .next()
                        .unwrap()
                        .to_owned(),
                );
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            let _ = done.send(());
        });
        Self {
            url,
            requests,
            task: Some(task),
            finished,
        }
    }
    fn finish(&mut self) -> Vec<String> {
        self.finished
            .recv_timeout(Duration::from_secs(5))
            .expect("provider fixture did not finish");
        self.task.take().unwrap().join().unwrap();
        self.requests.lock().unwrap().clone()
    }
}

fn fixture(url: &str, embedded: Vec<u32>) -> (tempfile::TempDir, Rc<Runtime>, Key) {
    let directory = tempfile::tempdir().unwrap();
    let key = crate::model::DatabaseKey::from_bytes([0x79; 32]);
    let credentials = StalkerCredentials {
        portal_url: format!("{url}/c/"),
        mac: MacAddress::parse("00:1A:79:00:00:79").unwrap(),
        serial: None,
        device_id: None,
        device_id2: None,
        signature: None,
        signature2: None,
        login: None,
        password: None,
    };
    let config = SourceConfig::Stalker(credentials.clone());
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(directory.path().join("iptv.sqlite"), &key)
            .unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated series source".into(),
            config: config.clone(),
        })
        .unwrap();
    let native = embedded.is_empty();
    let series = VodItem {
        id: ContentId::from_stored("generated-series"),
        provider_id: Some("79".into()),
        name: "Generated series".into(),
        groups: vec![],
        poster: None,
        plot: None,
        year: None,
        rating_x10: None,
        added: None,
        duration_secs: None,
        stream: StreamLocator::Stalker {
            cmd: "ffmpeg /media/generated-file".into(),
            link: Default::default(),
        },
        is_series: true,
        episode_numbers: embedded,
    };
    if native {
        let item = iptv_providers::model::Series {
            id: series.id.clone(),
            provider_id: series.provider_id.clone(),
            name: series.name.clone(),
            groups: vec![],
            poster: None,
            plot: None,
            year: None,
            rating_x10: None,
            last_modified: None,
        };
        store
            .replace_catalog(source, &[], &[], &[], &[item])
            .unwrap();
    } else {
        store
            .replace_catalog(source, &[], &[], &[series], &[])
            .unwrap();
    }
    drop(store);
    let owner = Rc::new(Runtime::new(crate::Ports::new(
        directory.path().into(),
        move |_| Ok(key.clone()),
    )));
    owner.start_services();
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    let portal = StalkerPortal::from_credentials(
        &credentials,
        &iptv_providers::stalker::endpoint_candidates(&credentials.portal_url).unwrap()[0],
        PortalMode::Simple,
    )
    .unwrap();
    let session = StalkerSession::new(portal);
    owner
        .vod
        .sessions
        .lock()
        .unwrap()
        .insert(source.0, (config, session.clone()));
    let schedule = iptv_guide::EpgSchedule::new(vec![], Default::default(), 0).with_playback(Some(
        iptv_guide::playback::PlaybackContext::stalker(session),
    ));
    owner.parts.borrow_mut().insert(source, Rc::new(schedule));
    (
        directory,
        owner,
        Key {
            source: source.0,
            kind: Kind::Series,
            id: "generated-series".into(),
        },
    )
}

#[test]
fn embedded_stalker_episodes_keep_coordinates_and_reject_forged_or_stale_details() {
    let mut provider = Provider::new(vec![
        r#"{"js":{"cmd":"ffmpeg https://example.test/generated-episode.ts"}}"#,
    ]);
    let (_directory, owner, key) = fixture(&provider.url, vec![2, 4]);
    let details = owner
        .services()
        .block_on(owner.vod_details(key.clone()))
        .unwrap();
    assert_eq!(
        details
            .episodes
            .iter()
            .map(|e| e.number)
            .collect::<Vec<_>>(),
        [2, 4]
    );
    assert!(
        provider.requests.lock().unwrap().is_empty(),
        "embedded discovery must not open another provider stream"
    );
    let episode = details.episodes[1].key.clone();
    let mut forged = details.clone();
    forged.incarnation = "generated-obsolete-incarnation".into();
    assert!(
        owner
            .services()
            .block_on(owner.vod_play(forged, episode.clone(), false))
            .is_err()
    );
    let (playing, stream) = owner
        .services()
        .block_on(owner.vod_play(details.clone(), episode.clone(), false))
        .unwrap();
    assert_eq!(playing.parent, Some(key.clone()));
    assert_eq!(playing.details.resume_episode, Some(episode.clone()));
    assert_eq!(stream.url, "https://example.test/generated-episode.ts");
    let requests = provider.finish();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("action=create_link") && requests[0].contains("series=4"));
    let progress = Progress::observed(15_000, Some(60_000), false);
    owner
        .services()
        .block_on(owner.vod_save_progress(
            episode.clone(),
            Some(key.clone()),
            progress,
            details.incarnation.clone(),
        ))
        .unwrap();
    // Neither an absent episode parent nor another source's parent is authority.
    assert!(
        owner
            .services()
            .block_on(owner.vod_save_progress(
                episode.clone(),
                None,
                Progress::default(),
                details.incarnation.clone()
            ))
            .is_err()
    );
    let mut wrong_parent = key.clone();
    wrong_parent.source += 1;
    assert!(
        owner
            .services()
            .block_on(owner.vod_save_progress(
                episode,
                Some(wrong_parent),
                Progress::default(),
                details.incarnation.clone()
            ))
            .is_err()
    );
    let repeated = owner.services().block_on(owner.vod_details(key)).unwrap();
    assert_eq!(repeated.episodes[1].progress, progress);
    assert!(owner.drain_services());
}

#[test]
fn native_stalker_season_pages_end_on_empty_and_preserve_opaque_commands() {
    let mut provider = Provider::new(vec![
        r#"{"js":[{"id":"season-three","video_id":"generated-video","is_season":true,"season_number":3}]}"#,
        r#"{"js":[{"id":"opaque-four","name":"Generated episode four","is_episode":true,"episode_num":4,"cmd":"ffmpeg /media/generated-opaque"}]}"#,
        r#"{"js":[]}"#,
        r#"{"js":[]}"#,
        r#"{"js":{"cmd":"ffmpeg https://example.test/generated-four.ts"}}"#,
    ]);
    let (_directory, owner, key) = fixture(&provider.url, vec![]);
    let details = owner
        .services()
        .block_on(owner.vod_details(key.clone()))
        .unwrap();
    assert_eq!(details.episodes.len(), 1);
    assert_eq!(
        (details.episodes[0].season, details.episodes[0].number),
        (3, 4)
    );
    let (playing, stream) = owner
        .services()
        .block_on(owner.vod_play(details.clone(), details.episodes[0].key.clone(), false))
        .unwrap();
    assert_eq!(playing.parent, Some(key));
    assert_eq!(stream.url, "https://example.test/generated-four.ts");
    let requests = provider.finish();
    assert_eq!(
        requests.len(),
        5,
        "discovery must stop at the acknowledged empty pages"
    );
    assert!(requests[1].contains("season_id=season-three"));
    assert!(requests[1].contains("movie_id=generated-video"));
    assert!(requests[4].contains("series=4"));
    assert!(requests[4].contains("generated-opaque"));
    assert!(owner.drain_services());
}

#[test]
fn repeated_native_pages_stop_when_no_new_episode_identity_was_discovered() {
    let body = r#"{"js":[{"id":"generated-file","name":"Generated season two","cmd":"ffmpeg /media/generated-file","season_number":2,"series":[2,4]}]}"#;
    let mut provider = Provider::new(vec![body, body]);
    let (_directory, owner, key) = fixture(&provider.url, vec![]);
    let details = owner.services().block_on(owner.vod_details(key)).unwrap();
    assert_eq!(
        details
            .episodes
            .iter()
            .map(|e| (e.season, e.number))
            .collect::<Vec<_>>(),
        [(2, 2), (2, 4)]
    );
    assert_eq!(
        provider.finish().len(),
        2,
        "a repeating portal page must not duplicate episodes or spend the full request budget"
    );
    assert!(owner.drain_services());
}

#[test]
fn canceling_a_focused_preview_closes_its_pending_provider_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let cancel = iptv_import::CancelToken::new();
    let interrupt = cancel.clone();
    let (closed, observed) = std::sync::mpsc::channel();
    let task = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
            assert!(header.len() < 16_384);
        }
        interrupt.cancel();
        let mut byte = [0];
        let ended = match stream.read(&mut byte) {
            Ok(0) => true,
            Err(e) => matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            ),
            _ => false,
        };
        closed.send(ended).unwrap();
    });
    let (_directory, owner, key) = fixture(&url, vec![]);
    assert!(
        owner
            .services()
            .block_on(owner.vod_preview(key, cancel))
            .is_err()
    );
    assert!(
        observed.recv_timeout(Duration::from_secs(6)).unwrap(),
        "cancelled preview retained its provider connection"
    );
    task.join().unwrap();
    assert!(owner.vod.entries.lock().unwrap().is_empty());
}

#[test]
fn focused_movie_preview_enriches_missing_metadata_without_registering_playback() {
    let mut provider = Provider::new(vec![concat!(
        r#"{"info":{"plot":"Generated detailed plot","releasedate":"2024-02-01","rating":"8.2","duration_secs":5400},"#,
        r#""movie_data":{"stream_id":1,"name":"Generated movie","container_extension":"mp4"}}"#,
    )]);
    let directory = tempfile::tempdir().unwrap();
    let database_key = crate::model::DatabaseKey::from_bytes([0x79; 32]);
    let mut store = iptv_store_sqlite::Store::open_encrypted(
        directory.path().join("iptv.sqlite"),
        &database_key,
    )
    .unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated preview source".into(),
            config: SourceConfig::Xtream(iptv_providers::model::XtreamCredentials {
                server_url: provider.url.clone(),
                username: iptv_providers::Secret::new("generated"),
                password: iptv_providers::Secret::new("generated"),
                user_agent: None,
            }),
        })
        .unwrap();
    let movies = iptv_providers::xtream::parse_vod_streams(&iptv_providers::HttpResponse::ok(
        r#"[{"stream_id":1,"name":"Generated movie","container_extension":"mp4"}]"#,
    ))
    .unwrap();
    let key = Key {
        source: source.0,
        kind: Kind::Movie,
        id: movies.items[0].id.as_str().into(),
    };
    store
        .replace_catalog(source, &[], &[], &movies.items, &[])
        .unwrap();
    drop(store);
    let owner = Rc::new(Runtime::new(crate::Ports::new(
        directory.path().into(),
        move |_| Ok(database_key.clone()),
    )));
    owner.start_services();
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    let details = owner
        .services()
        .block_on(owner.vod_preview(key, iptv_import::CancelToken::new()))
        .unwrap();
    assert_eq!(details.item.year, Some(2024));
    assert_eq!(details.item.rating_x10, Some(82));
    assert_eq!(details.item.duration_secs, Some(5400));
    assert_eq!(
        details.item.plot.as_deref(),
        Some("Generated detailed plot")
    );
    assert!(owner.vod.entries.lock().unwrap().is_empty());
    assert_eq!(provider.finish().len(), 1);
}
