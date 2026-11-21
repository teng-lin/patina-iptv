//! Foreground return cannot cancel a saved alarm while native recovery is blocked.
use crate::support;

use iptv_core::recording::{JobId, Request, WakeCapability};
use iptv_providers::{
    SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};
use iptv_runtime::{Ports, Runtime, model::DatabaseKey};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicI64, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use support::block_on;

type Gate = (SyncSender<()>, Receiver<()>);
static BROKER: Mutex<Option<Gate>> = Mutex::new(None);
static ALARM: AtomicI64 = AtomicI64::new(0);
static REGISTERED: Mutex<Option<SyncSender<i64>>> = Mutex::new(None);

#[test]
fn early_foreground_return_keeps_saved_wake_until_durable_jobs_are_loaded() {
    let directory = tempfile::tempdir().unwrap();
    let key = DatabaseKey::from_bytes([0x78; 32]);
    let mut store =
        iptv_store_sqlite::Store::open_encrypted(directory.path().join("iptv.sqlite"), &key)
            .unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated startup source".into(),
            config: SourceConfig::M3u(M3uSourceConfig {
                location: SecretUrl::new("https://example.test/generated.m3u"),
                user_agent: None,
                referrer: None,
            }),
        })
        .unwrap();
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1,Generated channel\nhttps://example.test/generated.ts\n",
    );
    store
        .replace_catalog(source, &parsed.groups, &parsed.channels.items, &[], &[])
        .unwrap();
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let next = at + 86400;
    store
        .schedule_recording(
            JobId(9),
            &Request {
                source: source.0,
                channel: parsed.channels.items[0].id.as_str().into(),
                channel_name: "Generated channel".into(),
                title: "Generated future program".into(),
                start: next,
                end: next + 60,
            },
            at,
        )
        .unwrap();
    drop(store);
    ALARM.store(next, Ordering::SeqCst);
    let mut ports = Ports::new(directory.path().to_owned(), |_| {
        if let Some((entered, release)) = BROKER.lock().unwrap().take() {
            entered.send(()).unwrap();
            release.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        Ok(DatabaseKey::from_bytes([0x78; 32]))
    });
    ports.background_recordings = true;
    ports.recording_schedule = |next, _| {
        ALARM.store(next, Ordering::SeqCst);
        if let Some(sender) = REGISTERED.lock().unwrap().as_ref() {
            let _ = sender.try_send(next);
        }
        WakeCapability::Exact
    };
    let (entered, seen) = sync_channel(1);
    let (release, parked) = sync_channel(1);
    *BROKER.lock().unwrap() = Some((entered, parked));
    let (registered, alarm) = sync_channel(8);
    *REGISTERED.lock().unwrap() = Some(registered);
    let owner = Runtime::new(ports);
    owner.start_services();
    owner.start_recordings();
    seen.recv_timeout(Duration::from_secs(10)).unwrap();
    let driver = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let refreshed = driver.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), owner.resume_recording_wake()).await
    });
    let during_startup = ALARM.load(Ordering::SeqCst);
    let pending = owner.recording_capabilities();
    // Release the actual native store worker before assertions, also on regressions.
    release.send(()).unwrap();
    refreshed
        .expect("foreground return must not wait for the blocked store")
        .unwrap();
    assert_eq!(
        during_startup, next,
        "an unloaded snapshot must not clear the saved alarm"
    );
    assert_eq!(
        pending.wake, None,
        "no new run report exists before recovery"
    );
    assert_eq!(alarm.recv_timeout(Duration::from_secs(10)).unwrap(), next);
    block_on(owner.refresh_recording_wake()).unwrap();
    assert!(owner.recording_capabilities().exact_start());
    assert_eq!(owner.recording_snapshot()[0].id, JobId(9));
    assert!(owner.shutdown_recording_host());
    assert!(owner.drain_services());
}
