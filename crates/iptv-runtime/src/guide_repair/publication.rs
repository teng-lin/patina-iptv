//! Preserve committed results while cache publication/refresh runs separately.
use super::*;
use crate::stored;

pub(super) async fn after_commit(
    owner: &Rc<Runtime>,
    commit: Commit,
    edit: u64,
    canceled: Option<crate::library::ImportCompletion>,
) -> Outcome {
    let id = commit.id.clone();
    let source = commit.source;
    {
        let mut state = owner.repairs.borrow_mut();
        state.plans.remove(&id);
        if !state.operations.contains_key(&id) && state.operations.len() >= MAX_OPERATIONS {
            if let Some(old) = state
                .operations
                .iter()
                .find(|(_, op)| {
                    !matches!(
                        op.phase,
                        OperationPhase::Pending | OperationPhase::Refreshing
                    )
                })
                .map(|(id, _)| id.clone())
            {
                state.operations.remove(&old);
            } else {
                // Admission reserves capacity before commit; retain an honest
                // durable receipt even if a future caller violates that rule.
                return Outcome {
                    commit: commit.clone(),
                    configuration_committed: true,
                    mapping_saved: true,
                    effective: Effective::Pending,
                    verification_pending: true,
                    operation: id,
                };
            }
        }
        state.operations.insert(
            id.clone(),
            Operation {
                id: id.clone(),
                commit: commit.clone(),
                phase: OperationPhase::Pending,
                effective: Effective::Pending,
                verification: None,
            },
        );
    }
    let outcome = Outcome {
        commit: commit.clone(),
        configuration_committed: true,
        mapping_saved: true,
        effective: Effective::Pending,
        verification_pending: true,
        operation: id.clone(),
    };
    // The durable mutation has already committed. Without an executor it
    // cannot be actively pending; keep an evictable receipt that callers can
    // verify when they have a host.
    if !owner.has_tasks() {
        if let Some(operation) = owner.repairs.borrow_mut().operations.get_mut(&id) {
            operation.phase = OperationPhase::ReadyToVerify;
        }
        return outcome;
    }
    let owner = owner.clone();
    let _ = owner.clone().try_spawn(async move {
        let current_operation = || {
            owner
                .repairs
                .borrow()
                .operations
                .get(&id)
                .is_some_and(|op| {
                    op.commit.revisions == commit.revisions
                        && op.commit.undone == commit.undone
                        && op.phase != OperationPhase::Canceled
                })
        };
        if !current_operation() {
            return;
        }
        let keep = stored::stored_past_days();
        let read = id.clone();
        let published = owner.services().epg().repair_publication(read, keep).await;
        let still_current = crate::stored::guide_edit_revision(&owner, source) == edit;
        let effective = match published {
            Ok(Some(rebuilt)) => {
                if still_current && crate::data::repair_source_shown(&owner, source) {
                    let imported = Box::new(crate::imports::Imported::current(*rebuilt));
                    crate::data::show_repair(&owner, imported, owner.browser_generation(), edit)
                        .await;
                }
                Effective::Installed
            }
            Ok(None) => Effective::Pending,
            Err(_) => Effective::Failed,
        };
        if !current_operation() {
            return;
        }
        if let Some(op) = owner.repairs.borrow_mut().operations.get_mut(&id) {
            op.effective = effective;
            op.phase = if still_current {
                OperationPhase::ReadyToVerify
            } else {
                OperationPhase::Superseded
            };
        }
        if still_current && effective != Effective::Installed {
            schedule(&owner, &commit, canceled);
        }
        owner.emit(crate::RuntimeEvent::GuideChanged);
    });
    outcome
}

fn refresh_observed(
    owner: &Rc<Runtime>,
    id: String,
    source: SourceId,
    expected: Revisions,
    undone: bool,
    accepted: bool,
) {
    if !owner.has_tasks() {
        return;
    }
    let read = crate::inspection::query_with_deadline(
        owner,
        InspectionControl::default(),
        Duration::from_secs(3),
        move |store| {
            Ok(Ok(store.guide_repair_snapshot(|store| {
                Ok((
                    store.guide_revisions(source)?,
                    store
                        .epg_guide(source)?
                        .is_some_and(|guide| guide.last_error.is_some()),
                ))
            })?))
        },
    );
    let owner = owner.clone();
    owner.clone().spawn(async move {
        let observed = read.await;
        let mut state = owner.repairs.borrow_mut();
        let Some(status) = state.operations.get_mut(&id) else {
            return;
        };
        if status.commit.revisions != expected
            || status.commit.undone != undone
            || matches!(
                status.phase,
                OperationPhase::Superseded | OperationPhase::Canceled
            )
        {
            return;
        }
        match observed {
            Ok((current, _))
                if current.mapping != expected.mapping
                    || current.configuration != expected.configuration =>
            {
                status.phase = OperationPhase::Superseded;
            }
            Ok((_, false)) if accepted => {
                if status.phase != OperationPhase::Verified {
                    status.phase = OperationPhase::ReadyToVerify;
                }
            }
            _ => {
                status.phase = OperationPhase::Failed;
                status.effective = Effective::Failed;
            }
        }
    });
}

fn schedule(owner: &Rc<Runtime>, commit: &Commit, after: Option<crate::library::ImportCompletion>) {
    if !owner.has_tasks() {
        return;
    }
    let cancel = CancelToken::new();
    let id = commit.id.clone();
    let source = commit.source;
    let expected = commit.revisions;
    let undone = commit.undone;
    {
        let mut state = owner.repairs.borrow_mut();
        state.revoke(source);
        state.refreshes.insert(source, (id.clone(), cancel.clone()));
        if let Some(operation) = state.operations.get_mut(&id) {
            operation.phase = OperationPhase::Pending;
        }
    }
    let owner = owner.clone();
    owner.clone().spawn(async move {
        // Coalesce consecutive batches before acquiring any network connection.
        owner.delay(Duration::from_secs(2)).await;
        if cancel.is_canceled() {
            return;
        }
        if let Some(operation) = owner.repairs.borrow_mut().operations.get_mut(&id) {
            operation.phase = OperationPhase::Refreshing;
        }
        // The normal refresh registration revokes the previous source job.
        // Remove our debounce marker first so it cannot revoke itself.
        owner.repairs.borrow_mut().refreshes.remove(&source);
        let finished = owner.clone();
        let operation = id.clone();
        if !crate::data::restart_repair_update(&owner, source, after, move |accepted| {
            refresh_observed(&finished, operation, source, expected, undone, accepted);
        }) && let Some(status) = owner.repairs.borrow_mut().operations.get_mut(&id)
        {
            status.phase = OperationPhase::Failed;
            status.effective = Effective::Failed;
        }
        if !cancel.is_canceled() {
            owner
                .repairs
                .borrow_mut()
                .refreshes
                .insert(source, (id, cancel));
        }
    });
}
