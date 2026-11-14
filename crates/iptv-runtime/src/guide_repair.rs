//! Application-owned immutable repair previews, atomic commit and observed status.
mod prepare;
mod publication;
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
    collections::HashMap,
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
    operations: HashMap<String, Operation>,
    refreshes: HashMap<SourceId, (String, CancelToken)>,
}
impl State {
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
        self.operations.clear();
        for (_, (_, cancel)) in self.refreshes.drain() {
            cancel.cancel();
        }
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
    pub fn preview_guide_repair(
        self: &Rc<Self>,
        request: Request,
        control: InspectionControl,
    ) -> impl Future<Output = Result<Preview>> + use<> {
        let owner = self.clone();
        let held = leases(self, &request);
        let services = self.services();
        let epoch = services.epoch();
        let deadline = Instant::now() + control.timeout.min(PREVIEW_DEADLINE);
        let id = random_id();
        let created_at = now();
        async move {
            if !services.is_current() {
                return Err(InspectionError::OwnerClosed.into());
            }
            if control.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let held = held?;
            let id = id?;
            let cancel = control.cancel.clone();
            let result = services
                .analyze(move |store| {
                    store.inspection_read(cancel, deadline, move |store| {
                        let result = store.guide_repair_snapshot(|store| {
                            Ok(prepare::prepare(
                                store, request, &held, epoch, id, created_at,
                            ))
                        })?;
                        Ok(Ok(result))
                    })
                })
                .await
                .map_err(|_| Error::Storage)???;
            if !services.is_current() {
                return Err(InspectionError::OwnerClosed.into());
            }
            let mut state = owner.repairs.borrow_mut();
            state
                .plans
                .retain(|_, plan| plan.epoch == epoch && plan.preview.expires_at > now());
            if state.plans.len() >= MAX_PLANS {
                return Err(Error::Busy);
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

    pub fn apply_guide_repair(
        self: &Rc<Self>,
        id: String,
        digest: [u8; 32],
        control: InspectionControl,
    ) -> impl Future<Output = Result<Outcome>> + use<> {
        let owner = self.clone();
        let services = self.services();
        let plan = self.repairs.borrow().plans.get(&id).cloned();
        async move {
            let plan = plan.ok_or(Error::PlanMissing)?;
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
            let held = leases(&owner, &plan.request)?;
            let source = plan.request.source;
            let cancelled = crate::data::stored::supersede(&owner, source);
            let edit = crate::stored::guide_edit_revision(&owner, source);
            let id_for_write = id.clone();
            let digest = digest_hex(&digest);
            let token = control.cancel.clone();
            let worker_plan = plan.clone();
            let saved = services
                .call(move |store| {
                    if token.is_cancelled() {
                        return Ok(Err(Error::Cancelled));
                    }
                    if let Err(error) = validate_leases(&held, &worker_plan) {
                        return Ok(Err(error));
                    }
                    let saved =
                        store.apply_guide_repair(&id_for_write, &digest, &worker_plan.batch)?;
                    Ok(committed(saved))
                })
                .await;
            cancelled.keep_check(&owner, source, owner.browser_generation(), now());
            // Once committed, cancellation/reply loss cannot turn durable effects
            // into a reported rollback. A retry uses the same journal receipt.
            let saved = saved.map_err(|_| Error::Storage)??;
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
        async move {
            if control.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let read_id = id.clone();
            let existing = services
                .read(move |store| store.guide_repair_commit(&read_id))
                .await
                .map_err(|_| Error::Storage)?
                .ok_or(Error::CommitMissing)?;
            let source = existing.source;
            let cancelled = crate::data::stored::supersede(&owner, source);
            let edit = crate::stored::guide_edit_revision(&owner, source);
            let token = control.cancel.clone();
            let saved = services
                .call(move |store| {
                    if token.is_cancelled() {
                        return Ok(Err(Error::Cancelled));
                    }
                    Ok(match store.undo_guide_repair(&id)? {
                        RepairResult::Expired => Err(Error::UndoExpired),
                        result => committed(result),
                    })
                })
                .await;
            cancelled.keep_check(&owner, source, owner.browser_generation(), now());
            let saved = saved.map_err(|_| Error::Storage)??;
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
        async move {
            if !window.valid() {
                return Err(Error::InvalidRequest);
            }
            let read_id = id.clone();
            let cancel = control.cancel.clone();
            let deadline = Instant::now() + control.timeout.min(PREVIEW_DEADLINE);
            let verified = services
                .analyze(move |store| {
                    store.inspection_read(cancel, deadline, move |store| {
                        Ok(Ok(store.verify_guide_repair(
                            &read_id,
                            window.from,
                            window.to,
                        )?))
                    })
                })
                .await
                .map_err(|_| Error::Storage)??
                .ok_or(Error::CommitMissing)?;
            if !verified.authorized {
                return Err(InspectionError::PermissionDenied.into());
            }
            let source = verified.commit.source;
            let current = services
                .read(move |store| store.guide_revisions(source))
                .await
                .map_err(|_| Error::Storage)?;
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
                } else {
                    OperationPhase::ReadyToVerify
                };
                operation.verification = Some(result.clone());
            }
            Ok(result)
        }
    }

    pub fn guide_repair_operation(
        self: &Rc<Self>,
        id: String,
    ) -> impl Future<Output = Result<Operation>> + use<> {
        let cached = self.repairs.borrow().operations.get(&id).cloned();
        let services = self.services();
        async move {
            if let Some(operation) = cached {
                return Ok(operation);
            }
            let read = id.clone();
            let stored = services
                .read(move |store| store.guide_repair_commit(&read))
                .await
                .map_err(|_| Error::Storage)?
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
