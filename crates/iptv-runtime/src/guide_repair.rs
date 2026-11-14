//! Application-owned immutable repair previews, atomic commit and observed status.
mod prepare;
mod publication;
#[cfg(test)]
mod tests;
mod write;
use crate::{
    Runtime,
    inspection::{InspectionControl, ProbeLease},
};
use iptv_import::CancelToken;
pub use iptv_library::guide_repair::*;
use iptv_library::inspection::{CandidateRef, ChannelRef, InspectionError, TimeWindow};
use iptv_library::{Choice, SourceId};
use iptv_store_sqlite::{RepairBatch, RepairCommit, RepairResult};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    rc::Rc,
    time::{Duration, Instant},
};

const MAX_PLANS: usize = 32;
const MAX_OPERATIONS: usize = 128;
const PREVIEW_DEADLINE: Duration = Duration::from_secs(15);

#[derive(Clone)]
struct StoredPlan {
    request: Request,
    preview: Preview,
    rows: Vec<MappingPreview>,
    indirect: Vec<IndirectEffect>,
    batch: RepairBatch,
    epoch: u64,
}
#[derive(Default)]
pub(crate) struct State {
    plans: HashMap<String, StoredPlan>,
    applying: HashSet<String>,
    operations: HashMap<String, Operation>,
    refreshes: HashMap<SourceId, (String, CancelToken)>,
}
impl State {
    fn reserve(&mut self, id: &str) -> Result<()> {
        if self.applying.contains(id) {
            return Err(Error::Busy);
        }
        let occupied: HashSet<_> = self.operations.keys().chain(self.applying.iter()).collect();
        if !occupied.contains(&id.to_owned()) && occupied.len() >= MAX_OPERATIONS {
            let old = self
                .operations
                .iter()
                .filter(|(key, op)| {
                    !self.applying.contains(*key)
                        && !matches!(
                            op.phase,
                            OperationPhase::Pending | OperationPhase::Refreshing
                        )
                })
                .min_by_key(|(_, op)| op.commit.created_at)
                .map(|(id, _)| id.clone());
            if let Some(old) = old {
                self.operations.remove(&old);
            } else {
                return Err(Error::Busy);
            }
        }
        self.applying.insert(id.into());
        Ok(())
    }
    pub(crate) fn revoke(&mut self, source: SourceId) {
        if let Some((id, token)) = self.refreshes.remove(&source) {
            token.cancel();
            if let Some(operation) = self.operations.get_mut(&id) {
                if operation.phase != OperationPhase::Cancelled {
                    operation.phase = OperationPhase::Superseded;
                }
            }
        }
    }
    pub(crate) fn clear(&mut self) {
        self.plans.clear();
        self.applying.clear();
        self.operations.clear();
        for (_, (_, cancel)) in self.refreshes.drain() {
            cancel.cancel();
        }
    }
}
struct Applying {
    owner: std::rc::Weak<Runtime>,
    id: String,
}
impl Drop for Applying {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            owner.repairs.borrow_mut().applying.remove(&self.id);
        }
    }
}
fn receipt(owner: &Runtime, saved: Commit) -> Outcome {
    let cached = owner.repairs.borrow().operations.get(&saved.id).cloned();
    Outcome {
        operation: saved.id.clone(),
        commit: saved,
        configuration_committed: true,
        mapping_saved: true,
        effective: cached
            .as_ref()
            .map_or(Effective::Pending, |op| op.effective),
        verification_pending: !cached
            .and_then(|op| op.verification)
            .is_some_and(|v| v.verified_coverage),
    }
}

fn now() -> i64 {
    crate::imports::now_unix()
}
fn random_id() -> Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| Error::Storage)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn digest_hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn commit(value: RepairCommit) -> Commit {
    Commit {
        id: value.id,
        source: value.source,
        created_at: value.created_at,
        undo_expires_at: value.expires_at,
        undone: value.undone,
        revisions: Revisions {
            mapping: value.mapping_revision,
            configuration: value.configuration_revision,
        },
        added_feeds: value.added_feeds,
    }
}
fn committed(value: RepairResult) -> Result<Commit> {
    match value {
        RepairResult::Committed(value) => Ok(commit(value)),
        RepairResult::Conflict(rows) => Err(Error::Conflict(rows)),
        RepairResult::Missing => Err(Error::CommitMissing),
        RepairResult::Expired => Err(Error::PlanExpired),
        RepairResult::Invalid => Err(Error::InvalidRequest),
    }
}
fn leases(owner: &Runtime, request: &Request) -> Result<Vec<ProbeLease>> {
    let mut evidence = Vec::new();
    for addition in &request.additions {
        evidence.push(&addition.evidence);
    }
    for mapping in &request.mappings {
        if let Target::Staged {
            evidence: staged, ..
        } = &mapping.target
        {
            evidence.push(staged);
        }
    }
    evidence.sort_by(|a, b| a.stage.cmp(&b.stage));
    evidence.dedup();
    evidence
        .into_iter()
        .map(|e| owner.lease_probe(e).map_err(Error::from))
        .collect()
}
fn validate_leases(leases: &[ProbeLease], plan: &StoredPlan) -> Result<()> {
    for held in leases {
        held.validate_alive()?;
    }
    for mapping in &plan.request.mappings {
        if let Target::Staged {
            evidence,
            epg_channel,
            ..
        } = &mapping.target
        {
            let held = leases
                .iter()
                .find(|l| l.evidence() == evidence)
                .ok_or(InspectionError::StaleEvidence)?;
            held.validate_candidate(&CandidateRef {
                source: plan.request.source,
                feed: evidence.feed.unwrap_or(iptv_epg::FeedId(0)),
                incarnation: evidence.incarnation.clone(),
                epg_channel: epg_channel.clone(),
            })?;
        }
    }
    Ok(())
}

impl Runtime {
    /// Implemented service surface, not a successful provider/credential probe.
    /// Host/storage readiness and source failures remain typed operation results.
    pub fn foundation_capabilities(&self) -> Vec<Capability> {
        [
            ("catalogue_queries", CapabilityState::Implemented),
            ("programme_queries", CapabilityState::Implemented),
            ("epg_inspection", CapabilityState::Implemented),
            ("epg_probe", CapabilityState::Implemented),
            ("epg_repair", CapabilityState::Implemented),
            ("epg_verification", CapabilityState::Implemented),
            ("epg_undo", CapabilityState::Implemented),
            ("provider_catchup", CapabilityState::ProviderUnverified),
            ("runtime_playback_control", CapabilityState::NotImplemented),
            ("recording", CapabilityState::NotImplemented),
            ("reminders", CapabilityState::NotImplemented),
            ("local_timeshift", CapabilityState::NotImplemented),
            ("vod_browsing", CapabilityState::NotImplemented),
            ("parental_pin", CapabilityState::NotImplemented),
            ("mcp", CapabilityState::NotImplemented),
        ]
        .into_iter()
        .map(|(name, state)| Capability { name, state })
        .collect()
    }

    pub fn preview_guide_repair(
        self: &Rc<Self>,
        request: Request,
        control: InspectionControl,
    ) -> impl Future<Output = Result<Preview>> + use<> {
        let owner = self.clone();
        let services = self.services();
        let epoch = services.epoch();
        let held = leases(self, &request);
        let id = random_id();
        let created_at = now();
        let prepared =
            crate::inspection::query_with_deadline(self, control, PREVIEW_DEADLINE, move |store| {
                let result = match (held, id) {
                    (Ok(held), Ok(id)) => store.guide_repair_snapshot(|store| {
                        Ok(prepare::prepare(
                            store, request, &held, epoch, id, created_at,
                        ))
                    })?,
                    (Err(error), _) | (_, Err(error)) => Err(error),
                };
                Ok(Ok(result))
            });
        async move {
            let result = prepared.await??;
            if !services.is_current() {
                return Err(InspectionError::OwnerClosed.into());
            }
            let mut state = owner.repairs.borrow_mut();
            state
                .plans
                .retain(|_, plan| plan.epoch == epoch && plan.preview.expires_at > now());
            while state.plans.len() >= MAX_PLANS
                || state
                    .plans
                    .values()
                    .map(|p| p.rows.len() + p.indirect.len())
                    .sum::<usize>()
                    + result.rows.len()
                    + result.indirect.len()
                    > 200_000
            {
                let old = state
                    .plans
                    .iter()
                    .filter(|(id, _)| !state.applying.contains(*id))
                    .min_by_key(|(id, plan)| (plan.preview.created_at, (*id).clone()))
                    .map(|(id, _)| id.clone());
                if let Some(old) = old {
                    state.plans.remove(&old);
                } else {
                    return Err(Error::Busy);
                }
            }
            let preview = result.preview.clone();
            state.plans.insert(preview.id.clone(), result);
            Ok(preview)
        }
    }

    pub fn guide_repair_plan(
        &self,
        id: &str,
        digest: [u8; 32],
        mapping_offset: usize,
        indirect_offset: usize,
        limit: usize,
    ) -> Result<Preview> {
        if limit == 0 || limit > MAX_PREVIEW_PAGE {
            return Err(Error::InvalidRequest);
        }
        let state = self.repairs.borrow();
        let plan = state.plans.get(id).ok_or(Error::PlanMissing)?;
        if plan.preview.digest != digest {
            return Err(Error::WrongDigest);
        }
        if plan.epoch != self.services().epoch() || plan.preview.expires_at <= now() {
            return Err(Error::PlanExpired);
        }
        if mapping_offset > plan.rows.len() || indirect_offset > plan.indirect.len() {
            return Err(Error::InvalidRequest);
        }
        let mut page = plan.preview.clone();
        page.mappings = plan
            .rows
            .iter()
            .skip(mapping_offset)
            .take(limit)
            .cloned()
            .collect();
        page.indirect = plan
            .indirect
            .iter()
            .skip(indirect_offset)
            .take(limit)
            .cloned()
            .collect();
        page.next_mapping = (mapping_offset + page.mappings.len() < plan.rows.len())
            .then_some(mapping_offset + page.mappings.len());
        page.next_indirect = (indirect_offset + page.indirect.len() < plan.indirect.len())
            .then_some(indirect_offset + page.indirect.len());
        Ok(page)
    }

    pub fn release_guide_repair_plan(&self, id: &str, digest: [u8; 32]) -> Result<bool> {
        let mut state = self.repairs.borrow_mut();
        if state.applying.contains(id) {
            return Err(Error::Busy);
        }
        if let Some(plan) = state.plans.get(id)
            && plan.preview.digest != digest
        {
            return Err(Error::WrongDigest);
        }
        Ok(state.plans.remove(id).is_some())
    }

    pub fn apply_guide_repair(
        self: &Rc<Self>,
        id: String,
        digest: [u8; 32],
        control: InspectionControl,
    ) -> impl Future<Output = Result<Outcome>> + use<> {
        let owner = self.clone();
        let services = self.services();
        let plan = self.repairs.borrow().plans.get(&id).cloned();
        let deadline = Instant::now() + control.timeout.min(PREVIEW_DEADLINE);
        let child = CancelToken::new();
        let guard = write::CancelOnDrop(child.clone());
        async move {
            let _guard = guard;
            let plan = match plan {
                Some(plan) => plan,
                None => {
                    let lookup = id.clone();
                    let hashed = digest_hex(&digest);
                    let remaining = InspectionControl {
                        cancel: control.cancel.clone(),
                        timeout: deadline.saturating_duration_since(Instant::now()),
                    };
                    let existing = crate::inspection::query_with_deadline(
                        &owner,
                        remaining,
                        PREVIEW_DEADLINE,
                        move |store| Ok(Ok(store.guide_repair_receipt(&lookup, &hashed)?)),
                    )
                    .await?;
                    return match existing {
                        RepairResult::Committed(saved) => Ok(receipt(&owner, commit(saved))),
                        RepairResult::Invalid => Err(Error::WrongDigest),
                        _ => Err(Error::PlanMissing),
                    };
                }
            };
            if plan.preview.digest != digest {
                return Err(Error::WrongDigest);
            }
            if plan.epoch != services.epoch() || plan.preview.expires_at <= now() {
                return Err(Error::PlanExpired);
            }
            if !services.is_current() {
                return Err(InspectionError::OwnerClosed.into());
            }
            if control.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            owner.repairs.borrow_mut().reserve(&id)?;
            let _applying = Applying {
                owner: Rc::downgrade(&owner),
                id: id.clone(),
            };
            let lookup = id.clone();
            let remaining = InspectionControl {
                cancel: control.cancel.clone(),
                timeout: deadline.saturating_duration_since(Instant::now()),
            };
            if let Some(existing) = crate::inspection::query_with_deadline(
                &owner,
                remaining,
                PREVIEW_DEADLINE,
                move |store| Ok(Ok(store.guide_repair_commit(&lookup)?)),
            )
            .await?
            {
                return Ok(receipt(&owner, commit(existing)));
            }
            let held = leases(&owner, &plan.request)?;
            let source = plan.request.source;
            let cancelled = crate::data::stored::supersede(&owner, source);
            let edit = crate::stored::guide_edit_revision(&owner, source);
            let id_for_write = id.clone();
            let digest = digest_hex(&digest);
            let token = child.clone();
            let phase = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
            let writer_phase = phase.clone();
            let worker_plan = plan.clone();
            let saved = services.call(move |store| {
                if token.is_cancelled() {
                    return Ok(Err(Error::Cancelled));
                }
                if Instant::now() >= deadline {
                    return Ok(Err(InspectionError::Deadline.into()));
                }
                if let Err(error) = validate_leases(&held, &worker_plan) {
                    return Ok(Err(error));
                }
                if let Err(error) = write::begin(&writer_phase, &token, deadline) {
                    return Ok(Err(error));
                }
                let saved = store.apply_guide_repair(&id_for_write, &digest, &worker_plan.batch)?;
                let result = committed(saved);
                writer_phase.store(
                    if result.is_ok() { 2 } else { 3 },
                    std::sync::atomic::Ordering::SeqCst,
                );
                Ok(result)
            });
            let saved = write::wait(
                &services,
                control,
                deadline,
                id.clone(),
                phase,
                child,
                saved,
            )
            .await;
            cancelled.keep_check(&owner, source, owner.browser_generation(), now());
            // Once committed, cancellation/reply loss cannot turn durable effects
            // into a reported rollback. A retry uses the same journal receipt.
            let saved = saved?;
            let outcome = publication::after_commit(&owner, saved, edit, cancelled.cancelled).await;
            Ok(outcome)
        }
    }

    pub fn undo_guide_repair(
        self: &Rc<Self>,
        id: String,
        control: InspectionControl,
    ) -> impl Future<Output = Result<Outcome>> + use<> {
        let owner = self.clone();
        let services = self.services();
        let deadline = Instant::now() + control.timeout.min(PREVIEW_DEADLINE);
        let child = CancelToken::new();
        let guard = write::CancelOnDrop(child.clone());
        async move {
            let _guard = guard;
            if control.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let read_id = id.clone();
            let remaining = InspectionControl {
                cancel: control.cancel.clone(),
                timeout: deadline.saturating_duration_since(Instant::now()),
            };
            let existing = crate::inspection::query_with_deadline(
                &owner,
                remaining,
                PREVIEW_DEADLINE,
                move |store| Ok(Ok(store.guide_repair_commit(&read_id)?)),
            )
            .await?
            .ok_or(Error::CommitMissing)?;
            owner.repairs.borrow_mut().reserve(&id)?;
            let _applying = Applying {
                owner: Rc::downgrade(&owner),
                id: id.clone(),
            };
            let source = existing.source;
            let cancelled = crate::data::stored::supersede(&owner, source);
            let edit = crate::stored::guide_edit_revision(&owner, source);
            let token = child.clone();
            let result_id = id.clone();
            let phase = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
            let writer_phase = phase.clone();
            let saved = services.call(move |store| {
                if token.is_cancelled() {
                    return Ok(Err(Error::Cancelled));
                }
                if let Err(error) = write::begin(&writer_phase, &token, deadline) {
                    return Ok(Err(error));
                }
                let result = match store.undo_guide_repair(&id)? {
                    RepairResult::Expired => Err(Error::UndoExpired),
                    result => committed(result),
                };
                writer_phase.store(
                    if result.is_ok() { 2 } else { 3 },
                    std::sync::atomic::Ordering::SeqCst,
                );
                Ok(result)
            });
            let saved =
                write::wait(&services, control, deadline, result_id, phase, child, saved).await;
            cancelled.keep_check(&owner, source, owner.browser_generation(), now());
            let saved = saved?;
            Ok(publication::after_commit(&owner, saved, edit, cancelled.cancelled).await)
        }
    }

    pub fn verify_guide_repair(
        self: &Rc<Self>,
        id: String,
        window: TimeWindow,
        control: InspectionControl,
    ) -> impl Future<Output = Result<Verification>> + use<> {
        let owner = self.clone();
        let services = self.services();
        let read_id = id.clone();
        let deadline = Instant::now() + control.timeout.min(PREVIEW_DEADLINE);
        let again = control.clone();
        let analyzed =
            crate::inspection::query_with_deadline(self, control, PREVIEW_DEADLINE, move |store| {
                if !window.valid() {
                    return Ok(Err(InspectionError::InvalidRequest));
                }
                Ok(Ok(store.verify_guide_repair(
                    &read_id,
                    window.from,
                    window.to,
                )?))
            });
        async move {
            let verified = analyzed.await?.ok_or(Error::CommitMissing)?;
            if !verified.authorized {
                return Err(InspectionError::PermissionDenied.into());
            }
            let source = verified.commit.source;
            let remaining = InspectionControl {
                cancel: again.cancel,
                timeout: deadline.saturating_duration_since(Instant::now()),
            };
            let current = crate::inspection::query_with_deadline(
                &owner,
                remaining,
                PREVIEW_DEADLINE,
                move |store| Ok(Ok(store.guide_revisions(source)?)),
            )
            .await?;
            let examined = Revisions {
                mapping: verified.examined_mapping,
                configuration: verified.examined_configuration,
            };
            let expected = Revisions {
                mapping: verified.commit.mapping_revision,
                configuration: verified.commit.configuration_revision,
            };
            let current = Revisions {
                mapping: current.mapping,
                configuration: current.configuration,
            };
            let same = examined == current && examined == expected && services.is_current();
            let result = Verification {
                commit: id.clone(),
                source,
                examined,
                expected,
                current,
                window,
                channels: verified.channels,
                programmes_available: verified.programmes_available,
                covered_seconds: verified.covered_seconds,
                identity_unresolved: true,
                current_revision: same,
                configuration_effective: verified.configuration_effective,
                verified_coverage: same && verified.configuration_effective,
            };
            if let Some(operation) = owner.repairs.borrow_mut().operations.get_mut(&id) {
                operation.phase = if result.verified_coverage {
                    OperationPhase::Verified
                } else if !same {
                    OperationPhase::Superseded
                } else if matches!(
                    operation.phase,
                    OperationPhase::Pending | OperationPhase::Refreshing
                ) {
                    operation.phase
                } else {
                    OperationPhase::ReadyToVerify
                };
                operation.verification = Some(result.clone());
            }
            Ok(result)
        }
    }

    pub fn recent_guide_repairs(
        &self,
        source: SourceId,
        limit: u32,
        control: InspectionControl,
    ) -> impl Future<Output = Result<Vec<Commit>>> + use<> {
        let read =
            crate::inspection::query_with_deadline(self, control, PREVIEW_DEADLINE, move |store| {
                if limit == 0 || limit > 50 {
                    return Ok(Err(InspectionError::InvalidRequest));
                }
                if store.source(source)?.is_none() {
                    return Ok(Err(InspectionError::SourceMissing(source)));
                }
                Ok(Ok(store
                    .recent_guide_repairs(source, limit)?
                    .into_iter()
                    .map(commit)
                    .collect()))
            });
        async move { read.await.map_err(Error::from) }
    }

    pub fn guide_repair_operation(
        self: &Rc<Self>,
        id: String,
    ) -> impl Future<Output = Result<Operation>> + use<> {
        let owner = self.clone();
        let cached = self.repairs.borrow().operations.get(&id).cloned();
        async move {
            if !owner.services().is_current() {
                return Err(InspectionError::OwnerClosed.into());
            }
            if let Some(operation) = cached {
                return Ok(operation);
            }
            let read = id.clone();
            let stored = crate::inspection::query_with_deadline(
                &owner,
                InspectionControl::default(),
                PREVIEW_DEADLINE,
                move |store| Ok(Ok(store.guide_repair_commit(&read)?)),
            )
            .await?
            .ok_or(Error::CommitMissing)?;
            Ok(Operation {
                id,
                commit: commit(stored),
                phase: OperationPhase::ReadyToVerify,
                effective: Effective::Pending,
                verification: None,
            })
        }
    }

    pub fn cancel_guide_repair_operation(self: &Rc<Self>, id: &str) -> Result<()> {
        let source = {
            let mut state = self.repairs.borrow_mut();
            let operation = state.operations.get_mut(id).ok_or(Error::CommitMissing)?;
            operation.phase = OperationPhase::Cancelled;
            let source = operation.commit.source;
            state
                .refreshes
                .get(&source)
                .filter(|(current, _)| current == id)
                .map(|_| source)
        };
        if let Some(source) = source {
            crate::data::stored::supersede(self, source);
        }
        Ok(())
    }
}
