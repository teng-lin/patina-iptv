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
fn deleting_and_reusing_a_source_id_cannot_retarget_recording_intent() {
    let (mut store, request) = fixture();
    let id = store
        .schedule_recording(JobId(43), &request, request.start - 100)
        .unwrap();
    assert!(store.recording_source_is_current(id).unwrap());
    let source = store
        .source(crate::SourceId(request.source))
        .unwrap()
        .unwrap()
        .source;
    store
        .delete_source(crate::SourceId(request.source))
        .unwrap();
    assert!(!store.recording_source_is_current(id).unwrap());
    let replacement = store.add_source(&source).unwrap();
    assert_eq!(replacement.0, request.source);
    assert!(!store.recording_source_is_current(id).unwrap());
    assert!(store.recording_job(id).unwrap().is_some());
    let parsed = iptv_providers::m3u::parse_m3u(
        "#EXTM3U\n#EXTINF:-1,Generated channel\nhttps://example.test/generated.ts\n",
    );
    store
        .replace_catalog(
            replacement,
            &parsed.groups,
            &parsed.channels.items,
            &[],
            &[],
        )
        .unwrap();
    let new_id = store
        .schedule_recording(JobId(44), &request, request.start - 100)
        .unwrap();
    assert_ne!(id, new_id);
    assert!(!store.recording_job(id).unwrap().unwrap().source_current);
    assert!(store.recording_job(new_id).unwrap().unwrap().source_current);
    assert_eq!(
        store
            .schedule_recording(JobId(45), &request, request.start - 100)
            .unwrap(),
        new_id
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

#[test]
fn playback_progress_is_durable_only_for_playable_jobs_and_cascades_on_delete() {
    use iptv_core::on_demand::Progress;
    let (mut store, request) = fixture();
    let id = store
        .schedule_recording(JobId(96), &request, request.start - 100)
        .unwrap();
    let point = Progress::observed(24_000, Some(120_000), false);
    store
        .save_recording_progress(id, point, request.start)
        .unwrap();
    assert_eq!(store.recording_progress(id).unwrap(), Progress::default());
    store
        .conn
        .execute(
            "UPDATE recording_jobs SET status='partial',bytes=1200 WHERE id=?1",
            [id.0],
        )
        .unwrap();
    store
        .save_recording_progress(id, point, request.start)
        .unwrap();
    assert_eq!(store.recording_progress(id).unwrap(), point);
    let finished = Progress::observed(119_000, Some(120_000), false);
    store
        .save_recording_progress(id, finished, request.start + 2)
        .unwrap();
    assert_eq!(store.recording_progress(id).unwrap().resume_ms(), 0);
    assert!(store.remove_recording(id).unwrap());
    assert_eq!(store.recording_progress(id).unwrap(), Progress::default());
    store
        .save_recording_progress(id, point, request.start + 3)
        .unwrap();
    assert_eq!(store.recording_progress(id).unwrap(), Progress::default());
}
#[test]
fn recurrence_claim_cancel_and_deletion_never_rearm_an_occurrence() {
    use iptv_core::recording::{Repeat, Rule, RuleId};
    let (mut store, request) = fixture();
    let rule = Rule {
        id: RuleId(100),
        local_start: request.start,
        request: request.clone(),
        repeat: Repeat::Daily,
        enabled: true,
    };
    let first = store
        .schedule_recording_rule_and_first(&rule, JobId(101), request.start - 1)
        .unwrap();
    assert_eq!(store.recording_rule_for_job(first).unwrap(), Some(rule.id));
    assert_eq!(
        store
            .materialize_recording_rule(rule.id, JobId(102), &request, request.start - 1)
            .unwrap(),
        None
    );
    store
        .transition_recording(first, Status::Scheduled, Status::Cancelled, 0, "Cancelled")
        .unwrap();
    assert!(store.remove_recording(first).unwrap());
    assert!(
        store
            .recording_rule_has_occurrence(rule.id, request.start)
            .unwrap()
    );
    assert_eq!(
        store
            .materialize_recording_rule(rule.id, JobId(103), &request, request.start - 1)
            .unwrap(),
        None
    );
    let mut next = request.clone();
    next.start += 86400;
    next.end += 86400;
    assert_eq!(
        store
            .materialize_recording_rule(rule.id, JobId(104), &next, request.start - 1)
            .unwrap(),
        Some(JobId(104))
    );
    store.set_recording_rule_enabled(rule.id, false).unwrap();
    assert_eq!(
        store.recording_job(JobId(104)).unwrap().unwrap().status,
        Status::Cancelled
    );
    next.start += 86400;
    next.end += 86400;
    assert_eq!(
        store
            .materialize_recording_rule(rule.id, JobId(105), &next, request.start - 1)
            .unwrap(),
        None
    );
    store.discard_recording_jobs().unwrap();
    assert!(store.recording_rules().unwrap().is_empty());
}
#[test]
fn stale_recurring_source_has_no_authority_for_a_replacement() {
    use iptv_core::recording::{Repeat, Rule, RuleId};
    let (mut store, request) = fixture();
    let rule = Rule {
        id: RuleId(100),
        local_start: request.start,
        request: request.clone(),
        repeat: Repeat::Daily,
        enabled: true,
    };
    store
        .schedule_recording_rule(&rule, request.start - 1)
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE guide_revisions SET incarnation='generated-replacement' WHERE source_id=?1",
            [request.source],
        )
        .unwrap();
    assert!(!store.recording_rules().unwrap()[0].enabled);
    assert_eq!(
        store
            .materialize_recording_rule(rule.id, JobId(101), &request, request.start - 1)
            .unwrap(),
        None
    );
}
