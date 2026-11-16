//! Synthetic direct-ID access, source fences and durable watch progress.
#[path = "../../iptv-store-sqlite/tests/common/mod.rs"]
mod common;
mod support;
use iptv_core::{
    parental::{Pin, Target},
    vod::{Key, Kind, Progress, Query},
};
use iptv_providers::model::StreamLocator;
use iptv_runtime::{Ports, Runtime};
use iptv_settings::Value;
use std::rc::Rc;
#[test]
fn movie_playback_progress_survives_restart_and_locked_direct_ids_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let database_key = iptv_runtime::model::DatabaseKey::from_bytes([0x63; 32]);
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(dir.path().join("iptv.sqlite"), &database_key)
            .unwrap();
    let source = store
        .add_source(&common::m3u_source("Generated on-demand source"))
        .unwrap();
    let mut catalog = common::catalog(1, 1, common::XT_PASSWORD);
    catalog.vod[0].stream = StreamLocator::Url {
        url: "https://example.test/generated-movie.mp4".into(),
        headers: vec![("X-Generated".into(), "fixture".into())],
        properties: vec![],
    };
    common::replace(&mut store, source, &catalog);
    drop(store);
    let key = Key {
        source: source.0,
        kind: Kind::Movie,
        id: catalog.vod[0].id.as_str().into(),
    };
    let make = || {
        Rc::new(Runtime::new(Ports::new(dir.path().to_owned(), {
            let key = database_key.clone();
            move |_| Ok(key.clone())
        })))
    };
    let owner = make();
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner.start_services();
    let details = support::block_on(owner.vod_details(key.clone())).unwrap();
    let (playing, stream) =
        support::block_on(owner.vod_play(details.clone(), key.clone(), false)).unwrap();
    assert_eq!(playing.key, key);
    assert_eq!(stream.url, "https://example.test/generated-movie.mp4");
    assert_eq!(stream.header("X-Generated"), Some("fixture"));
    let incarnation = details.incarnation.clone();
    let point = Progress::observed(37_000, Some(120_000), false);
    support::block_on(owner.vod_save_progress(key.clone(), None, point, incarnation)).unwrap();
    assert!(owner.drain_services());
    drop(owner);
    let owner = make();
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner.start_services();
    let details = support::block_on(owner.vod_details(key.clone())).unwrap();
    assert_eq!(details.item.progress, point);
    let (playing, _) =
        support::block_on(owner.vod_play(details.clone(), key.clone(), false)).unwrap();
    assert_eq!(playing.progress.resume_ms(), 37_000);
    let (playing, _) =
        support::block_on(owner.vod_play(details.clone(), key.clone(), true)).unwrap();
    assert_eq!(playing.progress.resume_ms(), 0);
    support::block_on(owner.parental_change_pin(None, Pin::parse("2468").unwrap())).unwrap();
    owner
        .set_setting("parental.vod_enabled", Value::Bool(true))
        .unwrap();
    let target = iptv_runtime::vod::target(&key);
    support::block_on(owner.parental_set_blocked(target.clone(), true)).unwrap();
    owner.parental_lock();
    assert!(support::block_on(owner.vod_details(key.clone())).is_err());
    assert!(support::block_on(owner.vod_play(details.clone(), key.clone(), false)).is_err());
    support::block_on(owner.parental_authorize(target, Pin::parse("2468").unwrap())).unwrap();
    assert!(support::block_on(owner.vod_play(details, key.clone(), false)).is_ok());
    let (cards, _, _, _) = support::block_on(owner.vod_catalogue(
        Query {
            continue_watching: true,
            ..Default::default()
        },
        0,
    ))
    .unwrap();
    assert!(cards.iter().any(|c| c.key == key));
    support::block_on(owner.parental_authorize(Target::settings(), Pin::parse("2468").unwrap()))
        .unwrap();
    owner
        .set_setting("parental.vod_enabled", Value::Bool(false))
        .unwrap();
    assert!(owner.drain_services());
}

/// Two playlists carrying the same generated catalogue: identical provider
/// ids and titles, playable through local direct links.
fn two_playlists(
    dir: &std::path::Path,
    database_key: &iptv_runtime::model::DatabaseKey,
) -> (Vec<iptv_store_sqlite::SourceId>, common::Catalog) {
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(dir.join("iptv.sqlite"), database_key).unwrap();
    let mut catalog = common::catalog(1, 1, common::XT_PASSWORD);
    for (index, movie) in catalog.vod.iter_mut().enumerate() {
        movie.poster = Some(format!("https://example.test/generated-poster-{index}.png"));
        movie.stream = StreamLocator::Url {
            url: format!("https://example.test/generated-movie-{index}.mp4"),
            headers: vec![],
            properties: vec![],
        };
    }
    let sources = ["Generated alpha", "Generated beta"]
        .into_iter()
        .map(|name| {
            let source = store.add_source(&common::m3u_source(name)).unwrap();
            common::replace(&mut store, source, &catalog);
            source
        })
        .collect();
    (sources, catalog)
}

#[test]
fn discovery_reaches_unfinished_titles_on_every_playlist_and_resumes_them() {
    use iptv_core::vod::{DiscoveryQuery, Shelf};
    let dir = tempfile::tempdir().unwrap();
    let database_key = iptv_runtime::model::DatabaseKey::from_bytes([0x72; 32]);
    let (sources, catalog) = two_playlists(dir.path(), &database_key);
    let owner = Rc::new(Runtime::new(Ports::new(dir.path().to_owned(), {
        let key = database_key.clone();
        move |_| Ok(key.clone())
    })));
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner.start_services();
    let movie = |source: &iptv_store_sqlite::SourceId| Key {
        source: source.0,
        kind: Kind::Movie,
        id: catalog.vod[1].id.to_string(),
    };
    let (alpha, beta) = (movie(&sources[0]), movie(&sources[1]));
    assert!(
        support::block_on(owner.vod_discovery(DiscoveryQuery::first(Shelf::ContinueWatching)))
            .unwrap()
            .cards
            .is_empty()
    );
    // Watch the same-named title on both playlists, to different points.
    for (key, ms) in [(&alpha, 37_000), (&beta, 61_000)] {
        let details = support::block_on(owner.vod_details(key.clone())).unwrap();
        support::block_on(owner.vod_save_progress(
            key.clone(),
            None,
            Progress::observed(ms, Some(120_000), false),
            details.incarnation,
        ))
        .unwrap();
    }
    let page =
        support::block_on(owner.vod_discovery(DiscoveryQuery::first(Shelf::ContinueWatching)))
            .unwrap();
    assert!(!page.more);
    let keys: Vec<_> = page.cards.iter().map(|c| c.key.clone()).collect();
    assert_eq!(
        keys.len(),
        2,
        "duplicate ids on two playlists stay separate"
    );
    assert!(keys.contains(&alpha) && keys.contains(&beta));
    let names: Vec<_> = page.cards.iter().map(|c| c.source_name.as_str()).collect();
    assert!(names.contains(&"Generated alpha") && names.contains(&"Generated beta"));
    // Discovery → details → resume, for the second playlist's copy.
    let card = page.cards.iter().find(|c| c.key == beta).unwrap();
    assert_eq!(card.progress.resume_ms(), 61_000);
    let details = support::block_on(owner.vod_details(card.key.clone())).unwrap();
    let (playing, stream) =
        support::block_on(owner.vod_play(details, card.key.clone(), false)).unwrap();
    assert_eq!(playing.key, beta);
    assert_eq!(playing.progress.resume_ms(), 61_000);
    assert_eq!(stream.url, "https://example.test/generated-movie-1.mp4");
    // Recently added lists every title of both playlists, movies and the
    // series together, a bounded page at a time.
    let mut recent = Vec::new();
    loop {
        let page = support::block_on(owner.vod_discovery(DiscoveryQuery {
            limit: 3,
            ..DiscoveryQuery::after(Shelf::RecentlyAdded, recent.len())
        }))
        .unwrap();
        assert!(page.cards.len() <= 3);
        recent.extend(page.cards.into_iter().map(|c| c.key));
        if !page.more {
            break;
        }
    }
    assert_eq!(recent.len(), 2 * (catalog.vod.len() + catalog.series.len()));
    assert!(recent.iter().any(|k| k.kind == Kind::Series));
    assert_eq!(
        recent[0].kind,
        Kind::Series,
        "the generated series' last-modified time is the newest"
    );
    // A protected title keeps its place but not its artwork or plot, and
    // cannot be opened; the other playlist's copy is untouched.
    support::block_on(owner.parental_change_pin(None, Pin::parse("2468").unwrap())).unwrap();
    owner
        .set_setting("parental.vod_enabled", Value::Bool(true))
        .unwrap();
    support::block_on(owner.parental_set_blocked(iptv_runtime::vod::target(&alpha), true)).unwrap();
    owner.parental_lock();
    for shelf in [Shelf::ContinueWatching, Shelf::RecentlyAdded] {
        let page = support::block_on(owner.vod_discovery(DiscoveryQuery {
            limit: 100,
            ..DiscoveryQuery::first(shelf)
        }))
        .unwrap();
        let locked = page.cards.iter().find(|c| c.key == alpha).unwrap();
        assert!(
            locked.poster.is_none() && locked.plot.is_none(),
            "{shelf:?}"
        );
        let open = page.cards.iter().find(|c| c.key == beta).unwrap();
        assert!(open.poster.is_some() && open.plot.is_some(), "{shelf:?}");
    }
    assert!(support::block_on(owner.vod_details(alpha.clone())).is_err());
    assert!(support::block_on(owner.vod_details(beta.clone())).is_ok());
    assert!(owner.drain_services());
}

#[test]
fn discovery_drops_a_deleted_playlist_and_refuses_its_stale_progress() {
    use iptv_core::vod::{DiscoveryQuery, Shelf};
    let dir = tempfile::tempdir().unwrap();
    let database_key = iptv_runtime::model::DatabaseKey::from_bytes([0x73; 32]);
    let (sources, catalog) = two_playlists(dir.path(), &database_key);
    let make = || {
        let owner = Rc::new(Runtime::new(Ports::new(dir.path().to_owned(), {
            let key = database_key.clone();
            move |_| Ok(key.clone())
        })));
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        owner
    };
    let owner = make();
    let keys: Vec<_> = sources
        .iter()
        .map(|source| Key {
            source: source.0,
            kind: Kind::Movie,
            id: catalog.vod[0].id.to_string(),
        })
        .collect();
    let point = Progress::observed(40_000, Some(120_000), false);
    let mut incarnations = Vec::new();
    for key in &keys {
        let details = support::block_on(owner.vod_details(key.clone())).unwrap();
        support::block_on(owner.vod_save_progress(
            key.clone(),
            None,
            point,
            details.incarnation.clone(),
        ))
        .unwrap();
        incarnations.push(details.incarnation);
    }
    assert!(owner.drain_services());
    drop(owner);
    // The second playlist is deleted and added again under the same id.
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(dir.path().join("iptv.sqlite"), &database_key)
            .unwrap();
    let source = store.source(sources[1]).unwrap().unwrap().source;
    store.delete_source(sources[1]).unwrap();
    let recreated = store.add_source(&source).unwrap();
    assert_eq!(recreated, sources[1]);
    common::replace(&mut store, recreated, &catalog);
    drop(store);
    let owner = make();
    let continuing = |owner: &Runtime| {
        support::block_on(owner.vod_discovery(DiscoveryQuery::first(Shelf::ContinueWatching)))
            .unwrap()
            .cards
            .into_iter()
            .map(|c| c.key)
            .collect::<Vec<_>>()
    };
    assert_eq!(continuing(&owner), vec![keys[0].clone()]);
    // A late writer from before the deletion cannot bring the old progress
    // back to the new playlist, whether awaited or queued.
    assert!(
        support::block_on(owner.vod_save_progress(
            keys[1].clone(),
            None,
            point,
            incarnations[1].clone()
        ))
        .is_err()
    );
    owner.queue_vod_progress(keys[1].clone(), None, point, incarnations[1].clone());
    assert!(owner.drain_services());
    drop(owner);
    let owner = make();
    assert_eq!(continuing(&owner), vec![keys[0].clone()]);
    assert!(owner.drain_services());
    drop(owner);
    // Disabling the remaining playlist removes its titles from both shelves
    // and from details, though its progress is kept for when it returns.
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(dir.path().join("iptv.sqlite"), &database_key)
            .unwrap();
    store
        .set_source_policy(
            sources[0],
            iptv_store_sqlite::SourcePolicy {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
    drop(store);
    let owner = make();
    assert!(continuing(&owner).is_empty());
    let recent =
        support::block_on(owner.vod_discovery(DiscoveryQuery::first(Shelf::RecentlyAdded)))
            .unwrap();
    assert!(recent.cards.iter().all(|c| c.key.source == sources[1].0));
    assert!(support::block_on(owner.vod_details(keys[0].clone())).is_err());
    assert!(owner.drain_services());
}

#[test]
fn stale_observation_cannot_write_to_a_reused_source_id() {
    let dir = tempfile::tempdir().unwrap();
    let database_key = iptv_runtime::model::DatabaseKey::from_bytes([0x64; 32]);
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(dir.path().join("iptv.sqlite"), &database_key)
            .unwrap();
    let source_data = common::m3u_source("Generated reused source");
    let source = store.add_source(&source_data).unwrap();
    let catalog = common::catalog(1, 1, common::XT_PASSWORD);
    common::replace(&mut store, source, &catalog);
    let key = Key {
        source: source.0,
        kind: Kind::Movie,
        id: catalog.vod[0].id.to_string(),
    };
    let old = store.source_incarnation(source).unwrap();
    store.delete_source(source).unwrap();
    let replacement = store.add_source(&source_data).unwrap();
    assert_eq!(source, replacement);
    common::replace(&mut store, replacement, &catalog);
    drop(store);
    let owner = Rc::new(Runtime::new(Ports::new(dir.path().to_owned(), move |_| {
        Ok(database_key.clone())
    })));
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner.start_services();
    let point = Progress::observed(40_000, Some(100_000), false);
    assert!(
        support::block_on(owner.vod_save_progress(key.clone(), None, point, old.clone())).is_err()
    );
    owner.queue_vod_progress(key.clone(), None, point, old);
    assert!(owner.drain_services());
    let key = iptv_runtime::model::DatabaseKey::from_bytes([0x64; 32]);
    let store =
        iptv_store_sqlite::Store::open_encrypted(dir.path().join("iptv.sqlite"), &key).unwrap();
    assert_eq!(
        store
            .vod_progress(&iptv_core::vod::Key {
                source: source.0,
                kind: Kind::Movie,
                id: catalog.vod[0].id.to_string()
            })
            .unwrap(),
        Progress::default()
    );
}

#[cfg(feature = "test-support")]
#[test]
fn an_out_of_order_episode_resumes_after_restart_and_a_stale_writer_cannot_restore_its_cursor() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::Duration,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let body = serde_json::json!({
        "info": {"name": "Generated out-of-order series"},
        "episodes": (1..=6).map(|number| serde_json::json!({
            "id":1000+number,"episode_num":number,"title":format!("Generated episode {number}"),
            "season":if number<5 {1} else {2},"container_extension":"mp4"
        })).collect::<Vec<_>>()
    })
    .to_string();
    let provider = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                assert!(request.len() < 16_384);
            }
            assert!(
                String::from_utf8(request)
                    .unwrap()
                    .contains("action=get_series_info")
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iptv.sqlite");
    let database_key = iptv_runtime::model::DatabaseKey::from_bytes([0x75; 32]);
    let mut store = iptv_store_sqlite::Store::open_encrypted(&path, &database_key).unwrap();
    let mut source_data = common::xtream_source("Generated out-of-order source");
    let iptv_providers::model::SourceConfig::Xtream(credentials) = &mut source_data.config else {
        unreachable!()
    };
    credentials.server_url = format!("http://{address}");
    let source = store.add_source(&source_data).unwrap();
    let catalogue = common::catalog(1, 1, common::XT_PASSWORD);
    common::replace(&mut store, source, &catalogue);
    let series = Key {
        source: source.0,
        kind: Kind::Series,
        id: catalogue.series[0].id.to_string(),
    };
    drop(store);
    let make = || {
        let owner = Rc::new(Runtime::new(Ports::new(directory.path().to_owned(), {
            let database_key = database_key.clone();
            move |_| Ok(database_key.clone())
        })));
        owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
        owner.start_services();
        owner
    };
    let owner = make();
    let details = support::block_on(owner.vod_details(series.clone())).unwrap();
    let fifth = details.episodes[4].key.clone();
    let old_incarnation = details.incarnation.clone();
    let point = Progress::observed(37_000, Some(120_000), false);
    support::block_on(owner.vod_save_progress(
        fifth.clone(),
        Some(series.clone()),
        point,
        old_incarnation.clone(),
    ))
    .unwrap();
    assert!(owner.drain_services());
    drop(owner);
    let owner = make();
    let details = support::block_on(owner.vod_details(series.clone())).unwrap();
    assert_eq!(details.resume_episode, Some(fifth.clone()));
    let mut screen = iptv_core::vod::Screen::default();
    screen.show_details(details.clone());
    assert_eq!(screen.episode, Some(fifth.clone()));
    assert_eq!(screen.season, Some(2));
    let (playing, _) = support::block_on(owner.vod_play(details, fifth.clone(), false)).unwrap();
    assert_eq!(playing.key, fifth);
    assert_eq!(playing.progress.resume_ms(), 37_000);
    provider.join().unwrap();
    assert!(owner.drain_services());
    let mut store = iptv_store_sqlite::Store::open_encrypted(&path, &database_key).unwrap();
    store.delete_source(source).unwrap();
    let replacement = store.add_source(&source_data).unwrap();
    assert_eq!(replacement, source);
    common::replace(&mut store, replacement, &catalogue);
    drop(store);
    assert!(
        support::block_on(owner.vod_save_progress(
            fifth,
            Some(series.clone()),
            point,
            old_incarnation
        ))
        .is_err()
    );
    assert!(owner.drain_services());
    let store = iptv_store_sqlite::Store::open_encrypted(path, &database_key).unwrap();
    assert_eq!(store.vod_resume_episode(&series).unwrap(), None);
}

#[cfg(feature = "test-support")]
#[test]
fn xtream_movies_keep_the_configured_user_agent_for_direct_and_generated_urls() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::Duration,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let provider = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
            assert!(request.len() < 16_384);
        }
        assert!(
            String::from_utf8(request)
                .unwrap()
                .contains("action=get_vod_info")
        );
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let database_key = iptv_runtime::model::DatabaseKey::from_bytes([0x79; 32]);
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(dir.path().join("iptv.sqlite"), &database_key)
            .unwrap();
    let mut source_data = common::xtream_source("Generated source headers");
    let iptv_providers::model::SourceConfig::Xtream(credentials) = &mut source_data.config else {
        unreachable!()
    };
    credentials.server_url = format!("http://{address}");
    let source = store.add_source(&source_data).unwrap();
    let mut catalog = common::catalog(1, 1, common::XT_PASSWORD);
    // An imported direct_source should keep source defaults, while explicit
    // per-item headers take precedence over those defaults.
    for (index, headers) in [
        vec![("X-Generated".into(), "fixture".into())],
        vec![("user-agent".into(), "Generated-Item/2.0".into())],
    ]
    .into_iter()
    .enumerate()
    {
        catalog.vod[index].stream = StreamLocator::Url {
            url: format!("https://media.example.test/generated-{index}.mp4"),
            headers,
            properties: vec![],
        };
    }
    common::replace(&mut store, source, &catalog);
    drop(store);
    let owner = Rc::new(Runtime::new(Ports::new(dir.path().to_owned(), move |_| {
        Ok(database_key.clone())
    })));
    owner.bind_tasks(drop, |_| Box::pin(std::future::pending()));
    owner.start_services();
    for (index, expected_agent) in ["Synthetic/1.0", "Generated-Item/2.0", "Synthetic/1.0"]
        .into_iter()
        .enumerate()
    {
        let key = Key {
            source: source.0,
            kind: Kind::Movie,
            id: catalog.vod[index].id.to_string(),
        };
        let details = support::block_on(owner.vod_details(key.clone())).unwrap();
        let (_, stream) = support::block_on(owner.vod_play(details, key, false)).unwrap();
        assert_eq!(stream.header("User-Agent"), Some(expected_agent));
        if index == 0 {
            assert_eq!(stream.header("X-Generated"), Some("fixture"));
        }
        assert_eq!(stream.public_only, index != 2);
    }
    provider.join().unwrap();
    assert!(owner.drain_services());
}
