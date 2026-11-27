use super::super::github::ReleaseInfo;
use super::*;

fn release(version: &str) -> ReleaseCheck {
    ReleaseCheck {
        installed_version: "1.0.0".into(),
        release: ReleaseInfo {
            version: version.into(),
            notes: "Synthetic release notes".into(),
            published_at: None,
            url: "https://github.com/synthetic/patina/releases/tag/v2.0.0".into(),
            download: None,
        },
        status: ReleaseStatus::NoCompatiblePackage,
    }
}

fn complete(
    state: &mut Controller,
    now: i64,
    trigger: Trigger,
    frequency: Frequency,
    version: &str,
) {
    let token = state
        .begin_check(now, trigger, frequency)
        .expect("check due");
    assert!(state.finish_check(token, now, Ok(release(version))));
}

#[test]
fn weekly_default_checks_due_at_startup_resume_and_while_running() {
    assert_eq!(Frequency::default(), Frequency::Weekly);
    assert_eq!(Frequency::from_setting("unknown"), Frequency::Weekly);
    let mut state = Controller::default();
    complete(
        &mut state,
        1000,
        Trigger::Startup,
        Frequency::Weekly,
        "2.0.0",
    );
    assert!(
        state
            .begin_check(1000 + 7 * 86_400 - 1, Trigger::Automatic, Frequency::Weekly)
            .is_none()
    );
    assert!(
        state
            .begin_check(1000 + 7 * 86_400, Trigger::Automatic, Frequency::Weekly)
            .is_some()
    );
    // Polling and entering About cannot race the in-flight operation.
    assert!(
        state
            .begin_check(1000 + 8 * 86_400, Trigger::Manual, Frequency::Weekly)
            .is_none()
    );
}

#[test]
fn never_is_manual_only_and_on_launch_is_once_per_owner() {
    let mut state = Controller::default();
    for trigger in [Trigger::Startup, Trigger::Automatic] {
        assert!(state.begin_check(1, trigger, Frequency::Never).is_none());
    }
    complete(&mut state, 1, Trigger::Manual, Frequency::Never, "2.0.0");
    complete(
        &mut state,
        2,
        Trigger::Startup,
        Frequency::OnLaunch,
        "2.0.0",
    );
    assert!(
        state
            .begin_check(3, Trigger::Startup, Frequency::OnLaunch)
            .is_none()
    );
    assert!(
        state
            .begin_check(86_400, Trigger::Automatic, Frequency::OnLaunch)
            .is_none()
    );
    let mut restarted = Controller::default();
    restarted.restore(state.document()).unwrap();
    assert!(
        restarted
            .begin_check(4, Trigger::Startup, Frequency::OnLaunch)
            .is_some()
    );
}

#[test]
fn monthly_is_calendar_month_with_short_month_and_leap_year_clamping() {
    let time = |text: &str| DateTime::parse_from_rfc3339(text).unwrap().timestamp();
    for (last, due) in [
        ("2028-01-31T12:34:56Z", "2028-02-29T12:34:56Z"),
        ("2027-01-31T12:34:56Z", "2027-02-28T12:34:56Z"),
        ("2027-12-15T12:34:56Z", "2028-01-15T12:34:56Z"),
    ] {
        let mut state = Controller::default();
        complete(
            &mut state,
            time(last),
            Trigger::Startup,
            Frequency::Monthly,
            "2.0.0",
        );
        assert!(
            state
                .begin_check(time(due) - 1, Trigger::Automatic, Frequency::Monthly)
                .is_none()
        );
        assert!(
            state
                .begin_check(time(due), Trigger::Automatic, Frequency::Monthly)
                .is_some()
        );
    }
}

#[test]
fn failures_back_off_without_advancing_success_or_losing_cached_release() {
    let mut state = Controller::default();
    complete(&mut state, 1000, Trigger::Manual, Frequency::Daily, "2.0.0");
    let token = state
        .begin_check(1000 + 86_400, Trigger::Automatic, Frequency::Daily)
        .unwrap();
    state.finish_check(token, 1000 + 86_400, Err(CheckError::Network));
    let snapshot = state.snapshot();
    assert_eq!(snapshot.last_success, Some(1000));
    assert_eq!(snapshot.last_attempt, Some(1000 + 86_400));
    assert_eq!(snapshot.cached, Some(release("2.0.0")));
    assert_eq!(snapshot.error, Some(CheckError::Network));
    assert_eq!(snapshot.retry_after, Some(1000 + 86_400 + 900));
    let mut restarted = Controller::default();
    restarted.restore(state.document()).unwrap();
    assert!(
        restarted
            .begin_check(1000 + 86_400 + 899, Trigger::Startup, Frequency::Daily)
            .is_none()
    );
    let token = restarted
        .begin_check(1000 + 86_400 + 900, Trigger::Automatic, Frequency::Daily)
        .unwrap();
    restarted.finish_check(token, 1000 + 86_400 + 900, Err(CheckError::RateLimited));
    assert_eq!(
        restarted.snapshot().retry_after,
        Some(1000 + 86_400 + 900 + 3600)
    );
    // An explicit user request always retries, including during backoff.
    complete(
        &mut restarted,
        1000 + 86_400 + 901,
        Trigger::Manual,
        Frequency::Never,
        "2.0.0",
    );
    assert_eq!(restarted.snapshot().retry_after, None);
}

#[test]
fn skipped_and_seen_versions_survive_restart_but_manual_check_still_shows_them() {
    let mut state = Controller::default();
    complete(&mut state, 0, Trigger::Startup, Frequency::Daily, "2.0.0");
    assert_eq!(state.snapshot().notice, Some(release("2.0.0")));
    state.skip_version("2.0.0");
    let mut restarted = Controller::default();
    restarted.restore(state.document()).unwrap();
    complete(
        &mut restarted,
        86_400,
        Trigger::Startup,
        Frequency::Daily,
        "2.0.0",
    );
    assert!(restarted.snapshot().notice.is_none());
    complete(
        &mut restarted,
        86_401,
        Trigger::Manual,
        Frequency::Never,
        "2.0.0",
    );
    assert_eq!(restarted.snapshot().cached, Some(release("2.0.0")));
    complete(
        &mut restarted,
        2 * 86_400 + 1,
        Trigger::Automatic,
        Frequency::Daily,
        "3.0.0",
    );
    assert_eq!(restarted.snapshot().notice, Some(release("3.0.0")));
    restarted.acknowledge_notice();
    let mut again = Controller::default();
    again.restore(restarted.document()).unwrap();
    complete(
        &mut again,
        3 * 86_400 + 1,
        Trigger::Startup,
        Frequency::Daily,
        "3.0.0",
    );
    assert!(again.snapshot().notice.is_none());
}

#[test]
fn stale_completions_cannot_clear_or_replace_current_check() {
    let mut state = Controller::default();
    let first = state
        .begin_check(0, Trigger::Manual, Frequency::Never)
        .unwrap();
    assert!(state.finish_check(first, 1, Ok(release("2.0.0"))));
    let second = state
        .begin_check(2, Trigger::Manual, Frequency::Never)
        .unwrap();
    assert!(!state.finish_check(first, 3, Ok(release("9.0.0"))));
    assert!(state.snapshot().checking);
    assert_eq!(state.snapshot().cached, Some(release("2.0.0")));
    assert!(state.finish_check(second, 4, Ok(release("3.0.0"))));
}

#[test]
fn opening_updates_joins_scheduled_work_without_a_second_request_or_duplicate_notice() {
    let mut state = Controller::default();
    let token = state
        .begin_check(0, Trigger::Startup, Frequency::Weekly)
        .unwrap();
    assert!(
        state
            .begin_check(1, Trigger::Manual, Frequency::Weekly)
            .is_none()
    );
    assert!(state.snapshot().checking);
    assert!(state.finish_check(token, 2, Ok(release("2.0.0"))));
    assert_eq!(state.snapshot().cached, Some(release("2.0.0")));
    assert!(state.snapshot().notice.is_none());
    assert_eq!(state.durable.notified_version.as_deref(), Some("2.0.0"));
}

#[test]
fn pending_notice_refresh_is_not_a_repeating_check_and_clear_actions_are_durable() {
    for frequency in [Frequency::Weekly, Frequency::Monthly] {
        let mut state = Controller::default();
        complete(&mut state, 1000, Trigger::Startup, frequency, "2.0.0");
        assert_eq!(state.document()["pending_version"], "2.0.0");
        // A notice deferred by full-screen viewing is already held by this owner.
        assert!(
            state
                .begin_check(1001, Trigger::Automatic, frequency)
                .is_none()
        );
        for action in ["acknowledge", "skip", "manual", "current"] {
            let mut restarted = Controller::default();
            restarted.restore(state.document()).unwrap();
            complete(&mut restarted, 1002, Trigger::Startup, frequency, "2.0.0");
            assert!(restarted.snapshot().notice.is_some());
            match action {
                "acknowledge" => restarted.acknowledge_notice(),
                "skip" => restarted.skip_version("2.0.0"),
                "manual" => complete(&mut restarted, 1003, Trigger::Manual, frequency, "2.0.0"),
                _ => {
                    let token = restarted
                        .begin_check(1003, Trigger::Manual, frequency)
                        .unwrap();
                    let mut current = release("2.0.0");
                    current.status = ReleaseStatus::Current;
                    restarted.finish_check(token, 1003, Ok(current));
                }
            }
            assert!(
                restarted.document()["pending_version"].is_null(),
                "{action}"
            );
            let mut again = Controller::default();
            again.restore(restarted.document()).unwrap();
            assert!(
                again
                    .begin_check(1004, Trigger::Startup, frequency)
                    .is_none(),
                "{action}"
            );
        }
    }
}

#[tokio::test]
async fn unseen_release_is_refreshed_after_disk_restart_before_normal_due_time() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("profile");
    let make = || {
        crate::Runtime::new(crate::Ports::new(directory.clone(), |_| {
            Err(crate::model::StoreError::unavailable(
                "synthetic unavailable key",
            ))
        }))
    };
    {
        let owner = make();
        owner.start_services();
        owner.restore_app_update_state().await.unwrap();
        let token = owner
            .begin_app_update_check(1000, Trigger::Startup, Frequency::Monthly)
            .unwrap();
        owner.finish_app_update_check(token, 1001, Ok(release("2.0.0")));
        // Full-screen playback prevents this notice being shown before exit.
        assert!(owner.app_update_snapshot().notice.is_some());
        owner.persist_app_update_state().await.unwrap();
    }
    {
        let owner = make();
        owner.start_services();
        owner.restore_app_update_state().await.unwrap();
        assert!(owner.app_update_snapshot().cached.is_none());
        assert!(
            owner
                .begin_app_update_check(1002, Trigger::Startup, Frequency::Never)
                .is_none()
        );
        let token = owner
            .begin_app_update_check(1002, Trigger::Startup, Frequency::Monthly)
            .unwrap();
        owner.finish_app_update_check(token, 1003, Err(CheckError::Network));
        owner.persist_app_update_state().await.unwrap();
    }
    {
        let owner = make();
        owner.start_services();
        owner.restore_app_update_state().await.unwrap();
        assert!(
            owner
                .begin_app_update_check(1902, Trigger::Startup, Frequency::Monthly)
                .is_none()
        );
        let token = owner
            .begin_app_update_check(1903, Trigger::Automatic, Frequency::Monthly)
            .unwrap();
        owner.finish_app_update_check(token, 1904, Ok(release("2.0.0")));
        assert_eq!(owner.app_update_snapshot().notice, Some(release("2.0.0")));
        assert!(
            owner
                .begin_app_update_check(1905, Trigger::Automatic, Frequency::Monthly)
                .is_none()
        );
        owner.acknowledge_app_update_notice();
        owner.persist_app_update_state().await.unwrap();
    }
    let owner = make();
    owner.start_services();
    owner.restore_app_update_state().await.unwrap();
    assert!(
        owner
            .begin_app_update_check(2000, Trigger::Startup, Frequency::Monthly)
            .is_none()
    );
}

#[test]
fn current_or_ahead_results_clear_obsolete_notice_and_clock_rollback_remains_usable() {
    let mut state = Controller::default();
    complete(
        &mut state,
        1000,
        Trigger::Startup,
        Frequency::Weekly,
        "2.0.0",
    );
    let token = state
        .begin_check(900, Trigger::Automatic, Frequency::Weekly)
        .unwrap();
    let mut current = release("2.0.0");
    current.status = ReleaseStatus::Current;
    state.finish_check(token, 900, Ok(current));
    assert!(state.snapshot().notice.is_none());
    assert!(
        state
            .begin_check(901, Trigger::Automatic, Frequency::Weekly)
            .is_none()
    );

    // Failure backoff also uses the corrected clock, rather than suppressing
    // discovery until the wall clock catches up to a previous incorrect date.
    let token = state
        .begin_check(1000, Trigger::Manual, Frequency::Never)
        .unwrap();
    state.finish_check(token, 1000, Err(CheckError::Network));
    assert!(
        state
            .begin_check(800, Trigger::Automatic, Frequency::Weekly)
            .is_some()
    );
}

#[tokio::test]
async fn actual_runtime_persists_schedule_beside_profile_and_reloads_on_restart() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("profile");
    let make = || {
        crate::Runtime::new(crate::Ports::new(directory.clone(), |_| {
            Err(crate::model::StoreError::unavailable(
                "synthetic unavailable key",
            ))
        }))
    };
    {
        let owner = make();
        owner.start_services();
        owner.restore_app_update_state().await.unwrap();
        let token = owner
            .begin_app_update_check(1000, Trigger::Startup, Frequency::Weekly)
            .unwrap();
        owner.finish_app_update_check(token, 1001, Ok(release("2.0.0")));
        owner.skip_app_update_version("2.0.0");
        owner.persist_app_update_state().await.unwrap();
    }
    // A restored/replaced library must not reset the automatic discovery timer.
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
    let owner = make();
    owner.start_services();
    owner.restore_app_update_state().await.unwrap();
    assert_eq!(owner.app_update_snapshot().last_success, Some(1001));
    assert_eq!(
        owner.app_update_snapshot().skipped_version.as_deref(),
        Some("2.0.0")
    );
    assert!(
        owner
            .begin_app_update_check(1002, Trigger::Startup, Frequency::Weekly)
            .is_none()
    );
    let token = owner
        .begin_app_update_check(1003, Trigger::Manual, Frequency::Never)
        .unwrap();
    owner.finish_app_update_check(token, 1004, Ok(release("2.0.0")));
    assert_eq!(owner.app_update_snapshot().cached, Some(release("2.0.0")));
}

#[tokio::test]
async fn corrupt_schedule_reports_failure_and_leaves_manual_discovery_available() {
    let owner = crate::Runtime::default();
    owner.start_services();
    let file = path(owner.data_dir());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"not JSON").unwrap();
    assert!(owner.restore_app_update_state().await.is_err());
    assert!(
        owner
            .begin_app_update_check(1, Trigger::Manual, Frequency::Never)
            .is_some()
    );
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
}
