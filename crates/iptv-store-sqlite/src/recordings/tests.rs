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
fn series_occurrences_keep_raw_identity_across_padding_cancellation_and_retry() {
    use iptv_core::recording::{Repeat, Rule, RuleId};
    let (mut store, original) = fixture();
    let mut padded = original.clone();
    padded.start -= 300;
    padded.end += 600;
    let rule = Rule {
        id: RuleId(400),
        local_start: original.start,
        request: padded.clone(),
        repeat: Repeat::Series,
        enabled: true,
        lead_seconds: 300,
        trail_seconds: 600,
    };
    let first = store
        .schedule_recording_rule_and_first_at(
            &rule,
            JobId(401),
            original.start,
            original.start - 1000,
        )
        .unwrap();
    let loaded = store.recording_rules().unwrap().pop().unwrap();
    assert_eq!((loaded.lead_seconds, loaded.trail_seconds), (300, 600));
    assert!(
        store
            .recording_rule_has_occurrence(rule.id, original.start)
            .unwrap()
    );
    assert!(
        !store
            .recording_rule_has_occurrence(rule.id, padded.start)
            .unwrap()
    );
    assert_eq!(
        store
            .materialize_recording_rule_at(
                rule.id,
                JobId(402),
                &padded,
                original.start,
                original.start - 1000
            )
            .unwrap(),
        None
    );
    // A retry may have a later clamped record-now start; its raw occurrence is stable.
    let mut retry = rule.request.clone();
    retry.start += 10;
    assert_eq!(
        store
            .schedule_recording_rule_with_first(
                &rule,
                JobId(403),
                &retry,
                original.start,
                original.start - 1000
            )
            .unwrap(),
        first
    );
    store
        .transition_recording(first, Status::Scheduled, Status::Cancelled, 0, "Canceled")
        .unwrap();
    store.remove_recording(first).unwrap();
    assert_eq!(
        store
            .materialize_recording_rule_at(
                rule.id,
                JobId(404),
                &padded,
                original.start,
                original.start - 1000
            )
            .unwrap(),
        None
    );
}

#[test]
fn cancelling_recurring_schedules_frees_the_rule_quota_without_deleting_job_history() {
    use iptv_core::recording::{Repeat, Rule, RuleId};
    let (mut store, request) = fixture();
    for n in 0..110 {
        let mut occurrence = request.clone();
        occurrence.start += n * 86400;
        occurrence.end += n * 86400;
        let rule = Rule {
            id: RuleId(1000 + n),
            local_start: occurrence.start,
            request: occurrence,
            repeat: Repeat::Daily,
            enabled: true,
            lead_seconds: 0,
            trail_seconds: 0,
        };
        let job = store
            .schedule_recording_rule_and_first(&rule, JobId(2000 + n), request.start - 1)
            .unwrap();
        store.cancel_recording_rule(rule.id).unwrap();
        assert!(store.recording_rules().unwrap().is_empty());
        assert_eq!(
            store.recording_job(job).unwrap().unwrap().status,
            Status::Cancelled
        );
    }
    assert_eq!(store.recording_jobs().unwrap().len(), 110);
}

#[test]
fn a_colliding_first_job_cannot_silently_attach_a_different_repeating_rule() {
    use iptv_core::recording::{Repeat, Rule, RuleId};
    let (mut store, request) = fixture();
    let mut rule = Rule {
        id: RuleId(500),
        local_start: request.start,
        request: request.clone(),
        repeat: Repeat::Daily,
        enabled: true,
        lead_seconds: 0,
        trail_seconds: 0,
    };
    let first = store
        .schedule_recording_rule_and_first(&rule, JobId(501), request.start - 1)
        .unwrap();
    rule.id = RuleId(502);
    rule.repeat = Repeat::Weekly;
    assert!(
        store
            .schedule_recording_rule_and_first(&rule, JobId(503), request.start - 1)
            .is_err()
    );
    assert_eq!(store.recording_rules().unwrap().len(), 1);
    assert_eq!(
        store.recording_rule_for_job(first).unwrap(),
        Some(RuleId(500))
    );
}

#[test]
fn recurring_requests_with_different_durations_do_not_reuse_the_shorter_rule() {
    use iptv_core::recording::{Repeat, Rule, RuleId};
    for repeat in [Repeat::Daily, Repeat::Weekly] {
        let (mut store, mut request) = fixture();
        request.end = request.start + 1800;
        let mut rule = Rule {
            id: RuleId(600),
            local_start: request.start,
            request: request.clone(),
            repeat,
            enabled: true,
            lead_seconds: 0,
            trail_seconds: 0,
        };
        let shorter = store
            .schedule_recording_rule_and_first(&rule, JobId(601), request.start - 1)
            .unwrap();
        rule.id = RuleId(602);
        rule.request.end = rule.request.start + 3600;
        let longer = store
            .schedule_recording_rule_and_first(&rule, JobId(603), request.start - 1)
            .unwrap();
        assert_ne!(shorter, longer);
        assert_eq!(
            store.recording_job(shorter).unwrap().unwrap().request.end - request.start,
            1800
        );
        assert_eq!(
            store.recording_job(longer).unwrap().unwrap().request.end - request.start,
            3600
        );
        assert_eq!(store.recording_rules().unwrap().len(), 2);
        assert_eq!(
            store
                .schedule_recording_rule_and_first(&rule, JobId(604), request.start - 1)
                .unwrap(),
            longer,
            "an equivalent longer request remains idempotent"
        );
    }
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
        lead_seconds: 0,
        trail_seconds: 0,
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
        .transition_recording(first, Status::Scheduled, Status::Cancelled, 0, "Canceled")
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
    store
        .set_recording_rule_enabled(rule.id, false, request.start - 1)
        .unwrap();
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
        lead_seconds: 0,
        trail_seconds: 0,
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

#[test]
fn enabling_a_rule_restores_only_future_jobs_cancelled_by_disabling_it() {
    use iptv_core::recording::{Repeat, Rule, RuleId};
    let (mut store, request) = fixture();
    let rule = Rule {
        id: RuleId(101),
        request: request.clone(),
        repeat: Repeat::Daily,
        local_start: request.start,
        enabled: true,
        lead_seconds: 0,
        trail_seconds: 0,
    };
    let first = store
        .schedule_recording_rule_and_first(&rule, JobId(201), request.start - 1)
        .unwrap();
    let mut next = request.clone();
    next.start += 86400;
    next.end += 86400;
    let manual = store
        .materialize_recording_rule(rule.id, JobId(202), &next, request.start - 1)
        .unwrap()
        .unwrap();
    next.start += 86400;
    next.end += 86400;
    let removed = store
        .materialize_recording_rule(rule.id, JobId(203), &next, request.start - 1)
        .unwrap()
        .unwrap();
    next.start += 86400;
    next.end += 86400;
    let restored = store
        .materialize_recording_rule(rule.id, JobId(204), &next, request.start - 1)
        .unwrap()
        .unwrap();
    store
        .transition_recording(manual, Status::Scheduled, Status::Cancelled, 0, "Canceled")
        .unwrap();
    store
        .set_recording_rule_enabled(rule.id, false, request.start - 1)
        .unwrap();
    assert!(store.remove_recording(removed).unwrap());
    // The first occurrence has elapsed by the time the user re-enables it.
    store
        .set_recording_rule_enabled(rule.id, true, request.end + 1)
        .unwrap();
    assert_eq!(
        store.recording_job(first).unwrap().unwrap().status,
        Status::Cancelled
    );
    assert_eq!(
        store.recording_job(manual).unwrap().unwrap().status,
        Status::Cancelled
    );
    assert!(store.recording_job(removed).unwrap().is_none());
    assert!(
        store
            .recording_rule_has_occurrence(rule.id, request.start + 2 * 86400)
            .unwrap()
    );
    assert_eq!(
        store.recording_job(restored).unwrap().unwrap().status,
        Status::Scheduled
    );
}

#[test]
fn checked_recording_commands_authorize_inside_the_accepting_transaction() {
    use iptv_core::recording::{Repeat, Rule, RuleId};
    let (mut store, request) = fixture();
    let at = request.start - 100;
    let job = store.schedule_recording(JobId(901), &request, at).unwrap();
    let rule = Rule {
        id: RuleId(902),
        request: Request {
            start: request.start + 86400,
            end: request.end + 86400,
            ..request.clone()
        },
        local_start: request.start + 86400,
        repeat: Repeat::Daily,
        enabled: true,
        lead_seconds: 0,
        trail_seconds: 0,
    };
    store
        .schedule_recording_rule_and_first(&rule, JobId(903), at)
        .unwrap();
    store
        .set_recording_rule_enabled(rule.id, false, at)
        .unwrap();
    let deny = |store: &Store| {
        assert!(
            !store.conn.is_autocommit(),
            "authorization must run inside the writer transaction"
        );
        assert_eq!(store.recording_job(job)?.unwrap().request.end, request.end);
        Err(StoreError::Unavailable("Synthetic revoked grant"))
    };
    // Even an identical retry must pass authorization before returning its old ID.
    assert!(
        store
            .schedule_recording_checked(JobId(904), &request, at, deny)
            .is_err()
    );
    let edited = Request {
        end: request.end + 60,
        ..request.clone()
    };
    assert!(
        store
            .update_recording_checked(job, &edited, at, deny)
            .is_err()
    );
    let another = Rule {
        id: RuleId(905),
        request: edited,
        ..rule.clone()
    };
    assert!(
        store
            .schedule_recording_rule_with_first_checked(
                &another,
                JobId(906),
                &another.request,
                another.request.start,
                at,
                deny
            )
            .is_err()
    );
    assert!(
        store
            .set_recording_rule_enabled_checked(rule.id, true, at, deny)
            .is_err()
    );
    assert_eq!(store.recording_jobs().unwrap().len(), 2);
    assert_eq!(
        store.recording_job(job).unwrap().unwrap().request.end,
        request.end
    );
    let rules = store.recording_rules().unwrap();
    assert_eq!(rules.len(), 1);
    assert!(!rules[0].enabled);
}

#[test]
fn raw_recording_admission_stays_closed_while_checked_and_accepted_intent_can_proceed() {
    use iptv_core::recording::{Repeat, Rule, RuleId};
    let (mut store, request) = fixture();
    let at = request.start - 100;
    let job = store.schedule_recording(JobId(911), &request, at).unwrap();
    let rule = Rule {
        id: RuleId(912),
        request: Request {
            start: request.start + 86400,
            end: request.end + 86400,
            ..request.clone()
        },
        local_start: request.start + 86400,
        repeat: Repeat::Daily,
        enabled: true,
        lead_seconds: 0,
        trail_seconds: 0,
    };
    store
        .schedule_recording_rule_and_first(&rule, JobId(913), at)
        .unwrap();
    store
        .set_recording_rule_enabled(rule.id, false, at)
        .unwrap();
    store
        .set_blocked(
            crate::SourceId(request.source),
            &ContentId::from_stored(request.channel.clone()),
            true,
        )
        .unwrap();
    assert!(store.schedule_recording(JobId(914), &request, at).is_err());
    let edited = Request {
        end: request.end + 60,
        ..request.clone()
    };
    assert!(store.update_recording(job, &edited, at).is_err());
    assert!(store.set_recording_rule_enabled(rule.id, true, at).is_err());
    let authorize = |store: &Store| {
        assert!(!store.conn.is_autocommit());
        Ok(())
    };
    assert_eq!(
        store
            .schedule_recording_checked(JobId(914), &request, at, authorize)
            .unwrap(),
        job
    );
    assert!(
        store
            .update_recording_checked(job, &edited, at, authorize)
            .unwrap()
    );
    store
        .set_recording_rule_enabled_checked(rule.id, true, at, authorize)
        .unwrap();
    let next = Request {
        start: rule.request.start + 86400,
        end: rule.request.end + 86400,
        ..request.clone()
    };
    assert_eq!(
        store
            .materialize_recording_rule(rule.id, JobId(915), &next, at)
            .unwrap(),
        Some(JobId(915))
    );
    // A new source incarnation cannot inherit even a checked re-enable.
    store
        .set_recording_rule_enabled(rule.id, false, at)
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE guide_revisions SET incarnation='generated-replacement' WHERE source_id=?1",
            [request.source],
        )
        .unwrap();
    assert!(
        store
            .set_recording_rule_enabled_checked(rule.id, true, at, authorize)
            .is_err()
    );
    assert!(
        store
            .materialize_recording_rule(rule.id, JobId(916), &next, at)
            .unwrap()
            .is_none()
    );
}

fn completion_fixture() -> (Store, Job, RecordingCompletion) {
    let (mut store, request) = fixture();
    let id = store
        .schedule_recording(JobId(700), &request, request.start - 1)
        .unwrap();
    let mut job = store.recording_job(id).unwrap().unwrap();
    assert!(store.claim_recording(&job).unwrap());
    job.status = Status::Recording;
    let completion = RecordingCompletion {
        job: id,
        status: Status::Completed,
        bytes: 188,
        digest: "ab".repeat(32),
        message: "Recording finished".into(),
    };
    (store, job, completion)
}

#[test]
fn completion_intent_is_idempotent_and_cannot_replace_a_different_receipt() {
    let (mut store, job, completion) = completion_fixture();
    assert!(
        store
            .prepare_recording_completion(&job, &completion)
            .unwrap()
    );
    assert!(
        store
            .prepare_recording_completion(&job, &completion)
            .unwrap()
    );
    let mut wrong = completion.clone();
    wrong.digest = "cd".repeat(32);
    assert!(!store.prepare_recording_completion(&job, &wrong).unwrap());
    assert!(!store.commit_recording_completion(&wrong).unwrap());
    assert!(store.commit_recording_completion(&completion).unwrap());
    assert!(store.commit_recording_completion(&completion).unwrap());
    assert!(store.recording_completion(job.id).unwrap().is_none());
    assert_eq!(
        store.recording_job(job.id).unwrap().unwrap().status,
        Status::Completed
    );
    let sync: i64 = store
        .conn
        .pragma_query_value(None, "synchronous", |r| r.get(0))
        .unwrap();
    assert_eq!(sync, 1, "receipt must restore connection policy");
}

#[test]
fn newer_state_and_deleted_jobs_supersede_pending_completion() {
    for delete in [false, true] {
        let (mut store, job, completion) = completion_fixture();
        assert!(
            store
                .prepare_recording_completion(&job, &completion)
                .unwrap()
        );
        assert!(
            store
                .transition_recording(
                    job.id,
                    Status::Recording,
                    Status::Cancelled,
                    0,
                    "Synthetic cancellation"
                )
                .unwrap()
        );
        if delete {
            assert!(store.remove_recording(job.id).unwrap());
        }
        assert!(!store.commit_recording_completion(&completion).unwrap());
        assert!(
            !store
                .prepare_recording_completion(&job, &completion)
                .unwrap()
        );
        assert!(store.recording_completion(job.id).unwrap().is_none());
        assert_eq!(
            store.recording_job(job.id).unwrap().map(|job| job.status),
            (!delete).then_some(Status::Cancelled)
        );
    }
}

#[test]
fn failed_terminal_update_retains_receipt_for_retry() {
    let (mut store, job, completion) = completion_fixture();
    assert!(
        store
            .prepare_recording_completion(&job, &completion)
            .unwrap()
    );
    store.conn.execute_batch("CREATE TEMP TRIGGER synthetic_full BEFORE UPDATE OF status ON recording_jobs BEGIN SELECT RAISE(FAIL, 'synthetic storage failure'); END;").unwrap();
    assert!(store.commit_recording_completion(&completion).is_err());
    assert_eq!(
        store.recording_completion(job.id).unwrap(),
        Some(completion.clone())
    );
    store
        .conn
        .execute_batch("DROP TRIGGER synthetic_full")
        .unwrap();
    assert!(store.commit_recording_completion(&completion).unwrap());
}
