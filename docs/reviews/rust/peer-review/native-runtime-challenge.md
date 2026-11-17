# Native adapter, runtime and shell challenge review

Second-pass review against `0e197073a272eceb0631e4d53b08d2823950c416`,
2026-11-16. Scope: the zero findings in `iptv-sqlcipher-init`, three store
findings, three player findings, seven runtime findings and five shell findings.
This is an independent source challenge and fix-plan input, using the
`rust-review` and `rust-best-practices` skills. The crates are internal native
libraries or application integration; the declared runtime v1 foundation facade
has the stronger, documented compatibility contract. Edition 2024 and Rust
1.99.1 apply. Local futures and `Rc` ownership remain intentional.

Read the workspace AGENTS, original workspace plan, review README, runtime v1
contract, security contract and all five reports. Re-read the affected source,
callers and portable page owner. Consulted the actual pinned SQLCipher C source
in the Cargo registry and installed libmpv `render.h`. No Cargo commands,
new execution probes, source/test/manifest edits, staging or commits were made.
The baseline evidence and its failing/missing gates remain exactly those in
[the review README](../README.md); prior temporary probes were not rerun here.
Only this challenge artifact is owned by this reviewer.

P1 means confirmed correctness/security work for the next release; P2 means
robustness/API work or an investigation without an established release defect;
P3 means polish. An accepted source path is not a claim of a newly executed
regression or successful platform validation.

## Must fix: confirmed release work

| Finding | Disposition | Priority | Independent reason and source |
|---|---|---|---|
| `iptv-store-sqlite-001` | Accept | P1 | `src/epg.rs:590` exhausts its binding cursor before feed/programme reads; `src/epg_titles.rs:27` similarly retains reusable numeric EPG IDs after its first statement. Neither owns a read transaction. `epg_guide` at `epg.rs:519` also combines separate metadata/feed snapshots. Pin one logical read snapshot, sharing a caller's active transaction. |
| `iptv-player-mpv-001` | Correct | P1 | `mpv/mod.rs:595` increments `loads`; pre-load refusal branches in `mpv/worker.rs:328`, `:355`, `:374` emit only `TransportError`; `mod.rs:865` stores this as a clue and only `Loading` decrements. This can leave buffering and suppress later readiness. The proposed hook-failure extension needs distinct accounting, detailed below. |
| `iptv-player-mpv-002` | Accept | P1 | `mpv/worker.rs:234` sends command notes, then `:241` enters an infinite event wait before `:307` notifies. Command responses need not generate an additional native event. Use a nonblocking first drain when notes were sent, retaining notification coalescing. The prior ctypes result supports native wait semantics, not a passing production regression. |
| `iptv-runtime-001` | Correct | P1 | `src/inspection.rs:150`–`:224` delegates to adapter inspection without capturing parental authority. `store/inspection.rs:295`, `:349`, `:397` enforce only legacy overlay exclusions. Modern source/category/custom-group/adult and explicit policy is in `runtime/parental.rs:537` plus store membership. The public runtime v1 text explicitly prohibits a known-ID bypass. The fix must handle granted legacy blocks and authorization-aware continuation, detailed below. |
| `iptv-runtime-002` | Correct | P1 | `src/recordings.rs:524`, `:588`, `:668` and `recordings/rules.rs:40`, `:119`, `:142` authorize before later awaits and queued durable mutations. No snapshot/revocation gate enters the accepting transaction. New intent and already accepted background authority must stay distinct; fix the transaction boundary and legacy unlock conflict together. The recorded race probe remains inconclusive as durable-state proof. |
| `iptv-runtime-003` | Merge with `iptv-guide-002` | P1 | `src/data/pages.rs:78` resolves the owner's current schedule by source at arrival; `guide/pages.rs:161` accepts the same page/range while pending. A replacement schedule or eviction/retry can reuse that tuple. One cross-crate request identity fix must protect every success and failure reply. |
| `iptv-runtime-004` | Accept; implement with page-identity batch | P1 | `src/data/pages.rs:67` converts a storage error to `GuideStore::default()` and inserts it; portable pages subsequently regard it as loaded. Retain true empty success, expose failure and make another user lookup retryable without resetting the schedule. Identity checking must precede clearing any pending request. |
| `iptv-runtime-006` | Accept, narrow to direct runtime callers | P1 | `src/vod.rs:735`–`:755` awaits a final source read after access checks. `src/recordings.rs:823`–`:848` awaits a file lookup; `:758` awaits relay creation afterwards. The direct results lack the final owner/grant fence. `app/player.rs:806` synchronously calls `source_allowed` at actual shell publication, so this is not evidence of a current shell decoder bypass. Experimental API status does not make the direct-call revocation race correct. |
| `iptv-runtime-007` | Accept | P1 | `inspection/probe.rs:1092` calls `validate_candidate`; `:511` allocates an independent token and fresh 60-second deadline. `:544` uses these for the stage integrity scan, retaining a real worker after the caller's shorter cancellation/deadline. Runtime v1 expressly covers resource release, not only timely public error replies. Forward the child's token and original deadline, including current-configuration queueing. |
| `iptv-app-002` | Accept | P1 | `src/guide_assign_view.rs:159` saves and unconditionally closes on reply, unlike open's monotonic `REQUEST` check. Closing/reopening while the write waits lets A close B. Capture the existing request ticket at choice and fence only stale UI effects; the already accepted save may still finish. |
| `iptv-app-003` | Correct | P1 | `src/guide_repair_view.rs:276` discards begin's control and checks only a generation that `library/guide_repair_screen.rs:58` restarts per screen. Other callbacks have the same collision on errors or committed results after cancellation. Retain a per-operation cancellation ticket (or monotonic screen instance), check it before any state/finish mutation, and release stale preview resources with the accepting owner. |
| `iptv-app-004` | Accept | P1 | `src/menu_view.rs:485` always renders generated fixture description text while actual schedule description lookup exists in `header.rs:192`. Show the selected programme's description, including combined schedule and page invalidation, or its actual empty state. This is an incorrect viewer-facing field, not a request to copy reference wording or artwork. |
| `iptv-app-005` | Correct | P1 | `src/player_keys.rs:171` omits VOD and early resume; `chooser.rs:275` sleep clears only live `playing`, while `video.rs:169` admits recorded/VOD/early independently. Common stop at `player.rs:286` clears neither `early` nor startup resume intent. Broaden admission, retire all stop state, and route sleep through it while preserving persistent surfaces. |

## Should fix and investigate

| Finding | Disposition | Priority | Independent reason and source |
|---|---|---|---|
| `iptv-store-sqlite-002` | Accept | P2 | `src/migrate.rs:150` checks only count/max, so `[0, 2]` passes and a current-version malformed history can bypass migration. For nonempty history require min = 1 as well; the integer primary key then establishes contiguity. This is corruption detection robustness; no ordinary migration generated the bad history. |
| `iptv-store-sqlite-003` | Accept | P2 | Read-before-write deferred mutations can fail promotion under supported WAL contention; `recordings.rs:194`, `overlay.rs:225`, `organise.rs:152` retain that shape. `schedule_recording_rule_with_first` at `recordings.rs:351` calls `insert_rule`, whose first SQL reads channel admission before writing. `library_preferences.rs:120` reads ordering before starting any transaction, so move those reads inside the mutation boundary. Use Immediate only for mutations and retain bounded busy handling. |
| `iptv-player-mpv-003` | Investigate before lifecycle change | P2 | `Worker::drop` at `mpv/worker.rs:209` synchronously joins; `Inner::drop` at `mpv/mod.rs:1098` does this before freeing rendering. A render/client wait dependency is possible, but a production hang or unacceptable latency was not established here. Installed `render.h` explicitly forbids waiting for ordinary client calls from a render thread, which warrants a targeted native investigation. It does not validate the proposed reaper/lifecycle implementation on each host. |
| `iptv-runtime-005` | Accept | P2 | `inspection/probe.rs:591`–`:638` performs separate source/policy/incarnation/feed reads and repeats `source(...).expect("source checked")`. Analysis jobs do not implicitly start read transactions (`store/inspection.rs:238` only installs progress handling). A concurrent delete may panic the caught worker task; edits may mix configuration. Reuse one source and one snapshot, return typed missing/stale errors, and audit the specifically named compound reader helpers. No process crash is demonstrated. |
| `iptv-app-001` | Accept | P2 | `src/content_image.rs:103` returns a cached image after permit acquisition without sending `wanted`, whereas `:157` consumes only the oneshot. Send cached success too. This is a real recycled-row placeholder bug; its impact is limited to artwork completion and does not imply transport credential leakage. |

No findings were assigned to `iptv-sqlcipher-init`; retain the zero-finding
conclusion. `src/lib.rs` correctly caches only readiness success, retries on
failure and opens fresh connections until export registration is visible.
Pinned `sqlcipher.c:111280` takes the static master mutex and returns early
after successful cipher initialization; `:111416` releases it before
`:111420` registers the export extension. The adapter's explicit hook followed
by actual export polling addresses precisely this registration gap. No new
P1/P2/P3 is justified from its five-second bounded polling alone. Recheck this
private ABI and ordering on a native dependency change.

## Required corrections to the consolidated fix plan

### Authorization must preserve successful unlock and bounded semantic pages

Use one authoritative parental decision for both modern and legacy blocks.
`store/parental.rs:127` includes the legacy live overlay, explicit blocks,
adult-labelled groups and explicit unblocking exceptions; runtime
`native_allowed` combines these with ancestry and grants. Merely adding that
gate in front of `inspection_channel` leaves the adapter's unconditional
`channel.blocked` rejection intact. The same issue affects hard-coded SQL
`COALESCE(o.blocked,0)=0` in pages. An unlocked legacy channel must become
readable through the runtime facade, while raw adapter calls without trusted
runtime authority must not acquire an unrestricted bypass.

Filtering a limited page after the fact can return a false empty/end result
and lose allowed items. Select/filter before forming the page limit, lookahead
and cursor, in one consistent membership/evidence snapshot. Bind continuation
to the relevant authorization context as well as owner, scope and semantic
evidence, so a cursor made under a revoked grant cannot silently continue with
different visibility. Revalidate the accepting grant at publication after the
last await, including expiration. Preserve typed missing/disabled/access/storage
outcomes and the three-second end-to-end budget. Cover channel, programme,
candidate and coverage surfaces; inspect repair/probe disclosure/admission
against their actual contracts rather than announcing a blanket exemption.
Keep v1 signature/type changes behind the contract-revision policy.

### Recording authority is admitted durably once

Capture the source incarnation and parental snapshot before queuing new or
changed intent. Resolve current membership, enabled state and policy in the
same transaction that first schedules, updates or enables it. A runtime gate
immediately before calling a store method that starts its own transaction is
not sufficient to establish that atomicity against another connection.
Refactor the internal adapter boundary so the trusted gate and mutation share
the writer transaction; do not let portable policy depend on platform APIs.

The existing `insert_job` at `store/recordings.rs:65` rejects legacy overlays
even after a runtime PIN grant. Correct that inconsistency while introducing
the trusted admission path; preserve fail-closed defaults for untrusted raw
store entry points. Include one-shot, updates, recurring first job and rule
re-enable. Ordinary materialization/execution of previously accepted recurring
authority must survive a later UI lock. Cancellation/stopping/removal should
not gain an unnecessary PIN requirement. Preserve idempotent receipt recovery
and do not report a committed job as uncommitted due to a later lock.

### Page completion needs both owner identity and attempt identity

Merge guide-002 and runtime-003 into one implementation batch, with runtime-004
using its same identity. Source ID and equal hourly window are insufficient.
Protect replacement schedules, combined children, source reincarnation,
eviction/re-request and failure/retry of the same day. A stale failure must not
clear the newer attempt's pending state; a stale success must not fill it.
Deliver failure without installing a loaded empty store. Waking waiters must
not cause an immediate infinite retry loop under persistent storage failure;
make the error observable and retry admission deliberate/bounded. A genuine
successful empty day remains cached normally.

### Native failures need accounting without weakening transport guards

Every pre-load guard refusal needs exactly one terminal completion, associated
with its request, and newest-load handling consistent with successful loads.
Keep normal `TransportError` log/relay evidence aggregation until end-file;
turning every clue into an immediate error introduces duplicate/stale failures.
The failed `stream-open-filename` hook substitution at `worker.rs:262` occurs
after `execute` has emitted `Loading(id)` at `:389`. It needs a current-entry or
request-bound terminal error, not an unconditional second decrement of `loads`.
Test refusing two queued loads, refusing then accepting, stopping during queued
loads, and permanent hook retirement. Missing native guard support must remain
fail-closed with zero upstream request; stock libmpv is not a permissible
fallback to make the baseline decoder tests pass.

For 003, first reproduce/measure removal with a controlled synchronous native
operation on the actual render/control threads. Document platform GL context
ownership, the callback-waker lifetime, surviving worker ownership, retained
`dead` worker and final handle destruction. `free_render` makes the GL context
current and unregisters callbacks; the render context must precede final mpv
core destruction. An off-thread reaper sketch alone proves none of these
conditions. Do not reorder destruction or introduce unbounded reaper threads
before those obligations and repeated multiview/PiP teardown tests are resolved.

### Shell callback fences cover errors and committed replies

Guide assignment can reuse its existing monotonic request counter. Repair needs
a ticket that survives numeric generation reset: retain a clone of begin's
cancel token before moving the control into a runtime operation. Check that
ticket and current screen identity before all success, error and `finish`
mutations. Runtime cancellation alone cannot suppress a committed/lost reply
or prevent an old error from finishing a new screen. A stale preview must be
released using its captured owner, not whichever runtime is ambient later.

For Stop/sleep, enumerate live, archive, recording, movie, episode, early
resume, multiview and desktop PiP. Call the common stop operation so final
on-demand progress is queued before the playback capability is closed, source
publication and in-flight resolution are retired, provider reservations are
released, and reconnect state cannot revive playback. Clear `early` before
later `source_ready` invokes `resume::adopt`; `resume.rs:129` otherwise explicitly
restarts an early identity whose stream closed. Clear startup resume intent as
appropriate. Verify a stopped early request remains stopped across its delayed
preparation and guide arrival. The separate suggestion about VOD arriving while
the initial early channel read waits remains an investigation candidate; this
review does not promote it to a new confirmed finding without a reachable
product sequence and regression.

## Validation required before implementation closure

Add deterministic synthetic regressions to existing integration/unit harnesses;
do not add test executables. Use real SQLCipher/store transactions for snapshot
and migration evidence, barriers for writer/revocation and page completion
ordering, independent Runtime consumers for access and late-grant results, and
the actual stage worker for cancellation/resource release. Keep genuine empty
success separate from failure, successful unlock separate from lock denial,
and accepted recording jobs separate from queued new intent.

Native fixes need a correctly guarded runtime and actual decoding/surface
evidence, including retained frame, PiP and multiview on affected hosts. A
missing device/host lane remains pending. No source-fix gate is satisfied by
this challenge artifact. The existing NSH exact-revision failure, absent core
wasm target and failed native-player baseline must remain visible; do not edit
the NSH pin or weaken transport guard requirements as review cleanup.

## Final consolidated-plan review

Reviewed the initial draft and its revised W7, priority and W14 text in the
[consolidated plan](../../../plans/2026-11-16-rust-review-consolidated-fix-plan.md).
All 18 IDs appear with implementation ownership: store
001/003 and runtime 005 in W4; store 002 in W13; player 001/002 in W8 and
003 as investigation W14; runtime 001/002/006 in W7; runtime 003/004 merged
with guide 002 in W5; runtime 007 in W1; all five app IDs in W9. SQLCipher's
zero-finding conclusion is retained. No finding disappeared or was marked fixed.

The draft incorporates transaction-local recording authority and legitimate
legacy unlocks; authorization before page limits/lookahead/cursors; owner plus
attempt identity for page success/failure; captured-owner preview release;
pre-load versus post-Loading hook accounting; and complete Stop/sleep handling,
including pending early-resume reads. W1 tests actual worker release. W14
authorizes investigation only and explicitly prohibits a blind destruction
reorder or unbounded reaper. These are appropriate implementation constraints.

**Final verdict: no blocking omission remains in this assigned area.** The
revised W7(1) explicitly requires a synchronous accepting-owner, grant-epoch and
expiry check after the last analysis/query await and immediately before returning
inspection data. It also includes the authorized-SQL-result followed by relock
before delivery regression. This resolves the first draft's sole security-plan
omission: authorization inside the SQL snapshot alone cannot fence delivery.
The verdict approves the plan's scope and gates; no defect is fixed and no
source/lifecycle change is authorized by this review run.

Priority calibration against the draft's stated production/API criteria:

| IDs initially differing between this challenge and the draft | Reconciled final assessment |
|---|---|
| runtime-006 | P2 is defensible: these direct helpers are experimental and the current production shell has its own final fence. W7 still fixes the confirmed defect and tests independent native callers. Do not describe it as a shell playback bypass. |
| runtime-007 | P2 is defensible with the explicit bounded-robustness rationale: the caller can receive cancellation promptly while a worker remains for a fresh finite 60-second scan; a product-wide stall was not demonstrated. This remains a stable v1 cancellation/resource-release violation, so it must stay accepted and W1's real-worker gate must remain. |
| app-002 | Accept final P2 for limited, recoverable picker dismissal. The revised priority criteria explicitly permit recoverable UI defects at P2 and resolve the initial wording inconsistency. |
| app-003 | Accept final P1. A normal close/reopen interleaving installs the prior source's commit into a new screen, exposing Verify/Undo for that commit. Runtime authorization does not establish that an action on an otherwise authorized wrong commit matches the viewer's intent. This is a content/action identity defect, even though no parental bypass is claimed. |
| app-004 | Accept final P2 for a wrong, low-impact viewer field. The revised priority criteria explicitly permit recoverable field defects at P2. |

These priority distinctions do not retract any accepted defect or permit its
regression gate to be skipped. Prefer explicit impact-based wording to using a
report's Must/Should heading as a release-severity proxy.

W14 now correctly records observed join behavior and the retained ownership
tradeoff without asserting an unproven time bound. It remains investigation
before any teardown protocol change, with native API/thread ownership proof
and affected-platform validation still required. No blocking priority or
lifecycle wording issue remains. This follow-up used only the requested plan
read and this artifact update; no execution tests or other edits were made.

### Final CLI reconciliation follow-up

Verified the added W4/W7 paragraphs and their corresponding rationale in
`consolidation-decisions.md` against the specific source paths. Retaining
store-003 is correct: `store_service.rs:97` owns a writer per `Service`,
`Runtime::new` at `lib.rs:168` creates the UI service, and conditional background
host acquisition at `recordings/host.rs:61` creates another for the same ports.
`host.rs:105` selects that host service for execution instead of reusing the UI
service. A process storage lock cannot by itself serialize those independent
in-process writer queues. The plan correctly limits write-intent changes to
actual read-then-write mutations and retains contention regressions.

The parental correction is also necessary. `app/lib.rs:354` calls
`parental_content_left` during an ordinary channel change; that method at
`runtime/parental.rs:400` increments the epoch unconditionally. Existing
`native_allowed` at `:537` rejects an epoch mismatch before even checking whether
policy protects the target. Its wholesale reuse would wrongly fail unrelated
unprotected inspection/recording requests. W7 now requires effective-policy,
membership and actually required-grant revalidation, separates cursor
invalidation from fresh-request rejection and adds unprotected-zapping tests.
Its final publication fence remains essential, interpreted through this
effective-authority rule rather than as blanket epoch equality for all targets.
Moving the runtime's preflight duplicate return (`recordings.rs:553`) into
transaction-local acceptance is consistent with the existing transactional
`insert_job` duplicate lookup (`store/recordings.rs:72`) and the same-transaction
authority requirement; retain idempotent recovery of already accepted intent.

**Final verdict remains: no blocking omission in this assigned area.** These
CLI reconciliation corrections improve the plan without weakening protected
content admission or authorizing source changes. Only source/document reads and
this artifact append were performed; no builds or execution probes were run.
