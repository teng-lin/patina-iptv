//! This consumer names only the declared native service facade and portable
//! values. Its exact constructions and exhaustive matches deliberately require
//! a compatibility decision when public shapes or outcome variants change.
use std::{future::Future, rc::Rc, time::Duration};

use iptv_import::CancelToken;
use iptv_library::guide_repair::{
    Capability, CapabilityState, Commit, Effective, Error as RepairError, Operation,
    OperationPhase, Outcome, Preview, Request, Target, Verification,
};
use iptv_library::inspection::{
    Candidate, CandidateProgram, CandidateRef, ChannelInfo, ChannelRef, Coverage, Cursor,
    InspectionError, Page, ProbeEvidence, ProbeSummary, ProgramInfo, Query, SourceScope,
    TimeWindow,
};
use iptv_library::{Choice, SourceId};
use iptv_runtime::{
    Runtime,
    inspection::{InspectionControl, ProbeRequest, ProbeTarget},
};

fn inspection<T, F: Future<Output = Result<T, InspectionError>>>(_: F) {}
fn repair<T, F: Future<Output = Result<T, RepairError>>>(_: F) {}

/// Compiled, never executed: signature checks cannot spawn workers or mutate
/// storage merely to prove that an independently written consumer type-checks.
fn consume(
    owner: &Rc<Runtime>,
    source: SourceId,
    channel: ChannelRef,
    candidate: CandidateRef,
    evidence: ProbeEvidence,
) {
    let control = InspectionControl {
        cancel: CancelToken::new(),
        timeout: Duration::from_secs(1),
    };
    let window = TimeWindow {
        from: 1_794_657_600,
        to: 1_794_664_800,
    };
    let cursor = Cursor {
        context: "synthetic-context".into(),
        snapshot: "synthetic-snapshot".into(),
        after: vec!["synthetic-content".into()],
    };
    let query = Query {
        scope: SourceScope::Source(source),
        text: "Synthetic".into(),
        include_hidden: false,
        limit: 10,
        cursor: Some(cursor.clone()),
    };
    // Construct a public result as well as reading it: adding a required field
    // affects consumers even if callers of the query method would still compile.
    let record = ChannelInfo {
        key: channel.clone(),
        name: "Synthetic Amber".into(),
        number: Some(1),
        hidden: false,
        blocked: false,
        epg_id: Some("amber.main".into()),
        epg_name: None,
        choice: Choice::Automatic,
    };
    let _: Page<ChannelInfo> = Page {
        items: vec![record],
        next: None,
    };
    inspection::<ChannelInfo, _>(owner.inspect_channel(channel.clone(), control.clone()));
    inspection::<Page<ChannelInfo>, _>(owner.query_channels(query.clone(), control.clone()));
    inspection::<Page<ProgramInfo>, _>(owner.query_programs(
        query.clone(),
        window,
        Some(channel.clone()),
        control.clone(),
    ));
    inspection::<Page<Coverage>, _>(owner.epg_audit(query.clone(), window, control.clone()));
    inspection::<Page<Candidate>, _>(owner.epg_candidates(channel.clone(), query, control.clone()));
    inspection::<ProbeSummary, _>(owner.probe_feed(
        ProbeRequest {
            source,
            target: ProbeTarget::Configured(candidate.feed),
        },
        control.clone(),
    ));
    inspection::<Page<Candidate>, _>(owner.probe_candidates(
        evidence.clone(),
        "Synthetic".into(),
        Some(cursor),
        10,
        control.clone(),
    ));
    inspection::<Vec<CandidateProgram>, _>(owner.probe_schedule(
        evidence.clone(),
        candidate.clone(),
        window,
        control.clone(),
    ));
    let target = Target::Staged {
        evidence: evidence.clone(),
        epg_channel: "amber.main".into(),
        addition: Some(0),
    };
    let request = Request {
        source,
        mappings: Vec::new(),
        feeds: Vec::new(),
        additions: Vec::new(),
        feed_order: None,
        window,
    };
    let _ = Request::mapping(channel, target, window);
    repair::<Preview, _>(owner.preview_guide_repair(request, control.clone()));
    let _: Result<Preview, RepairError> =
        owner.guide_repair_plan("synthetic-plan", [0; 32], 0, 0, 10);
    repair::<Outcome, _>(owner.apply_guide_repair(
        "synthetic-plan".into(),
        [0; 32],
        control.clone(),
    ));
    repair::<Outcome, _>(owner.undo_guide_repair("synthetic-commit".into(), control.clone()));
    repair::<Verification, _>(owner.verify_guide_repair(
        "synthetic-commit".into(),
        window,
        control,
    ));
    repair::<Operation, _>(owner.guide_repair_operation("synthetic-commit".into()));
    let _: Result<(), RepairError> = owner.cancel_guide_repair_operation("synthetic-commit");
    let _: Result<iptv_runtime::inspection::ProbeLease, InspectionError> =
        owner.lease_probe(&evidence);
    let _: Option<Commit> = None;
    let _: Vec<Capability> = owner.foundation_capabilities();
    let _: Result<bool, RepairError> = owner.release_guide_repair_plan("synthetic-plan", [0; 32]);
    repair::<Vec<Commit>, _>(owner.recent_guide_repairs(source, 10, InspectionControl::default()));
}

fn inspection_error(error: InspectionError) -> &'static str {
    match error {
        InspectionError::InvalidRequest => "invalid request",
        InspectionError::SourceMissing(_) => "missing source",
        InspectionError::SourceDisabled(_) => "disabled source",
        InspectionError::ChannelMissing => "missing channel",
        InspectionError::PermissionDenied => "access denied",
        InspectionError::StaleCursor => "stale continuation",
        InspectionError::StaleEvidence => "stale evidence",
        InspectionError::Canceled => "cancelled",
        InspectionError::Deadline => "deadline",
        InspectionError::Busy => "busy",
        InspectionError::Storage => "storage failure",
        InspectionError::Network => "network failure",
        InspectionError::AddressPolicy => "address policy",
        InspectionError::InvalidFeed => "invalid feed",
        InspectionError::LimitExceeded => "budget exceeded",
        InspectionError::OwnerClosed => "owner retired",
    }
}
fn repair_error(error: RepairError) -> &'static str {
    match error {
        RepairError::Inspection(error) => inspection_error(error),
        RepairError::InvalidRequest => "invalid request",
        RepairError::PlanMissing => "missing plan",
        RepairError::PlanExpired => "expired plan",
        RepairError::WrongDigest => "wrong digest",
        RepairError::CommitMissing => "missing commit",
        RepairError::CompletionUnknown { operation: _ } => "reconcile operation",
        RepairError::UndoExpired => "expired inverse",
        RepairError::Conflict(_) => "conflict",
        RepairError::Storage => "storage failure",
        RepairError::Busy => "busy",
        RepairError::Canceled => "cancelled",
    }
}
fn operation_phase(phase: OperationPhase) -> &'static str {
    match phase {
        OperationPhase::Pending => "pending",
        OperationPhase::Refreshing => "refreshing",
        OperationPhase::ReadyToVerify => "ready to verify",
        OperationPhase::Verified => "verified",
        OperationPhase::Failed => "failed",
        OperationPhase::Superseded => "superseded",
        OperationPhase::Canceled => "cancelled",
    }
}
fn effective(value: Effective) -> &'static str {
    match value {
        Effective::Pending => "not installed",
        Effective::Installed => "installed",
        Effective::Failed => "installation failed",
    }
}
fn capability(value: CapabilityState) -> &'static str {
    match value {
        CapabilityState::Implemented => "implemented",
        CapabilityState::UnsupportedHost => "unsupported host",
        CapabilityState::ProviderUnverified => "provider unverified",
        CapabilityState::NotImplemented => "not implemented",
    }
}

#[test]
fn independent_consumer_compiles_against_the_declared_service_types() {
    let _: fn(&Rc<Runtime>, SourceId, ChannelRef, CandidateRef, ProbeEvidence) = consume;
    let _: fn(InspectionError) -> &'static str = inspection_error;
    let _: fn(RepairError) -> &'static str = repair_error;
    let _: fn(OperationPhase) -> &'static str = operation_phase;
    let _: fn(Effective) -> &'static str = effective;
    let _: fn(CapabilityState) -> &'static str = capability;
}
