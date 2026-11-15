use super::*;
use iptv_providers::{
    ContentId, SecretUrl,
    model::{M3uSourceConfig, Source, SourceConfig},
};

fn fixture() -> (Store, Request) {
    let mut store = Store::open_in_memory().unwrap();
    let source = store
        .add_source(&Source {
            name: "Generated DVR source".into(),
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
    let request = Request {
        source: source.0,
        channel: parsed.channels.items[0].id.as_str().into(),
        channel_name: "Generated channel".into(),
        title: "Generated programme".into(),
        start: 1800000000,
        end: 1800003600,
    };
    (store, request)
}
#[test]
fn edited_or_cancelled_intent_invalidates_a_scheduler_claim() {
    let (mut store, mut request) = fixture();
    let id = store
        .schedule_recording(JobId(41), &request, request.start - 100)
        .unwrap();
    assert_eq!(
        store
            .schedule_recording(JobId(42), &request, request.start - 100)
            .unwrap(),
        id
    );
    let stale = store.recording_job(id).unwrap().unwrap();
    request.start += 60;
    assert!(
        store
            .update_recording(id, &request, request.start - 100)
            .unwrap()
    );
    assert!(!store.claim_recording(&stale).unwrap());
    let current = store.recording_job(id).unwrap().unwrap();
    assert!(store.claim_recording(&current).unwrap());
    assert!(!store.claim_recording(&current).unwrap());
    assert!(
        !store
            .update_recording(id, &request, request.start - 100)
            .unwrap()
    );
    assert!(!store.remove_recording(id).unwrap());
}
#[test]
fn scheduling_rechecks_blocking_and_source_enablement() {
    let (mut store, request) = fixture();
    let source = crate::SourceId(request.source);
    store
        .set_blocked(
            source,
            &ContentId::from_stored(request.channel.clone()),
            true,
        )
        .unwrap();
    assert!(
        store
            .schedule_recording(JobId(41), &request, request.start - 100)
            .is_err()
    );
    store
        .set_blocked(
            source,
            &ContentId::from_stored(request.channel.clone()),
            false,
        )
        .unwrap();
    store
        .set_source_policy(
            source,
            crate::SourcePolicy {
                enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        store
            .schedule_recording(JobId(41), &request, request.start - 100)
            .is_err()
    );
}
#[test]
fn portable_backups_exclude_host_jobs_without_changing_live_intent() {
    let (mut store, request) = fixture();
    store
        .schedule_recording(JobId(41), &request, request.start - 100)
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("generated.db");
    store.snapshot_to(&path).unwrap();
    assert!(
        Store::open(path)
            .unwrap()
            .recording_jobs()
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.recording_jobs().unwrap().len(), 1);
}
