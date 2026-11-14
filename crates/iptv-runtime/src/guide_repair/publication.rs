//! Preserve committed results while cache publication/refresh runs separately.
use super::*;
use crate::{
    imports::{Imported, Streams},
    stored::{self, Matching},
};
use iptv_epg::ChannelKey;
use iptv_store_sqlite::{EpgOverride, Store, StoreError};

fn rebind(
    store: &mut Store,
    id: &str,
    keep: u32,
) -> std::result::Result<Option<Box<Imported>>, StoreError> {
    let Some((commit, choices, config_changed)) = store.guide_repair_publication(id)? else {
        return Err(StoreError::Unavailable("guide repair commit is missing"));
    };
    let revisions = store.guide_revisions(commit.source)?;
    if revisions.mapping != commit.mapping_revision
        || revisions.configuration != commit.configuration_revision
    {
        return Err(StoreError::Unavailable("guide repair was superseded"));
    }
    if config_changed || choices.iter().any(|(_, choice)| choice.is_none()) {
        return Ok(None);
    }
    for (_, choice) in &choices {
        if let Some(value @ EpgOverride::Xmltv { .. }) = choice {
            let Some(feed) = value.feed() else {
                return Ok(None);
            };
            if store
                .inspection_stored_feed_incarnation(commit.source, feed)?
                .is_none()
            {
                return Ok(None);
            }
        }
    }
    let cached = store
        .epg_guide(commit.source)?
        .is_some_and(|guide| guide.replaced_at.is_some());
    let (groups, channels) = stored::catalog_in(store, commit.source)?;
    let overrides = store
        .epg_overrides(commit.source)?
        .into_iter()
        .map(|(key, value)| (ChannelKey::new(key.as_str()), value.to_override()))
        .collect();
    let fingerprint = Matching {
        channels: &channels,
        overrides: &overrides,
    }
    .fingerprint();
    let mut all = true;
    for (channel, choice) in choices {
        let selected = match &choice {
            Some(EpgOverride::Disabled) if !cached => continue,
            Some(EpgOverride::Disabled) => None,
            Some(value @ EpgOverride::Xmltv { epg_channel, .. }) => {
                value.feed().map(|feed| (feed, epg_channel.as_str()))
            }
            None => return Ok(None),
        };
        all &= store.rebind_epg(
            commit.source,
            &ChannelKey::new(channel.as_str()),
            selected,
            fingerprint,
        )?;
    }
    if !all {
        return Ok(None);
    }
    let now = now();
    Ok(Some(Box::new(Imported {
        guide: store.load_epg(
            commit.source,
            &crate::paging::store_window(crate::paging::window(now, keep)),
        )?,
        groups,
        channels,
        playback: Streams::Current,
        now,
        source: commit.source,
        resume: false,
    })))
}

pub(super) async fn after_commit(
    owner: &Rc<Runtime>,
    commit: Commit,
    edit: u64,
    cancelled: Option<crate::library::ImportCompletion>,
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
    let owner = owner.clone();
    owner.clone().spawn(async move {
        let current_operation = || {
            owner
                .repairs
                .borrow()
                .operations
                .get(&id)
                .is_some_and(|op| {
                    op.commit.revisions == commit.revisions
                        && op.commit.undone == commit.undone
                        && op.phase != OperationPhase::Cancelled
                })
        };
        if !current_operation() {
            return;
        }
        let keep = stored::keep_past_days(&owner.settings());
        let read = id.clone();
        let published = owner
            .services()
            .call(move |store| rebind(store, &read, keep))
            .await;
        let still_current = crate::stored::guide_edit_revision(&owner, source) == edit;
        let effective = match published {
            Ok(Some(imported)) => {
                if still_current && crate::data::repair_source_shown(&owner, source) {
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
            schedule(&owner, &commit, cancelled);
        }
        owner.emit(crate::RuntimeEvent::GuideChanged);
    });
    outcome
}

fn schedule(owner: &Rc<Runtime>, commit: &Commit, after: Option<crate::library::ImportCompletion>) {
    let cancel = CancelToken::new();
    let id = commit.id.clone();
    let source = commit.source;
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
        if cancel.is_cancelled() {
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
        if !crate::data::stored::restart_repair_update(&owner, source, after, move |accepted| {
            let mut state = finished.repairs.borrow_mut();
            if let Some(status) = state.operations.get_mut(&operation)
                && !matches!(
                    status.phase,
                    OperationPhase::Superseded | OperationPhase::Cancelled
                )
            {
                status.phase = if accepted {
                    OperationPhase::ReadyToVerify
                } else {
                    OperationPhase::Failed
                };
            }
        }) && let Some(status) = owner.repairs.borrow_mut().operations.get_mut(&id)
        {
            status.phase = OperationPhase::Failed;
        }
        if !cancel.is_cancelled() {
            owner
                .repairs
                .borrow_mut()
                .refreshes
                .insert(source, (id, cancel));
        }
    });
}
