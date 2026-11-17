//! Cached single-source startup, descriptions, older pages and manual guide edits.
#![cfg(feature = "test-support")]
use crate::common;
use iptv_epg::{ChannelKey, DAY, EpgRef, FeedId, MatchKind, ProgrammeData};
use iptv_import::{EpgImport, EpgSourceConfig, GuideSink, ListedChannel};
use iptv_runtime::{Ports, Runtime, RuntimeEvent, data};
use iptv_settings::{Settings, Target, Value};
use iptv_store_sqlite::{
    DatabaseKey, EpgFeedMeta, EpgStage, GuideSource, SourceId, SourcePolicy, Store,
};
use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const KEY: [u8; 32] = [0x72; 32];
const FEED: FeedId = FeedId(8);
#[derive(Debug)]
enum Event {
    Schedule(usize),
    Guide,
    Settings,
    Notice(&'static str),
}

async fn event(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    condition: impl Fn(&Event) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = rx.recv().await {
            if condition(&event) {
                return;
            }
        }
        panic!("fixture event port closed");
    })
    .await
    .expect("expected service acknowledgement did not arrive");
}

fn seed(root: &std::path::Path, now: i64) -> (SourceId, String, Settings) {
    let mut settings = Settings::new();
    settings
        .set("epg.update_on_start", Value::Bool(false))
        .unwrap();
    settings
        .set("epg.update_on_playlist_change", Value::Bool(false))
        .unwrap();
    settings.set("epg.update_interval", Value::secs(0)).unwrap();
    settings
        .set("epg.keep_past", Value::secs(7 * 86400))
        .unwrap();
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("settings.json"), settings.to_json_string()).unwrap();
    let mut store =
        Store::open_encrypted(root.join("iptv.sqlite"), &DatabaseKey::from_bytes(KEY)).unwrap();
    let source = store
        .add_source(&common::m3u_source("Generated cached guide"))
        .unwrap();
    let catalogue = common::catalog(2, 1, common::XT_PASSWORD);
    common::replace(&mut store, source, &catalogue);
    store
        .set_source_policy(
            source,
            SourcePolicy {
                refresh_on_start: false,
                ..Default::default()
            },
        )
        .unwrap();
    let config = GuideSource {
        feed: FEED,
        provider_index: None,
        url: iptv_providers::SecretUrl::new("https://example.invalid/generated.xml"),
        name: Some("Generated cached feed".into()),
        enabled: true,
        shift_minutes: 0,
    };
    store
        .set_guide_sources(source, std::slice::from_ref(&config))
        .unwrap();
    let options = iptv_import::store_options(&settings, &Target::Global);
    let mut stage = EpgStage::create_in_store(&store, root).unwrap();
    stage.begin_feed(FEED, 1000, options, now).unwrap();
    for (index, channel) in catalogue.channels.iter().enumerate() {
        let id = channel.epg_id.as_deref().unwrap();
        for start in [now - 3 * DAY, now - 60] {
            stage
                .programme(
                    id,
                    &ProgrammeData {
                        start,
                        stop: Some(start + 3600),
                        title: if index == 0 {
                            "Generated first programme"
                        } else {
                            "Generated second programme"
                        },
                        desc: Some("Generated retained description"),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
    }
    stage.finish_feed().unwrap();
    let import = EpgImport {
        bindings: catalogue
            .channels
            .iter()
            .map(|channel| {
                (
                    ChannelKey::new(channel.id.as_str()),
                    EpgRef {
                        source: FEED,
                        epg_id: channel.epg_id.clone().unwrap(),
                        kind: MatchKind::TvgId,
                    },
                )
            })
            .collect(),
        listed: vec![(
            FEED,
            catalogue
                .channels
                .iter()
                .map(|channel| ListedChannel {
                    id: channel.epg_id.clone().unwrap(),
                    name: Some(channel.name.clone()),
                })
                .collect(),
        )],
        ..Default::default()
    };
    let mut imported_config = EpgSourceConfig::new(FEED, config.url);
    imported_config.priority = 1000;
    imported_config.store = options;
    let metadata = EpgFeedMeta {
        config: imported_config,
        fetched_at: now,
        etag: None,
        last_modified: None,
        bytes: 1024,
    };
    store
        .replace_epg(
            source,
            &mut stage,
            &import,
            &[metadata],
            iptv_import::matching_fingerprint(&catalogue.channels, &Default::default()),
        )
        .unwrap();
    (source, catalogue.channels[0].id.to_string(), settings)
}

#[test]
fn cached_startup_pages_descriptions_and_manual_choices_preserve_channel_identity() {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio::task::LocalSet::new().block_on(&executor, async {
        let directory = tempfile::tempdir().unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let (source, key, _) = seed(directory.path(), now);
        let owner = Rc::new(Runtime::new(Ports::new(directory.path().into(), |_| Ok(DatabaseKey::from_bytes(KEY)))));
        owner.bind_tasks(|task| drop(tokio::task::spawn_local(task)), |duration| Box::pin(tokio::time::sleep(duration)));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let resumes = Rc::new(Cell::new(0));
        let count = resumes.clone();
        let weak = Rc::downgrade(&owner);
        owner.bind(move |event| match event {
            RuntimeEvent::EarlyResume { done, .. } => { count.set(count.get() + 1); let _ = done.send(()); },
            RuntimeEvent::ScheduleReady(schedule) => {
                let owner = weak.upgrade().unwrap();
                let channels = schedule.channel_count();
                owner.set_schedule(schedule.clone());
                owner.favorites_catalog_changed(schedule.as_ref());
                tx.send(Event::Schedule(channels)).unwrap();
            },
            RuntimeEvent::GuideChanged => { tx.send(Event::Guide).unwrap(); },
            RuntimeEvent::SettingsChanged => { tx.send(Event::Settings).unwrap(); },
            RuntimeEvent::Notify(message) => { tx.send(Event::Notice(message)).unwrap(); },
            _ => {},
        });
        owner.start_services();
        owner.load_settings();
        data::start(&owner);
        event(&mut rx, |event| matches!(event, Event::Schedule(2))).await;
        if let Some(finished) = owner.pending_import_completion(source) { tokio::time::timeout(Duration::from_secs(5), finished).await.unwrap(); }
        assert_eq!(resumes.get(), 1);
        assert_eq!(owner.shown_source(), Some(source));
        data::stored::refresh_status(&owner);
        event(&mut rx, |event| matches!(event, Event::Settings) && !owner.source_lines(source).is_empty()).await;
        assert_eq!(owner.schedule().channel_key(0).as_deref(), Some(key.as_str()));
        let current = data::assign::current(&owner, source, &key).await;
        assert_eq!(current.bound.as_ref().map(|binding| binding.1.as_str()), Some("ch0.example"));
        assert_eq!(data::assign::guide_channels(&owner, source).await.len(), 2);

        let lookup = data::stored::description_lookup(&owner, source).unwrap();
        assert_eq!(lookup(&ChannelKey::new(&key), now - 60), None);
        event(&mut rx, |event| matches!(event, Event::Guide)).await;
        assert_eq!(lookup(&ChannelKey::new(&key), now - 60).as_deref(), Some("Generated retained description"));

        let schedule = owner.schedule();
        let (done, retry) = futures_channel::oneshot::channel();
        assert!(!data::pages::ready(&owner, &schedule, 0, now - 3 * DAY, move |result| { result.expect("retained guide page loads"); let _ = done.send(()); }));
        tokio::time::timeout(Duration::from_secs(5), retry).await.unwrap().unwrap();
        assert!(data::pages::ready(&owner, &owner.schedule(), 0, now - 3 * DAY, |_| panic!("loaded page must not queue another request")));
        data::pages::pin_archive(&owner, Some((0, now - 3 * DAY)));
        data::pages::evict(&owner);
        data::pages::pin_archive(&owner, None);
        data::pages::evict(&owner);

        for choice in [data::assign::Choice::NoGuide, data::assign::Choice::Channel { feed: FEED, epg_channel: "ch1.example".into() }] {
            let (done, applied) = futures_channel::oneshot::channel();
            data::assign::choose(&owner, source, key.clone(), choice.clone(), move |result| { let _ = done.send(result); });
            assert_eq!(tokio::time::timeout(Duration::from_secs(5), applied).await.unwrap().unwrap(), data::assign::Applied::Now);
            assert_eq!(data::assign::current(&owner, source, &key).await.choice, Some(choice));
            assert_eq!(owner.schedule().channel_key(0).as_deref(), Some(key.as_str()));
        }
        assert_eq!(data::assign::current(&owner, source, &key).await.bound.unwrap().1, "ch1.example");
        // Refuse the real pool submission, rather than replacing the network/store.
        owner.host_handle().refuse_work(1);
        let (done, applied) = futures_channel::oneshot::channel();
        data::assign::choose(&owner, source, key.clone(), data::assign::Choice::Automatic, move |result| { let _ = done.send(result); });
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), applied).await.unwrap().unwrap(), data::assign::Applied::Downloading);
        event(&mut rx, |event| matches!(event, Event::Notice(message) if message.contains("Too many playlists"))).await;
        if let Some(finished) = owner.pending_import_completion(source) { tokio::time::timeout(Duration::from_secs(5), finished).await.unwrap(); }
        assert_eq!(data::assign::current(&owner, source, &key).await.choice, Some(data::assign::Choice::Automatic));
        let (done, cleared) = futures_channel::oneshot::channel();
        assert!(data::stored::clear(&owner, move |result| { let _ = done.send(result); }));
        assert!(tokio::time::timeout(Duration::from_secs(5), cleared).await.unwrap().unwrap());
        assert_eq!(owner.schedule().channel_count(), 2);
        assert!(data::assign::current(&owner, source, &key).await.bound.is_none());
        data::stored::refresh_status(&owner);
        event(&mut rx, |event| matches!(event, Event::Settings)).await;
        assert!(owner.drain_services());
        assert_eq!(data::assign::current(&owner, source, &key).await, Default::default(), "retired reads are a failed observation, not a resurrected binding");
    });
}
