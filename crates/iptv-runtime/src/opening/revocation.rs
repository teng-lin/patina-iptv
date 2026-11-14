//! Real-store ordering checks; the held continuation belongs to this opening.
use super::*;
use iptv_epg::{EpgRef, MatchKind, SourceBuilder, StoreOptions, XmltvProgramme};
use iptv_import::{EpgSourceConfig, GuideSink};
use iptv_providers::SecretUrl;
use iptv_store_sqlite::{DatabaseKey, EpgFeedMeta, SourcePolicy, Store};
use std::{cell::Cell, rc::Rc, task::Poll};

const NOW: UnixTime = 1_800_000_000;

fn fixture() -> (tempfile::TempDir, Rc<crate::Runtime>, Job, ChannelKey) {
    let directory = tempfile::tempdir().unwrap();
    let key = DatabaseKey::from_bytes([0x76; 32]);
    let mut store = Store::open_encrypted(directory.path().join("iptv.sqlite"), &key).unwrap();
    store.set_clock(|| NOW);
    let source = common::m3u_source("Generated cached opening");
    let id = store.add_source(&source).unwrap();
    let catalog = common::catalog(1, 1, common::XT_PASSWORD);
    let channel = ChannelKey::new(catalog.channels[0].id.as_str());
    common::replace(&mut store, id, &catalog);
    store
        .set_source_policy(
            id,
            SourcePolicy {
                enabled: true,
                refresh_seconds: 0,
                refresh_on_start: false,
            },
        )
        .unwrap();
    // No configured feeds, but a stored state: cached policy decides Discard.
    store
        .record_epg_failure(id, iptv_library::ImportErrorKind::Network)
        .unwrap();
    drop(store);
    let owner = Rc::new(crate::Runtime::new(crate::Ports::new(
        directory.path().to_owned(),
        move |_| Ok(key.clone()),
    )));
    let job = Job {
        source,
        epg_url: None,
        stored: Some(id),
        purpose: super::super::imports::Purpose::Switch,
        edit: false,
    };
    (directory, owner, job, channel)
}

fn guide(key: &ChannelKey, title: &str) -> (GuideStore, EpgFeedMeta) {
    let feed = iptv_epg::FeedId(1);
    let mut builder = SourceBuilder::new(feed, 0, StoreOptions::default(), NOW);
    builder.push_programme(XmltvProgramme {
        channel: "generated".into(),
        start: NOW,
        stop: Some(NOW + 3600),
        title: title.into(),
        ..XmltvProgramme::default()
    });
    let mut guide = GuideStore::new();
    guide.replace_source(builder.finish());
    guide.set_matches(
        [(
            key.clone(),
            vec![EpgRef {
                source: feed,
                epg_id: "generated".into(),
                kind: MatchKind::TvgId,
            }],
        )]
        .into_iter()
        .collect(),
    );
    let meta = EpgFeedMeta {
        config: EpgSourceConfig::new(feed, SecretUrl::new("https://generated.invalid/guide.xml")),
        fetched_at: NOW,
        etag: None,
        last_modified: None,
        bytes: 100,
    };
    (guide, meta)
}

fn staged_guide(
    store: &Store,
    key: &ChannelKey,
    text: &str,
) -> (crate::imports::Downloaded, tempfile::TempDir, EpgFeedMeta) {
    let (guide, meta) = guide(key, text);
    let directory = tempfile::tempdir().unwrap();
    let mut stage = iptv_store_sqlite::EpgStage::create_in_store(store, directory.path()).unwrap();
    stage
        .begin_feed(meta.config.id, 0, StoreOptions::default(), NOW)
        .unwrap();
    stage
        .programme("generated", &guide.programme_at(key, NOW).unwrap().data())
        .unwrap();
    stage.finish_feed().unwrap();
    let import = iptv_import::EpgImport {
        bindings: vec![(
            key.clone(),
            guide
                .binding(key)
                .map(|(source, epg_id)| EpgRef {
                    source,
                    epg_id: epg_id.into(),
                    kind: MatchKind::TvgId,
                })
                .unwrap(),
        )],
        ..iptv_import::EpgImport::default()
    };
    (
        crate::imports::Downloaded { stage, import },
        directory,
        meta,
    )
}

fn write_guide(
    store: &mut Store,
    source: SourceId,
    key: &ChannelKey,
    text: &str,
) -> Result<(), iptv_store_sqlite::StoreError> {
    let (mut download, _directory, meta) = staged_guide(store, key, text);
    store
        .replace_epg(source, &mut download.stage, &download.import, &[meta], 42)
        .map(|_| ())
}

async fn title(source: SourceId, key: ChannelKey) -> Option<String> {
    crate::store_service::call(move |store| {
        let guide = store.load_epg(source, &EpgWindow::from(NOW, 1))?;
        Ok(guide.programme_at(&key, NOW).map(|p| p.title().to_owned()))
    })
    .await
    .unwrap()
}

#[test]
fn revoked_cached_metadata_cannot_discard_a_newer_guide() {
    for cancel_all in [false, true] {
        let (_directory, owner, job, key) = fixture();
        crate::with_runtime(owner.clone(), || {
            owner.start_services();
            crate::executor::block_on(async {
                let source = job.stored.unwrap();
                let cancel = CancelToken::new();
                let work = crate::library::import_completion(cancel.clone());
                crate::library::track_import(source, &work);
                let (resume, held) = futures_channel::oneshot::channel();
                let reached = Rc::new(Cell::new(false));
                let paused = reached.clone();
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let stops = [&cancel, &work.guide];
                let mut opening = Box::pin(send_stored_after(
                    &job,
                    source,
                    NOW,
                    &tx,
                    &stops,
                    async move {
                        paused.set(true);
                        held.await.unwrap();
                    },
                ));
                std::future::poll_fn(|cx| {
                    assert!(opening.as_mut().poll(cx).is_pending());
                    if reached.get() {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
                .await;
                // Test each accepting token independently while metadata is held.
                if cancel_all {
                    cancel.cancel();
                    assert!(!work.guide.is_cancelled());
                } else {
                    crate::library::revoke_import_handoff(source);
                    assert!(work.guide.is_cancelled() && !cancel.is_cancelled());
                }
                let channel = key.clone();
                crate::store_service::call(move |store| {
                    write_guide(store, source, &channel, "Fresh generated programme")
                })
                .await
                .unwrap();
                resume.send(()).unwrap();
                let cached = opening.await;
                assert_eq!(
                    title(source, key.clone()).await.as_deref(),
                    Some("Fresh generated programme")
                );
                assert!(!cached, "a superseded discard is no cached policy answer");
                drop(tx);
                while let Some(message) = rx.recv().await {
                    assert!(
                        !matches!(message, Msg::NoCheck { .. }),
                        "obsolete metadata cannot settle new work"
                    );
                }
            });
            assert!(owner.drain_services());
        });
    }
}

#[test]
fn a_superseded_download_cannot_replace_a_newer_guide() {
    let (_directory, owner, job, key) = fixture();
    crate::with_runtime(owner.clone(), || {
        owner.start_services();
        crate::executor::block_on(async {
            let source = job.stored.unwrap();
            let channel = key.clone();
            crate::store_service::call(move |store| {
                write_guide(store, source, &channel, "New generated programme")
            })
            .await
            .unwrap();
            let channel = key.clone();
            let worker = owner.scope_worker(move || {
                let (downloaded, _scratch, meta) = crate::store_service::blocking(move |store| {
                    Ok(staged_guide(
                        store,
                        &channel,
                        "Obsolete generated programme",
                    ))
                })
                .unwrap();
                let stop = CancelToken::new();
                stop.cancel();
                let overrides = iptv_epg::Overrides::default();
                let matching = stored::Matching {
                    channels: &[],
                    overrides: &overrides,
                };
                assert!(
                    stored::replace(source, downloaded, &[meta.config], matching, NOW, &[&stop])
                        .is_err()
                );
            });
            std::thread::spawn(worker).join().unwrap();
            assert_eq!(
                title(source, key).await.as_deref(),
                Some("New generated programme")
            );
        });
        assert!(owner.drain_services());
    });
}
