use super::*;
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
            panel_controls(true, true, false).contains(&1),
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
    assert_eq!(panel_controls(true, true, true), [0, 1, 5, 2, 3, 4]);
    assert_eq!(panel_controls(true, true, false), [0, 1, 5, 2, 3]);
    assert_eq!(panel_controls(true, false, false), [0, 1, 2, 3]);
    assert_eq!(panel_controls(false, false, false), [0, 3]);
    assert_eq!(panel_controls(false, false, true), [0, 3, 4]);
}
