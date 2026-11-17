# Audit follow-up implementation plan after PR #98

Status: implementation authorized by the user after planning, in one follow-up PR.
The historical inspection below was performed November 17, 03:17–03:23 UTC; it
is retained as the planning baseline, not a claim about the current PR state.
PR #98 subsequently merged as `323c18ebac4718e81fb2c679669c081ccf1cf06b`.
The isolated `fix/audit-follow-up` worktree has been rebased onto that merge.
See [implementation and validation record](2026-11-16-audit-follow-up-implementation.md)
for current dispositions and evidence. The user's later authorization includes
commits, opening the PR, fixing CI/review/conflicts, and merging when ready.

## Baseline and review scope

- Audit baseline: `b289c58717fa16679a5cba53bade53300a358b0c`.
- [PR #98](https://github.com/teng-lin/patina-iptv/pull/98), reviewed head:
  `45efb4bf5e46b8dee46e5a563b63997184791b41`, base `master` at the audit baseline.
  Open, mergeable, not merged when inspected; 11 commits and 179 changed files.
- The clean implementation worktree matches that head. Inspected the changed-file
  inventory, implementation tracker, native lifecycle experiment, validation and
  Android evidence, and targeted source/diffs for overlapping and residual findings.
  This is a focused review for follow-up planning, not an independent exhaustive
  approval of every changed line.
- Source of audit IDs: [consolidated audit](../audit/2026-11-16-consolidated.md).
  F/C IDs below are its identifiers; original Claude IDs are explicitly prefixed.
- PR implementation/evidence: [tracker at reviewed head](https://github.com/teng-lin/patina-iptv/blob/45efb4bf5e46b8dee46e5a563b63997184791b41/docs/plans/2026-11-16-rust-review-implementation.md),
  [local validation](https://github.com/teng-lin/patina-iptv/blob/45efb4bf5e46b8dee46e5a563b63997184791b41/docs/reviews/rust/implementation/final-validation.md),
  [Android record](https://github.com/teng-lin/patina-iptv/blob/45efb4bf5e46b8dee46e5a563b63997184791b41/docs/reviews/rust/implementation/android-validation.md).

PR reports full local validation: 1,949 ordinary tests, four isolated timing gates,
nine wasm builds, 16 doctests, guarded GTK/PiP and production app build. Later
changes are reported as test synchronization/platform conditions and evidence.
These are **PR-author evidence**, not test runs repeated during this review.

Direct checks here: source/diff inspection, read-only PR/review/workflow metadata,
and invocation of the PR head's CI selector. The selector still omits host/full
helper checks for test-consumed docs, and host execution for changed lint wrappers.
No production code, checkout, device profile or hosted setting was changed.

## What #98 already owns

| Audit item | Reviewed implementation | Follow-up disposition |
|---|---|---|
| F1 cancellation waiters | Stable waiter slots, drop cleanup and >32/churn/repoll/race tests in `iptv-import/src/cancel.rs` | Do not reimplement; retain regressions and verify accepted final head. |
| F2 programme Details | Shared real-description lookup, invalidation and distinct-source/empty-data tests in `header.rs`; overlay uses lookup | Main defect addressed. Empty fallback remains blank; localized missing-description copy is optional polish in P8. |
| C1 refused loads | Load identity, distinct pre-load rejection/post-loading guard failure and stale-reply regressions | Do not duplicate. Preserve guard restrictions and phase-specific accounting. |
| C2 worker notification | Nonblocking native-event drain when notes are queued; quiet-response notification regression | Do not duplicate. |
| F3 native teardown | Bounded child experiment establishes UI waiting and retained ownership; production teardown order intentionally unchanged | Investigation progressed, risk remains. P14 is evidence-led, not an automatic reaper rewrite. |
| Search/guide identities, admission, parental fences, redaction, private media handles | Broad additional corrections in the 39-finding PR tracker | Treat as #98-owned invariants, not new audit work. Re-test them around follow-up changes. |
| F5 platform evidence | Debug three-ABI APK, both API36 emulator forms, physical 32-bit TV and Stop checks reported | Partial progress. Physical tablet, speech and shipping-release-artifact acceptance remain distinct gaps. |

The PR does **not** close all similarly named concerns. Recording admission is not
terminal-state persistence; safe media handles are not nonblocking writes; search
identity is not picture-setting identity; Stop handling is not foreground
reauthorization. Source still shows these residual paths at the reviewed head:

| Residual | Evidence at PR head |
|---|---|
| C3 lost recording transition result | `crates/iptv-runtime/src/recordings.rs:1041` discards store result; terminal call at `:1626` |
| C4 blocking capture writes | `crates/iptv-net/src/recording.rs:237` calls `File::write_all` in async sink |
| C5 picture override identity | `crates/iptv-core/src/player_options.rs:62` retains `BTreeMap<usize, PictureSize>` |
| C6 resume-as-error | Android `PlayerComponent.kt:576` still emits synthetic errors to obtain reauthorization |
| C8 selector omissions | `tools/ci/validation-plan.py:53` treats all docs as non-code; direct selection reproduced omissions |
| Recording quota | `crates/iptv-runtime/src/recordings.rs:1410` adds active full-size reservations to existing media bytes |

## P0 — Accept #98 and establish the implementation base

**Owner:** existing #98 owner. **Dependency:** before overlapping follow-up edits.

1. Record final accepted SHA and its CI results. At first inspection,
   [Validation 44008945948](https://github.com/teng-lin/patina-iptv/actions/runs/44008945948)
   was in progress. Do not promote earlier-head results into final-head evidence.
2. Verify the explicit Windows full run executes the reparse-point regression;
   native-tier compilation alone is insufficient. Resolve actual code-review
   findings. The only submitted GitHub review observed was Copilot's quota-failure
   comment; it is not a substantive review. No inline threads were present.
3. Keep physical tablet and spoken accessibility limitations visible. #98's record
   says the physical tablet cover blocks playback. No pass can be inferred from
   emulator or TV success; disposition any outstanding merge gate explicitly.
4. After merge, start small follow-ups from the merge result. If stacking before
   merge is needed, pin the parent SHA and coordinate one owner per overlapping
   file. Never copy/cherry-pick selected uncommitted fragments from the other agent.
5. Use one target directory per worktree and pin Rust, adjacent NSH and guarded
   runtime. Reconcile source/feature changes before reusing any earlier evidence.

**Exit:** accepted base, exact-head checks and unresolved manual limits recorded.
No merge, review submission, rerun or branch-protection mutation is performed by
this planning task.

## P1 — Release provenance, changelog and documentation contracts

**Audit:** C10/C11, F4; Claude R2/R6/R10. **Owner:** release/docs.
**Scope:** README, SECURITY, release/CI guides, release planner/publisher tests.

- Recheck current release assets, digests, attestation predicates and workflow
  history; the audit's v0.1.4 observation is historical, not permanent state.
  Correct that version's promise without weakening future build provenance.
- Require the release version's changelog section and accurate signing/preview
  labels. Make expected production signing versus intentional preview explicit;
  fail inconsistent configurations rather than unexpectedly downgrade.
- Rehearse prepare/package/verification without credentials, then require verified
  provenance over final artifact bytes for a future publication. Preserve immutable
  public assets; do not retrofit a false build-origin claim.
- Replace duplicate outdated CI narrative with the canonical matrix. Record
  rollback/yank and upgrade-support procedures; backup compatibility and irreversible
  migration limits must constrain rollback advice.

**Acceptance:** synthetic publish-plan tests reject missing changelog, missing or
wrong provenance and mismatched signing mode; rehearsal result tied to SHA; one
accurate CI matrix. Hosted publication/signing is a later separately authorized
operation. Historical source-license availability gets component/source-offer
verification before any compliance conclusion.

## P2 — Recording terminal-state durability and restart recovery

**Audit:** C3. **Owner:** runtime/store. **Depends on:** P0.

- Return explicit transition outcomes: applied, superseded/current-state mismatch,
  retryable storage failure and terminal error. A false compare-and-set is not a
  license to overwrite newer cancellation/deletion or owner state.
- Make finalization and catalog reconciliation idempotent. Design a durable
  completion intent/receipt sufficient to distinguish complete from partial media;
  a finalized filename alone is insufficient because partial captures are finalized
  too. Bind any receipt to job/source incarnation, outcome and retained file.
- Bound retries/backoff and expose an actionable pending/error state; never silently
  lose the result or retry forever. Preserve acknowledged recording authority,
  deletion ordering, privacy permissions and the no-overwrite guarantee from #98.

**Acceptance:** injected busy/full/failed terminal writes, stale CAS, owner change,
stop/delete races and restart after each finalization boundary retain media and
report the right outcome. Reuse existing runtime/store harnesses. Add bounded
same-executable child-process crash cases rather than only constructing recovery
states. If schema/receipt format changes, include migration and old-file behavior.

## P3 — Capture writer isolation and storage admission

**Audit:** C4, Claude C5. **Owner:** network/runtime. **Depends on:** P2 protocol.

- Move slow writes off the two-worker async executor with a bounded writer queue
  and explicit ownership. PlaybackRelay HTTP serving has its own runtime; measure
  the affected capture/shared-ingest/owner tasks rather than asserting all relay
  work shares the pool.
- Use bounded byte capacity, cancellation-aware backpressure and acknowledgement.
  Do not silently drop media. Drain/stop acknowledgement precedes finalization,
  rename or deletion; a dropped async future does not stop a blocking OS write.
- Account retained bytes plus remaining reservations without double-counting active
  files. Handle concurrent admissions/resume/deletion, use overflow-safe arithmetic,
  and check available space where reliable while still handling ENOSPC at write time.
  Four is a concurrency ceiling, not a guarantee despite insufficient storage.

**Acceptance:** slow/blocked writer with concurrent playback, full queue cancellation,
stop/delete ordering, quota boundaries (including a fourth capture), disk-full and
restart. Set responsiveness budgets from existing shared-plan measurements; enforce
strict timing gates alone. Bound retiring work if native writes cannot be interrupted.

## P4 — Picture overrides by content identity

**Audit:** C5. **Owner:** portable core/app. **Depends on:** P0.

Replace positional keys with stable, source-scoped channel identity available at
Schedule boundaries. Preserve portability and avoid introducing provider/platform
crate dependencies into core. Represent absence of a live channel explicitly;
never alias it to channel zero. Define VOD/recording behavior deliberately (initially
use global sizing unless a separate persistent content setting is specified).
Keep Apply to all semantics and per-surface selection coherent. Do not blindly
persist current numeric indices during any settings migration.

**Acceptance:** refresh/reorder/hide/delete/source replacement, duplicate channel
names across sources, VOD/no-live selection, multiview and auxiliary windows preserve
the intended override. Core wasm and existing player-options/app harnesses pass.

## P5 — Foreground reauthorization without synthetic playback failure

**Audit:** C6. **Owner:** core/player/Android/iOS/runtime. **Depends on:** P0, P4 if
shared playback identity changes overlap.

Introduce a typed foreground/reauthorization event and state transition, separate
from transport failure/retry accounting. Preserve fresh parental grant checks,
source incarnation, owner generation, stream reservation, source tokens, playback
position and Stop intent. Never reopen a cached URL merely because the app resumed.
A denied capacity or expired grant must fail closed with the correct user state.

**Acceptance:** live, VOD-before-Ready, paused VOD, catch-up and recordings across
background/resume; expire grants, deny capacity, delete/change source and press Stop
while resolution is pending. No retry budget consumed merely for resuming; no extra
stream/player. Update host contract fixtures, Kotlin tests and native emulator runs;
iOS requires its own compile and native validation, not inference from Android.

## P6 — CI selection, coverage inventory and check identity

**Audit:** C8/C12, F9/F10/F13; Claude I4/I5. **Owner:** CI.

1. Select actual consumers for `docs/capabilities.md` and `docs/contributing.md`;
   execute changed lint/line-length wrappers as well as linting their syntax.
   Derive or explicitly maintain input dependencies with selector regression cases.
2. Require every workspace crate to have a coverage floor or a documented exemption,
   including `iptv-sqlcipher-init`. Keep no-target selections and exact-test filters
   from silently appearing successful.
3. Add credential-free iOS compile/simulator preparation for relevant paths if iOS
   remains a supported development target. Reuse current build preparation rather
   than assume a bare cargo check has all native dependencies.
4. Specify which full/nightly gates are required before release and how failure
   reaches a maintainer. Inspect actual Dependabot failures and closed PRs before
   diagnosing path dependencies; restore a tested dependency-update path.
5. Only after selection tests pass, prepare migration to a unique intended-app
   aggregate check. Verify zero-lane PR reporting and standalone check names, plus
   rollback for a mistaken requirement. Hosted protection edits are a separate action.
6. Retain release build-environment/native-package inventories. Full byte-reproducible
   builds are a separate requirement, not a claim implied by pinned dependencies.

**Acceptance:** direct selector matrix covers positive/negative cases and all known
consumed docs/scripts; helper/parity/coverage tests pass; intended jobs run on the
candidate SHA. Keep inexpensive docs-only paths where docs are not executable input.

## P7 — Production credential and artifact acceptance

**Audit:** C9/F5; Claude T2/R4. **Owner:** platform validation. **Depends on:** P0/P6.

- Run the existing non-test-support ignored Linux credential test in disposable
  DBus/Secret Service with a fresh profile; verify create, close/reopen, locked broker,
  wrong/missing key and failure-without-plaintext-fallback. Never use login keyrings.
- Add equivalent platform coverage incrementally. Package window startup is not
  proof of encrypted catalog persistence. Keep test-support builds and production
  builds explicit in results.
- Test the actual shipping release candidate, by artifact hash, on ARM tablet/TV
  and native desktop playback paths. #98's debug APK and physical 32-bit TV success
  do not close signed release/ARM64 tablet or speech acceptance.
- Include install-over-previous and backup/restore upgrade cases with synthetic
  profiles, encrypted stores, retained media and recording schedules.

**Acceptance:** evidence ledger lists commit, artifact hash, configuration, ABI,
OS/device and observed result; missing hardware remains a named gap. Signing
credentials and device actions require the appropriate later execution scope.

## P8 — Finish exposed controls and privacy actions

**Audit:** C7/F8; residual F2 polish. **Owner:** core/app/settings.

Inventory visible settings/actions per host, defaults, advanced mode and capability.
Do not rely on an approximate catalogue-string count. Implement clear-history/search
privacy actions with confirmation, persistence and runtime invalidation. Consistently
hide or explain globally unimplemented options in both test and production builds;
do not introduce release-only visibility divergence.

Implement numeric channel selection as portable core state: buffer, timeout,
confirmation, backspace/cancel and resolution by current content identity. Define
whether numbers follow displayed channel numbering and how hidden/unavailable
channels behave before wiring adapters. Use virtual time and synthetic large lists.
Add translated missing-description feedback if desired; do not reopen the fixed
EPG lookup defect.

**Acceptance:** remote, keyboard, touch and accessibility activation match displayed
capabilities; numeric selection survives refresh, respects parental locks and
resolves invalid/ambiguous input predictably. All new text enters localization.

## P9 — Accessibility and localization acceptance

**Audit:** C13/F5/F7; Claude U4–U11. **Owner:** app/hosts/QA.

- Give multiview tiles names, roles, selection state and useful actions, using shared
  semantics. Preserve one player per tile and the primary persistent surface.
- Audit reachable focus states, enlarged/system text, touch targets, modal isolation,
  rail/player/multiview sizing and physical-remote behavior. Verify reachability
  before treating calculated dimmed historical-cell contrast as a confirmed defect.
- Exercise listened-to speech and touch exploration, physical TV during decoded
  synthetic video, and supported desktop screen readers. Source/tree checks are
  separate evidence from speech or usability.
- Localize native recording notifications and track fluent review by locale/journey,
  prioritizing setup, recovery, recording failures and destructive actions.

**Acceptance:** extend existing semantic/layout tests and the manual evidence matrix;
record untested integrations explicitly. Do not advertise new RTL/reduced-motion
support merely because a platform flag exists; assess demand/scope separately.

## P10 — Security boundaries, diagnostics and bounded-input checks

**Audit:** backup decision, F12; Claude S2–S8/D2. **Owner:** security/native adapters.

- Explicitly say portable exports are unencrypted in the existing credential warning;
  document recordings, parental authority, recovery, OS backup and native-versus-Java
  HTTP policy in the security model. Preserve deliberate plaintext compatibility.
- Trace logo byte/pixel/decompression limits through the actual NSH decoder before
  adding a bounded decode contract. Recheck Xtream/private-origin policy, HTTP stream
  disclosure and the compile-time development-playlist hook against intended LAN use.
  Add synthetic negative cases only for established trust-boundary requirements.
- Establish locale initialization's complete native-thread precondition before moving
  it. Audit new logging/diagnostics for secrets and capability URLs. Prefer a user-
  initiated redacted support bundle over silently adding telemetry/crash reporting.
- Track advisory exceptions with owners/review dates and native dependency inventories;
  broaden fuzz oracles/seed coverage for exposed parsers. Evaluate sanitizers/Miri
  only where the platform/FFI boundary supports useful results.

**Acceptance:** explicit threat-boundary notes and bounded adversarial fixtures;
credential markers never appear in exported diagnostics. Optional encrypted backups
need a separately reviewed format, recovery/migration design and standard crypto
implementation; no bespoke crypto or involuntary telemetry is included in this plan.

## P11 — Crash consistency, resource budgets and observability

**Audit:** F11, Claude T3/C6–C9/T6–T9. **Owner:** runtime/tests.

Extend P2 crash-injection technique to backup restore swaps and catalog publication.
Add deterministic boundary acknowledgements, parent deadlines and child cleanup in
existing executables. Track queue age/depth and bounded retry/worker retirement;
coalesce superseded reads/settings requests where safe, retaining durable write
ordering. Measure Windows software-render cost and multiview CPU/frame latency on
representative synthetic media before choosing a GPU/backend rewrite. Replace fragile sleeps with actual state acknowledgements when they cause
failures, and clean recording sibling directories as part of fixture ownership.

**Acceptance:** crash/restart preserves committed data, avoids overwrite/leak and
reports incomplete work; queue/latency tests use meaningful synthetic contention.
Do not transform every sampled metric or long function into an unconditional rewrite.

## P12 — Cohesive ownership and contributor documentation

**Audit:** F6, Claude A3/A5/M1–M5/D3–D6. **Owner:** runtime/app/docs.
**Depends on:** correctness packages and stable characterization tests.

Extract one cohesive runtime domain at a time behind the current facade; make
start/cancel/publish transitions atomic at the state-model level. Separate shared
application state from per-window presentation to avoid manual signal copying.
Replace implicit off-thread owner creation with an explicit safe contract after
tracing actual callers. Preserve runtime API v1 and core portability.

Add a short documentation index, subsystem/runbook ownership, fork patch/upstream
status ledger and tested upgrade/rollback procedure. Separate user setup and
troubleshooting from developer builds; expose current platform support limits.
Fix the 25 existing rustdoc link warnings before enforcing a no-new-warning gate.
Consider core/domain extraction only after measuring dependency rebuild cost; do
not prescribe another crate merely because a module is large. Contributor and fork
concentration are maintenance risks, not proof another maintainer cannot help.

**Acceptance:** existing identity/owner-generation tests unchanged in meaning;
window isolation scenarios pass; contributor can reproduce synthetic setup and
locate release recovery instructions without relying on another agent's /tmp logs.

## P13 — Safe validation operation and durable evidence

**Added from #98's documented incident. Owner:** validation tooling.

The Android record discloses deletion of original tab36 userdata/cache/encryption
images during setup. Treat this as a concrete tooling defect to prevent, not a
reason to rerun against the original profile. Design validation commands around
complete disposable AVD copies, a dedicated application ID and explicitly scoped
serials. Before any destructive cleanup, verify a harness-owned marker and resolved
scratch root; reject symlinks/path escapes and original AVD/device profile roots.
Keep rollback/settings restoration and kill/reap logic in the same runner.

**Acceptance:** safe temporary-directory tests prove original profile paths are
refused, interrupted setup cleans only owned copies, and all adb actions specify a
serial. Add operation manifests and artifact hashes to evidence; no profile reset,
credential access or physical data deletion is authorized by this planning request.
Preserve failure evidence and distinguish fixture failures from product failures.

## P14 — Native teardown decision, not speculative repair

**Audit:** F3. **Owner:** player/native hosts. **Depends on:** #98 experiment and
current pinned native contracts.

Carry forward #98's measured synthetic wait and retained order. A future change
requires a real supported control/render interaction establishing the harmful wait
and a complete ownership graph, including terminate_destroy, callbacks, render
context/current platform context, native handles and reservations. If a different
protocol is justified, bound retirement/admission, account capacity until upstream
use really stops, and define process-exit behavior. Do not detach ownership or free
render resources early based on the audit's illustrative sketch.

**Acceptance:** affected-host native evidence, eventual release, no unbounded reapers,
no extra stream and preserved main/PiP surfaces. Otherwise retain the documented
tradeoff and explicitly defer redesign; “investigated” is not “fixed.”

## Execution order, ownership and completion

| Wave | Packages | Exit criterion |
|---|---|---|
| 0 | P0 | Accepted exact #98 base and remaining platform/review limits recorded |
| 1 | P1, P6, P13 | Accurate release promises; selection corrected; safe repeatable validation |
| 2 | P2, P4; then P3 and P5 | Recording durability, picture identity and lifecycle changes have regressions |
| 3 | P7, P8, P9, bounded P10 work | Production storage/artifact and core user journeys have concrete evidence |
| 4 | P11, P12; P14 decision | Recovery/resource and maintenance improvements supported by measured need |

Independent source/doc review may run concurrently; mutations of recordings,
playback, runtime ownership and CI plans have one owner each. Keep changes as small
reviewable PRs; do not turn the remaining audit into another 179-file batch. Each
package records audit IDs, base/head SHA, owner, implementation status, tests,
manual gaps and rollback/data-compatibility notes. “Written,” “source-reviewed,”
“tested,” “merged” and “release-verified” are separate states.

Map remaining compact audit items explicitly: F4→P1/P12; F9/F10/F13→P6;
F11→P11; F12→P10; C11→P1; C12→P6; C13→P9. Additional Claude backlog claims
remain investigations in P10–P12 unless source/behavior verification establishes a
specific defect. Closed #98 items are reopened only for a new failing regression.

For code changes use existing `tests/it/` and `tests/app/` modules; no new top-level
integration executable. Ordinary tests use `--tests`, doctests run separately,
Clippy includes `--all-targets`, and isolated timing gates use the shared CI plan.
Required finishing evidence includes `cargo test -p iptv-core`, core wasm build,
`cargo build -p iptv-app`, and affected Android tab36/tv36 execution in disposable
profiles. The full shared plan can supply equivalent evidence without redundant
runs. Feature/configuration/native-artifact identity must accompany each result.

This plan adds no code, new accounts, telemetry, publication, hosted permission
changes or commitments to optional features. It is left uncommitted for review.
