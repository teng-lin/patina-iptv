# Rust review of iptv-runtime

This native application library owns service epochs, asynchronous work, inspection,
repair, persistence, recording and playback admission. Edition 2024 and Rust 1.99.1
apply. Its declared runtime v1 facade is a stable internal contract; other public
helpers remain experimental. The most urgent defect is a reproduced parental
access bypass through the inspection facade. This is a review only: proposed
patches below have not been applied.

## Must fix

### iptv-runtime-001 Inspection queries bypass authoritative parental policy

**Location:** `crates/iptv-runtime/src/inspection.rs:150`, `:157`, `:174`,
`:192`, `:209`; related store adapters `inspection.rs:295`, `:349`, `:397`.

**Rule:** Runtime v1 access contract in `docs/runtime-api-v1.md`; enforce
invariants at public API boundaries, rust-best-practices sections 1 and 4.

**Problem:** The facade directly delegates to storage inspection operations.
Those operations enforce the legacy channel overlay block, but do not consult
runtime parental grants, adult membership or the separate parental block tables.
A native consumer can therefore retrieve a protected channel by known ID or
through a query while the ordinary native playback command rejects it.

**Evidence:** A temporary executable used the actual public Runtime API, an
encrypted synthetic catalogue with one adult channel, an installed PIN and
`parental.live_enabled=true`. After locking, it printed:

```text
parental_check_denies=true
inspect_channel_returns_blocked_content=true
query_channels_returned=1
```

The exact probe is outside the repository at
`/tmp/patina-rust-review-20261116/runtime-probe/src/main.rs`; its result is in
`/tmp/patina-rust-review-20261116/runtime-probe.log`.

**Proposed fix:** Capture the owner's parental snapshot when accepting each
inspection operation, apply current policy/membership on the analysis lane,
and fence publication against revocation. A direct-ID operation can follow the
existing `commands::channel_native` admission pattern:

```rust
// Proposed admission inside the guarded store operation.
access.access.policy = crate::parental::policy_settings(&services.settings());
let target = iptv_core::parental::Target::live(key.source.0, key.content.as_str());
if !crate::parental::native_allowed(store, &target, &access)? {
    return Ok(Err(InspectionError::PermissionDenied));
}
```

For pages, enforce authorization before pagination/cursor generation rather than
filtering an already limited page. Cover programme queries, candidates and audit
results, and examine repair admission under the same contract. Preserve bounded
query execution and typed access outcomes.

**Regression:** Extend the existing `tests/it/parental_access.rs` and foundation
inspection scenarios with direct, source/category/custom-group/adult blocks,
known-ID and paginated reads, unlocked results, and revocation while queued.

**Disposition:** Open; executable reproduction and independent source review.

### iptv-runtime-002 Recording mutations can outlive their parental authorization

**Location:** `crates/iptv-runtime/src/recordings.rs:524`, `:588`, `:668`;
`crates/iptv-runtime/src/recordings/rules.rs:40`, `:119`, `:142`.

**Rule:** Native admission and revocation contract in `docs/architecture.md` and
`docs/runtime-api-v1.md`; rust-best-practices sections 1 and 7.

**Problem:** Scheduling, updating and enabling recurring recordings check parental
access before subsequent awaits, then enqueue mutations without a captured
parental Snapshot or execution-time `native_allowed` gate. A caller can authorize
protected content, begin scheduling, relock while the writer is occupied, and
still create durable recording authority. The store checks legacy overlay blocks
and source enabled state, which do not replace runtime parental policy. Existing
accepted jobs intentionally survive later UI locks; this finding concerns the
boundary before new or changed intent is durably accepted.

**Proposed fix:** Capture parental authority and original source incarnation at
acceptance; validate current policy, membership and authority in the same writer
transaction that accepts the intent. Fail stale/revoked requests without a job
or recurring rule. Reuse the import publication pattern rather than tying the
background recorder to a transient UI unlock after acceptance.

```rust
// Proposed writer admission before an atomic recording mutation.
access.access.policy = crate::parental::policy_settings(&accepted.settings());
if !crate::parental::native_allowed(store, &target, &access)? {
    return Err(iptv_store_sqlite::StoreError::Unavailable(
        "Unlock this recording with the parental PIN",
    ));
}
store.schedule_recording(id, &request, now())
```

Source/incarnation validation must be part of the same transaction; the excerpt
shows only the missing access gate. Apply the rule consistently to one-shot,
update, recurring-first-job and re-enable paths.

**Regression:** Hold writer execution after initial authorization, revoke or
expire the grant, then release it. Assert no new/changed job or enabled recurring
rule is committed. Also assert that already accepted jobs continue after locking.
Existing tests starting with locked content do not cover this interleaving.

**Disposition:** Open; independently confirmed source path. A supplemental local
probe reached a successful update reply after a counted parental-read completion,
relock and writer-barrier release. Its final assertion incorrectly expected the
store to change a title field that `update_recording` intentionally does not
update, so that probe is inconclusive as a complete durable-state regression;
it is not counted as a passing reproduction. Preserve this distinction when
implementing the proposed deterministic test. Source/output are outside the repo
at `/tmp/patina-rust-review-20261116/runtime-probe/src/bin/recording_admission.rs`
and `runtime-recording-admission-probe.log`.

### iptv-runtime-003 Older guide pages can be published into a replacement schedule

**Location:** `crates/iptv-runtime/src/data/pages.rs:55`, `:78`, `:98`.

**Rule:** AGENTS.md state/content identity invariant and runtime v1 stale-work
contract; rust-best-practices section 7 on ownership of asynchronous work.

**Problem:** The page loader captures only SourceId and PageRequest. When its
read finishes, `arrived` finds the owner's current schedule for that source and
inserts into it. A refresh can replace the schedule within the same loaded time
window while both versions request the same old day. The older read then
satisfies the new schedule's pending page and publishes stale programme data.
The portable request has no issuing schedule identity (`iptv-guide-002`).

**Proposed fix:** Bind every request/reply to the issuing schedule generation
and source incarnation, or insert only into the exact retained page owner. Fence
current UI notifications separately. A new request identity must be checked by
`Pages::insert`, not reconstructed from a source ID alone.

```rust
// Proposed reply admission, with a captured identity added to the loader.
if !reply.belongs_to(current_schedule_identity) {
    return;
}
```

`belongs_to` represents the proposed identity check, not an existing API.

**Regression:** Hold a page read; refresh/rebind the same source within the same
hour; request the same day on the new schedule; deliver the old reply first.
Assert it neither satisfies the new request nor overwrites the new programmes.
Repeat for a combined schedule and a deleted/recreated source.

**Disposition:** Open; runtime source path confirmed and portable collision
reproduced by the portable reviewer. One cross-crate fix with `iptv-guide-002`.

### iptv-runtime-004 Failed page reads become permanently cached empty guide data

**Location:** `crates/iptv-runtime/src/data/pages.rs:67` through `:72`, `:98`.

**Rule:** Runtime error contract distinguishes unreadable storage from a successful
empty result; rust-review Error handling and rust-best-practices section 5.

**Problem:** A failed `guide_page` read is replaced with `GuideStore::default()`
and inserted as a successful loaded page. Subsequent `ensure_loaded` calls do
not retry it. A temporary database failure therefore leaves that guide day empty
until eviction or schedule replacement, and catch-up can proceed using missing
programme evidence.

**Proposed fix:** Carry the error through page completion, clear only that
request's pending state, and keep the page retryable. Notify waiters of failure
without pretending that an empty page was read. Add a failure completion method
to the portable page owner if necessary.

```rust
match read.await {
    Ok(store) => complete_success(request_identity, store),
    Err(error) => complete_failure(request_identity, error),
}
```

The completion functions describe proposed methods, not an applied patch.

**Regression:** Fail an old-day read once and then let it succeed. Assert the
failure installs no empty page, a later user lookup retries, and real programme
bounds become available without rebuilding the entire schedule. Retain successful
empty-page caching as a separate case.

**Disposition:** Open; source-confirmed.

### iptv-runtime-006 Direct runtime playback results lack the final grant check

**Location:** `crates/iptv-runtime/src/vod.rs:735` through `:755`;
`crates/iptv-runtime/src/recordings.rs:758`, `:823` through `:848`.

**Rule:** Native admission/revocation invariant; rust-best-practices sections 1
and 7. The adjacent `vod_details_for` final publication gate at `vod.rs:560`
already documents the required check after the last await.

**Problem:** `vod_play` checks parental access, awaits a final source read, then
returns the resolved source without checking grants again. `recording_file` and
`open_recording` similarly await file lookup/relay construction after their check.
An unlock revoked during those awaits can therefore survive the direct runtime
API result. The app's `player::publish_source` does perform a synchronous final
`source_allowed` check; this finding does not claim the current shell bypasses
that check. These helpers are experimental, but independent native consumers
cannot rely on the shell gate.

**Proposed fix:** Retain the relevant content targets and captured authorization
through the operation. After its last await, synchronously validate that the owner
and grants remain current, including an episode's parent. Close a newly created
recording relay if admission fails.

```rust
if !current
    || !self.parental_allowed(&target(&playing.key))
    || playing.parent.as_ref().is_some_and(|parent| {
        !self.parental_allowed(&target(parent))
    })
{
    return Err(unavailable());
}
```

Retain membership/source snapshot checks; a cached grant check is only the
additional final revocation fence, not a replacement for durable membership.

**Regression:** Direct Runtime callers should pause the final source/file read,
relock, release it, and receive an error with no retained media capability.
Keep the existing app publication gate and test it independently.

**Disposition:** Open; independently confirmed by source; shell mitigation
verified by the app reviewer; deterministic race test remains proposed.

## Should fix

### iptv-runtime-005 Probe configuration is assembled without one database snapshot

**Location:** `crates/iptv-runtime/src/inspection/probe.rs:591` through `:638`,
especially the second source lookup at `:632`.

**Rule:** Runtime v1 source-incarnation contract; rust-review reachable panics;
rust-best-practices sections 1 and 5 on validating external state.

**Problem:** Configuration first verifies that a source exists, then separately
reads policy, source incarnation, feed configuration and the source again with
`expect("source checked")`. These are separate SQLite read snapshots on a lane
that permits concurrent writes. Deleting the source between checks makes the
expect panic; edits can also assemble a mixed configuration before later stale
checks reject it. The worker catches panics, so this is a failed request and
lost connection rather than a demonstrated process crash.

**Proposed fix:** Capture all configuration fields in one read transaction and
return `SourceMissing` for absence. Reuse the store's existing snapshot mechanism
without introducing an adapter type into the public facade.

```rust
// Proposed structure around the existing configuration read.
store.guide_repair_snapshot(|store| {
    let Some(source) = store.source(request.source)? else {
        return Ok(Err(InspectionError::SourceMissing(request.source)));
    };
    // Read policy, incarnation and selected feed within this same snapshot.
    // Reuse source rather than checking it again with expect.
    build_configuration_in_snapshot(store, source)
})
```

The final helper is a placeholder for the existing configuration construction.
Audit other compound reader-lane snapshots (`stored::guide_inputs`, library and
organization snapshots) for the same consistency requirement.

**Regression:** A barrier-controlled concurrent source deletion/configuration
edit must yield a coherent old/new snapshot or typed stale/missing outcome,
without a panic or mixed credentials/incarnations.

**Disposition:** Open; source-confirmed interleaving, not reproduced dynamically.

### iptv-runtime-007 Probe validation replaces the operation cancellation budget

**Location:** `crates/iptv-runtime/src/inspection/probe.rs:511` through `:517`,
`:1092`.

**Rule:** Runtime v1 end-to-end cancellation/budget contract; rust-best-practices
section 7 requires bounded cooperative work and prompt resource release.

**Problem:** `probe_schedule` checks its child cancellation token and deadline,
then invokes `validate_candidate`, which creates a new independent token and
60-second deadline for opening and scanning the encrypted snapshot. A shorter
caller deadline or cancellation can return publicly while this scan retains
its blocking-worker permit and snapshot without observing the operation's token.
The later parser loop resumes correct checks, but the integrity scan does not.

**Proposed fix:** Forward the existing child token and original deadline.

```rust
lease.validate_candidates_with_control(
    std::slice::from_ref(&candidate),
    &worker_control.cancel,
    deadline,
)?;
```

Also bound/cancel `current_configuration`'s queued analysis read using those
same controls instead of an unbounded `block_on` before its first worker check.

**Regression:** Pause a large synthetic snapshot integrity scan, expire/cancel
its short operation budget, and verify the real worker permit is released and
teardown does not wait for a fresh 60-second validation budget.

**Disposition:** Open; independently confirmed source path, no timing claim.

## Consider

Keep explicit local ownership, weak callbacks and the separate display and
analysis lanes. Bounded fallback draining in Drop is a documented persistence
tradeoff; do not replace it with a blanket nonblocking Drop rule during this
review. Native query authorization and playback publication deserve focused
regressions before API/style cleanup.

## What's already good

- Accepted store jobs retain their owner and epoch; no ambient Runtime registry
  can redirect a late operation to another library.
- Import publication rechecks cancellation, original source incarnation/config
  and administrative authority before committing staged data.
- Repair operations distinguish durable commitment from later cache publication
  and retain operation identities when completion is uncertain.
- Display reads and expensive analysis use separate lanes, with existing
  responsiveness and owner-retirement scenarios.

## Baseline and validation

- Baseline HEAD: `0e197073a272eceb0631e4d53b08d2823950c416`.
- Linux x86_64; cargo/rustc 1.99.1, declared edition 2024; native application
  library with a stable internal facade. No source changes or commits.
- Workspace Clippy with all targets and runtime/app test-support passed;
  workspace format check passed. The non-player workspace ordinary tests passed
  with runtime test-support, including this crate.
- Full host wrapper stopped at NSH revision drift: actual
  `32769658ad34ad364b960a134cc73c754d50f2d9`, pinned
  `9ed782cbf362ab63ba18c47dc657fe944c405e50`. The required core wasm build fails
  because the target standard library is absent. Neither condition was changed.
- Parent-coordinated native player tests have separate failures; they do not
  establish a defect in this crate. Full commands and outcomes will be retained
  in the central review index.
- Android/Apple/Windows device behavior, fuzzing, isolated timing gates and Miri
  have not been validated by this source review and remain fix-stage checks.

## File coverage

All production runtime modules were inspected for ownership, I/O, state and
admission paths. Inline and separate test modules were inspected around relevant
contracts; integration files were inventoried and relevant assertions traced.
The coverage inventory does not claim exhaustive auditing of every test body.
Native cfg branches received source inspection, not device execution.

- [x] `Cargo.toml`: Manifest and features reviewed.
- [x] `src/assign.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/backup/error.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/backup/instance.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/backup/tests.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `src/backup.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/commands.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/credential_store.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/data/assign.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/data/combined.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/data/library.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/data/opening.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/data/organization.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/data/pages.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/data/stored.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/data.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/error.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/events.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/executor.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/favorites.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/file_writer.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/guide_checks/refresh_tickets.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/guide_checks/retry.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/guide_checks.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/guide_repair/prepare.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/guide_repair/publication.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/guide_repair/tests.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `src/guide_repair/write.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/guide_repair.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/guides.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/imports/recovery_tests.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `src/imports.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/inspection/probe.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/inspection.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/lib.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/library/guide_work_supersession.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/library.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/model.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/opening/revocation.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/opening.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/operations.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/paging.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/parental.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/playback.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/ports.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/private_files.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/recordings/host.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/recordings/recovery_tests.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `src/recordings/rules.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/recordings/shared.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/recordings/shared_tests.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `src/recordings/tests.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `src/recordings.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/search.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/settings.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/state.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/storage.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/store_service.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/stored.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/testing.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/vod/recovery_tests.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `src/vod.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/wizard_map.rs`: Production source inspected; relevant inline tests traced.
- [x] `src/workers.rs`: Production source inspected; relevant inline tests traced.
- [x] `tests/it/architecture.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/capability_reports.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/command_journeys.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/command_values.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/data_startup.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/foundation_contract.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/foundation_inspection.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/foundation_probe.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/foundation_repair_fences.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/guide_check_owners.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/guide_journeys.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/host_lifecycle.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/library_journeys.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/lifecycle_recovery.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/main.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/native_backup.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/ownership.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/parental_access.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/parental_recovery.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/persistence_safety.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/read_lane_contention.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/recording_capability_restart.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/recording_capability_startup.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/relative_organization_moves.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/shell_boundary.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/vod_import_pipeline.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/it/vod_library.rs`: Test scenario inventory and relevant assertions inspected.
- [x] `tests/support/mod.rs`: Test scenario inventory and relevant assertions inspected.

No build script or example targets are declared in this crate.

## Resolution tracking

| Finding | Disposition | Implementation | Required evidence |
|---|---|---|---|
| iptv-runtime-001 | Open | No code changed | Locked and queued-revocation inspection regressions |
| iptv-runtime-002 | Open | No code changed | Writer-barrier revocation before durable recording admission |
| iptv-runtime-003 | Open | No code changed | Delayed page replies across schedule/source replacement |
| iptv-runtime-004 | Open | No code changed | Fail-once read followed by successful user retry |
| iptv-runtime-005 | Open | No code changed | Coherent snapshot under source deletion/edit |
| iptv-runtime-006 | Open | No code changed | Direct runtime late-revocation playback tests |
| iptv-runtime-007 | Open | No code changed | Integrity scan honors original cancellation/deadline |
