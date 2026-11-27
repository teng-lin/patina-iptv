use super::*;
use host::until_cancelled;
use nsh_core::element::create_root;

fn operation() -> InstallOperation {
    InstallOperation {
        owner: Owner::current(),
        busy: signal(true),
        panel_open: signal(true),
        focus: signal(0),
        pending_dismissed: signal(true),
        revision: signal(7),
    }
}

#[test]
fn cancelling_authorization_drops_the_wait_and_busy_guard_without_dispatching_a_late_error() {
    use std::{
        cell::Cell,
        future::Future,
        rc::Rc,
        task::{Context, Poll, Waker},
    };
    struct Dropped(Rc<Cell<bool>>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (owner, ()) = create_root(|| {
        let operation = operation();
        let busy = operation.busy;
        let dropped = Rc::new(Cell::new(false));
        let dispatched = Cell::new(false);
        let cancel_requested = Cell::new(false);
        let cancelled_flag = &cancel_requested;
        let guard = Dropped(Rc::clone(&dropped));
        let (complete, held) = futures_channel::oneshot::channel::<Result<(), &'static str>>();
        let (_cancel, cancelled) = futures_channel::oneshot::channel::<()>();
        let mut task = std::pin::pin!(async {
            let _operation = operation;
            let result = until_cancelled(
                async move {
                    let _guard = guard;
                    held.await.unwrap()
                },
                async move {
                    if !cancelled_flag.get() {
                        let _ = cancelled.await;
                    }
                },
                || cancel_requested.get(),
            )
            .await;
            if result.is_some() {
                dispatched.set(true);
            }
        });
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(task.as_mut().poll(&mut context), Poll::Pending);
        assert!(busy.get_untracked());
        assert!(!dropped.get());
        cancel_requested.set(true);
        // The timer has not fired. Its cancellation future remains suspended
        // in its oneshot wait when the authorization result wakes this task.
        // A late verification error is ready in the same shell turn. Cancel
        // still wins, so no notification, browser route or handoff is dispatched.
        complete
            .send(Err("synthetic verification failure"))
            .unwrap();
        assert_eq!(task.as_mut().poll(&mut context), Poll::Ready(()));
        assert!(dropped.get(), "the authorization waiter is released");
        assert!(!busy.get_untracked(), "another update can now be started");
        assert!(!dispatched.get());
    });
    owner.dispose();
}

#[test]
fn completing_after_later_focuses_the_ready_notice_review_action() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (owner, ()) = create_root(|| {
        let operation = operation();
        let (busy, panel_open, focus, dismissed, revision) = (
            operation.busy,
            operation.panel_open,
            operation.focus,
            operation.pending_dismissed,
            operation.revision,
        );
        // The accepted task outlives the panel. Later is control 3, while the
        // ready notice has only Review (0) and Later (1).
        focus.set(3);
        panel_open.set(false);
        drop(operation);
        assert!(!busy.get_untracked());
        assert!(!dismissed.get_untracked(), "the ready notice is eligible");
        assert_eq!(revision.get_untracked(), 8, "the view is notified");
        assert_eq!(focus.get_untracked(), 0, "Review has visible remote focus");
        assert_eq!(
            notice_navigation(focus, Action::Select),
            Some(NoticeAction::Review)
        );

        // The other visible button remains reachable and routes to dismissal.
        assert_eq!(notice_navigation(focus, Action::MoveRight), None);
        assert_eq!(focus.get_untracked(), 1);
        assert_eq!(
            notice_navigation(focus, Action::Select),
            Some(NoticeAction::Dismiss)
        );
    });
    owner.dispose();
}

#[test]
fn completing_with_the_panel_open_preserves_its_current_remote_focus() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (owner, ()) = create_root(|| {
        let operation = operation();
        let focus = operation.focus;
        focus.set(2);
        drop(operation);
        assert_eq!(focus.get_untracked(), 2);
    });
    owner.dispose();
}

#[test]
fn abandoned_operation_does_not_write_disposed_panel_signals() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (owner, operation) = create_root(operation);
    owner.dispose();
    // A root teardown can drop accepted tasks after its reactive state.
    // Guard cleanup must tolerate those already-disposed signals.
    drop(operation);
}

#[test]
fn retained_package_does_not_hide_manual_check_progress_or_failure() {
    crate::i18n::in_language("en", || {
        use iptv_runtime::app_updates::github::{DownloadLink, ReleaseCheck, ReleaseInfo};
        let mut state = Snapshot {
            checking: true,
            cached: Some(ReleaseCheck {
                installed_version: "1.0.0".into(),
                status: ReleaseStatus::UpdateAvailable,
                release: ReleaseInfo {
                    version: "2.0.0".into(),
                    notes: "Synthetic release notes".into(),
                    published_at: None,
                    url: "https://github.com/synthetic/patina/releases/tag/v2.0.0".into(),
                    download: Some(DownloadLink {
                        name: "synthetic-package".into(),
                        url: "https://github.com/synthetic/patina/releases/download/v2.0.0/synthetic-package".into(),
                        size: 9,
                    }),
                },
            }),
            ..Snapshot::default()
        };
        let retained_release = state.cached.clone();
        // A retained package must remain installable without replacing the
        // result of the explicit check the viewer just requested.
        assert_eq!(status_for(&state, true, false, None, true), "Checking…");
        state.checking = false;
        state.error = Some(CheckError::Network);
        assert_eq!(
            status_for(&state, true, false, None, true),
            "Could not connect to GitHub. Check your connection and try again."
        );
        assert!(
            panel_controls(true, true, false, false, true, false).contains(&1),
            "installation remains available"
        );
        assert_eq!(state.cached, retained_release);
        state.error = None;
        assert_eq!(
            status_for(&state, true, false, None, true),
            "Update ready to install."
        );
        assert_eq!(
            status_for(&state, true, true, Some((3, 9)), true),
            "Downloading update: 33%"
        );
    });
}

#[test]
fn manual_package_route_is_reachable_between_install_and_skip_without_hidden_focus_stops() {
    // Control identities follow the visual order, including the separately
    // confirmed browser route when authenticated installation is available.
    assert_eq!(
        panel_controls(true, true, true, false, true, false),
        [2, 3, 4]
    );
    assert_eq!(
        panel_controls(true, true, false, false, true, false),
        [0, 1, 5, 2, 3]
    );
    assert_eq!(
        panel_controls(true, false, false, false, true, false),
        [0, 1, 2, 3]
    );
    assert_eq!(
        panel_controls(false, false, false, false, true, false),
        [0, 3]
    );
    assert_eq!(
        panel_controls(false, false, true, false, true, false),
        [3, 4]
    );
    assert_eq!(panel_controls(true, true, false, true, true, false), [2, 3]);
    assert_eq!(panel_controls(false, false, false, true, true, false), [3]);
    assert_eq!(
        panel_controls(true, true, true, false, false, false),
        [3],
        "irrevocable native extraction keeps only Later remote-actionable"
    );
}

#[component]
fn UpdateOverlap() -> impl IntoElement {
    use iptv_runtime::app_updates::github::{DownloadLink, ReleaseCheck, ReleaseInfo};
    use std::rc::Rc;
    let directory = tempfile::tempdir().unwrap();
    crate::tasks::bind_owner();
    let application = Rc::new(crate::runtime_owner::new_runtime(
        directory.path().to_owned(),
    ));
    let ui = crate::runtime_owner::with_runtime(application.clone(), || {
        let ui = Ui::new(
            crate::SharedUi::new(),
            crate::playback::player::Player::new(),
        );
        update_ui(ui).ready.set(true);
        let release = |version: &str| ReleaseCheck {
            installed_version: "1.0.0".into(),
            status: ReleaseStatus::UpdateAvailable,
            release: ReleaseInfo {
                version: version.into(),
                notes: "Generated notes".into(),
                published_at: None,
                url: "https://github.com/synthetic/patina/releases".into(),
                download: Some(DownloadLink {
                    name: "synthetic-package".into(),
                    url: format!("https://example.test/synthetic-{version}"),
                    size: 9,
                }),
            },
        };
        let token = application
            .begin_app_update_check(1, Trigger::Manual, Frequency::Never)
            .unwrap();
        assert!(application.finish_app_update_check(token, 2, Ok(release("2.0.0"))));

        // An accepted installation holds its displayed release stable across
        // manual, startup and timer checks until its guard completes.
        update_ui(ui).install_busy.set(true);
        let installation = InstallOperation::new(ui);
        for trigger in [Trigger::Manual, Trigger::Startup, Trigger::Automatic] {
            check(ui, trigger);
            let state = application.app_update_snapshot();
            assert!(!state.checking, "a competing release check was not started");
            assert_eq!(state.cached.unwrap().release.version, "2.0.0");
        }
        drop(installation);

        // Conversely, a held check must finish before a cached older release
        // can authorize any installation or browser download action.
        let token = application
            .begin_app_update_check(3, Trigger::Manual, Frequency::Never)
            .unwrap();
        download(ui);
        manual_download(ui);
        update_ui(ui).focus.set(1);
        key(ui, Action::Select);
        update_ui(ui).focus.set(5);
        key(ui, Action::Select);
        assert!(!update_ui(ui).install_busy.get_untracked());
        assert!(application.app_update_snapshot().checking);
        assert!(application.finish_app_update_check(token, 4, Ok(release("3.0.0"))));
        assert_eq!(
            application
                .app_update_snapshot()
                .cached
                .unwrap()
                .release
                .version,
            "3.0.0"
        );
        assert!(!application.has_pending_app_update());
        ui
    });
    cleanup(move || drop(directory));
    view()
        .child(text("No overlapping update operations"))
        .child(text("Open verified fixture route").on_press(move || {
            crate::runtime_owner::with_runtime(application.clone(), || manual_download(ui));
        }))
}

#[test]
fn release_checks_and_accepted_installation_cannot_replace_each_others_version() {
    let mut app = nsh_testing::Headless::start(nsh_engine::EngineConfig::new(
        nsh_core::element::Root::of::<UpdateOverlap>(),
    ))
    .unwrap();
    app.wait_for("text:No overlapping update operations")
        .unwrap();
    assert!(
        app.screen().opened_urls().is_empty(),
        "held check blocks browser actions"
    );
    app.press("text:Open verified fixture route").unwrap();
    assert_eq!(
        app.screen().opened_urls(),
        ["https://example.test/synthetic-3.0.0"]
    );
}
