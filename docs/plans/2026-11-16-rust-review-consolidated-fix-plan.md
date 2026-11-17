# Consolidated Rust workspace fix plan

Created: 2026-11-16. Status: reviewed; implementation not started.
Baseline: `0e197073a272eceb0631e4d53b08d2823950c416`.

Fix the confirmed defects from fourteen crate reviews while preserving Patina's
portable state, stable runtime v1 semantics and persistent player ownership.
The [report index](../reviews/rust/README.md) contains individual findings,
reproductions, coverage and baseline checks. `iptv-nav` remains outside this
review and must be included when validating the eventual combined implementation.

This run ends with a reviewed plan. No implementation, tests, manifests or
lockfiles are changed, and no commits are created. Every fix below is future
work. Passing review does not mark a defect fixed or a platform gate passed.

## Evidence and priorities

The first pass produced 39 finding IDs. Most mechanisms have either executable
synthetic evidence or a specific source interleaving. Guide request identity
`iptv-guide-002` and its runtime publication consequence `iptv-runtime-003` share
one fix. Native player teardown remains an investigation before any lifecycle
change. Do not treat illustrative report patches as independently validated code.

Use P1 for wrong content/operation identity, stalled accepted controls, lost
guide data and violated runtime access contracts that should be resolved before
release. P2 covers recoverable UI/field defects, bounded API robustness,
diagnostic confidentiality and local availability problems with narrower triggers. A report's Must fix category does
not establish a remote exploit, release crash, or P1 priority by itself.

The required full host baseline is not green. NSH has a commit mismatch with
identical source trees; the wasm target standard library is missing; native
player tests link the system libmpv and have 10 failures. Workspace formatting,
strict Clippy, app build, 1483 non-player tests and 13 doctests passed. App unit
and headless scenarios also passed before the player target failed. Retain the
exact evidence and limitations in the report index; recover the intended native
runtime baseline before using decoder tests to assess a fix.

## Finding dispositions

Accept means the mechanism is established, with the scope and corrections below.
All accepted findings remain open for implementation. Investigate means the
observed risk merits a bounded task, not permission to apply an unproven patch.

| Finding IDs | Disposition and priority | Package and scope correction |
|---|---|---|
| iptv-core-001 | Accept, P2 | W10; formatting disclosure, no observed production log leak |
| iptv-core-002 | Accept, P2 | W11; safe public mutation can invalidate caches; audit callers before changing fields |
| iptv-core-003 | Accept, P2 | W11; zero-step public API hang; normal app navigation uses nonzero steps |
| iptv-settings-001 | Accept, P2 | W2; distinguish large step arithmetic from invalid custom schema helpers |
| iptv-settings-002 | Accept with API correction, P2 | W11; replace actual ad hoc key constructors before sealing wrappers |
| iptv-providers-001 | Accept, P2 | W2; extreme untrusted numeric metadata; no ordinary release crash established |
| iptv-providers-002 | Accept with compatibility correction, P2 | W10; redact Debug while preserving supported request/userinfo behavior |
| iptv-providers-003 | Accept with encoding correction, P1 | W2; preserve raw signed query bytes and existing append/duplicate-order contract |
| iptv-epg-001 | Accept, P2 | W2; maximum episode ordinal must fail consistently without wrapping |
| iptv-epg-002 | Accept, P2 | W2; centuries-scale relative compact slots and full-width arithmetic |
| iptv-epg-003 | Accept, P2 | W2; reject impossible seconds while preserving deliberate leap-second policy |
| iptv-library-001 | Accept, P2 | W10; editor formatting exposure, not observed logging |
| iptv-library-002 | Accept with caller coordination, P1 | W3; enforce unique feeds through import, edit and persisted data paths |
| iptv-guide-001 | Accept, P2 | W10; redact contexts and link commands transitively |
| iptv-guide-002 | Merge with runtime-003, P1 | W5; issuing page/schedule identity must survive every asynchronous hop |
| iptv-search-001 | Accept, P1 | W6; refresh can move focus and Play to another channel |
| iptv-search-002 | Accept, P2 | W2; bounded timestamps must fail or be skipped explicitly, not silently corrupted |
| iptv-import-001 | Accept, P1 | W1; never discard a live cancellation waiter; remove abandoned registrations |
| iptv-import-002 | Accept, P2 | W1; bound warnings during accumulation, not only final catalog construction |
| iptv-net-001 | Accept as local hardening, P2 | W12; private-directory mutation is required; no remote arbitrary-file exploit proven |
| iptv-net-002 | Accept, P2 | W12; authenticated stalled clients can consume every serving slot |
| iptv-store-sqlite-001 | Accept, P1 | W4; one reader operation needs one coherent SQLite snapshot |
| iptv-store-sqlite-002 | Accept, P2 | W13; validate the exact migration ledger before ordinary opening |
| iptv-store-sqlite-003 | Accept with targeted transaction correction, P2 | W4; reserve write intent only for actual read-then-write mutations |
| iptv-player-mpv-001 | Accept with accounting correction, P1 | W8; complete pre-load refusals once; later hook failure has different accounting |
| iptv-player-mpv-002 | Accept, P1 | W8; notify already queued replies without another native event |
| iptv-player-mpv-003 | Investigate before lifecycle change, P2 | W14; UI join is established, a production deadlock and safe reorder are not |
| iptv-runtime-001 | Accept with access integration correction, P1 | W7; honor grants before paging, legacy overlay denial and cursor construction |
| iptv-runtime-002 | Accept with transaction correction, P1 | W7; check authority inside durable intent acceptance, preserve already accepted jobs |
| iptv-runtime-003 | Accept; owns merged guide-002 integration, P1 | W5; reject old replies after refresh and source recreation |
| iptv-runtime-004 | Accept, P1 | W5; storage failure must stay retryable rather than becoming cached empty data |
| iptv-runtime-005 | Accept, P2 | W4; capture configuration in one snapshot and return typed absence |
| iptv-runtime-006 | Accept with scope correction, P2 | W7; direct experimental runtime callers; current shell has a final grant fence |
| iptv-runtime-007 | Accept, P2 | W1; preserve original worker cancellation and deadline through integrity validation |
| iptv-app-001 | Accept, P2 | W9; every successful cached fetch must send the observed reply |
| iptv-app-002 | Accept, P2 | W9; old assignment completion must not close a newer picker |
| iptv-app-003 | Accept with minimal ownership correction, P1 | W9; capture/check existing cancellation plus request generation/open state |
| iptv-app-004 | Accept, P2 | W9; render selected programme description through existing data lookup |
| iptv-app-005 | Accept with full stop correction, P1 | W9; clear/fence all active and pending playback variants |

`iptv-sqlcipher-init` has a completed report with no confirmed defect. Keep its
ABI/concurrency tests and success-only initialization behavior. No speculative
rewrite is scheduled for that crate.

## Work packages

### W1 Cancellation and worker bounds

Owners: `iptv-import`, `iptv-runtime`. Findings: import-001, import-002, runtime-007.

Give each cancellation future a stable waiter registration, update its waker on
repoll, and unregister it on completion/drop under the cancellation lock. Never
evict another pending waiter to impose a bound. Preserve wasm portability and
runtime independence. Cap Stalker warnings as pages merge, preserving dropped
warning accounting. Forward the original child token and deadline through probe
configuration/integrity validation, including a queued configuration lookup.

Regression gates: independently counted 33+ waiters, abandoned waiters, changed
wakers, registration/cancel races, bounded multi-page warnings, and cancellation
of real blocked probe work followed by worker-permit release. Use existing unit
and integration harnesses. Parent query completion alone does not prove that its
blocking worker stopped. Keep waking outside the cancellation lock and retain
native live-preparation cancellation plus runtime import rollback scenarios.

### W2 URL replies and numeric validation

Owners: `iptv-providers`, `iptv-epg`, `iptv-search`, `iptv-settings`.
Findings: providers-001/003, epg-001/002/003, search-002, settings-001.

Repair query-only Stalker replies using parsed URL boundaries while preserving
raw query bytes, percent encoding, fragments, duplicate keys and pair order.
Retain the existing documented raw append semantics: append the reply query to
the original query, keeping duplicate keys in order; replacement requires separate
provider evidence and an explicit behavior change. Preserve portal base-prefix
handling for relative media paths. Do not apply form decode/re-encode to signed
existing query material.

Use checked/widened arithmetic before narrowing, reject impossible offsets and
episode ordinals, and validate raw seconds before folding a real leap second. Reject non-ASCII
fixed-offset components before byte splitting: a value such as `UTC+é1` can
panic at a UTF-8 boundary even before numeric rejection.
For compact schedules/search indexes, specify behavior for invalid intervals or
unrepresentable spans; never silently rewrite them to maximum offsets or zero
duration. Widening two u32 fields alone does not protect an arbitrary i64 base.
Validate positive grid steps and safe bounds, then saturate/check multiplication
before addition. For full-width caller values, widen the complete grid/step
calculation before clamping so intermediate saturation cannot change an otherwise
representable result. Preserve normal calendar and UI behavior.

For epg-003, retain the documented rejection of impossible calendar values:
seconds 61–99 are invalid, while 60 keeps the existing leap-second folding.
Do not silently introduce a new tolerant mode. If compatibility evidence later
requires one, make that a separately reviewed parser policy.

Use small commits per owning crate. Regressions cover exact supplied counterexamples,
adjacent boundaries, accepted normal values, signed query bytes and duplicate keys.
Run parser-to-consumer tests and relevant existing fuzz targets. Exercise debug
and release arithmetic where the original behavior differs.

### W3 Unique guide feed identities

Owners: `iptv-library`, runtime import/edit callers. Finding: library-002.

Define one collision-free feed allocation policy with explicit exhaustion and
provider count handling. Preserve durable feed IDs and manual mappings. Ensure
1000 provider URLs cannot collide with a second user guide. Trace production
callers in runtime `imports.rs` and `library.rs`; reject invalid allocation before
publication and propagate a typed failure without replacing the old catalog.
Coordinate `iptv-import::epg::from_m3u_header` before fetching an unbounded header
list and the store repair allocator before reserving a new ID. A documented
ingestion cap with dropped-feed warnings can retain a valid playlist, provided
it shares the collision-free allocation policy. Do not reject all playlist
content merely because surplus guide URLs were present.

Inspect previously stored configurations for duplicate/reserved IDs. Define
fail-closed validation or a transactional repair that preserves mappings; never
silently renumber an existing feed. If persistence changes are needed, add a
historical synthetic fixture and migration/restore checks. Do not mistake a
runtime test fixture using sync_provider for a production restore path.
Record the selected validation/repair behavior per persistence path before
implementation so migration and restore gates assert that exact outcome.

Regression gates: provider/user interleaving around 999/1000 URLs, edits/reorder,
ID exhaustion, cancellation/error propagation and old-catalog retention.

### W4 Coherent database snapshots and mutations

Owners: `iptv-store-sqlite`, `iptv-runtime`.
Findings: store-sqlite-001/003, runtime-005.

Wrap compound EPG binding/feed/programme/title reads in one read transaction.
Do not allow an old numeric epg_channels ID to resolve against a replacement
feed. Capture probe configuration once and remove the checked-then-expect read.
Audit compound runtime guide/library/organization reads against the same snapshot
requirement. Keep these transactions on the existing read/analysis lanes and reuse an
already active caller transaction rather than starting a conflicting nested one.

For demonstrated read-then-write paths shared by UI and recording connections,
acquire write intent before their first read or use a bounded, safe retry policy.
Do not blanket-convert readers or long analysis operations to write transactions.
Keep cross-thread ownership unchanged.
The single writer is per Service, not per process: the UI constructs one in
runtime `lib.rs:168`, and background recording ownership constructs another in
`recordings/host.rs:61`. The process storage lock excludes other processes; it
does not serialize these two in-process writer lanes.

Regression gates: barrier-controlled EPG replacement with ID reuse; coherent
metadata and title iteration; source deletion/edit during configuration; two
Service writers on a demonstrated read-then-write path completing without
SQLITE_BUSY_SNAPSHOT or retrying within the stated bound; rollback and reader
responsiveness. First reproduce the failed snapshot promotion on the baseline.
Run the shared isolated timing gates after integration.

### W5 Page request ownership and failure recovery

Owners: `iptv-guide`, `iptv-runtime`. Findings: guide-002, runtime-003/004.
Depends on W4 for coherent page reads.

Bind requests and replies to the exact page owner and request incarnation.
Validate that identity at insertion; matching source/time/page index is
insufficient. Handle refresh, eviction/re-request and source recreation.
Separate failure completion from successful empty data: clear only the failed
pending request, notify dependent work appropriately and allow a later user
lookup to retry. Do not create a render-driven retry loop or publish stale pages.
Complete catch-up readiness with an explicit failure so its waiter cannot hang.

Regression gates: delayed old reply across same-window refresh and eviction,
combined schedules, deleted/recreated sources, fail-once read followed by retry,
successful empty pages and catch-up waiting on its own source. Reuse existing
page harnesses and test resource bounds without new test executables.

### W6 Search selection by content identity

Owners: `iptv-search`, runtime search adapters and app search views.
Finding: search-001. Integrate with W4's coherent indexing reads.

Replace catalog-index identity with source-qualified channel identity and semantic
programme start. Keep current indexes as resolved presentation positions where
needed, but restore focus and choose playback by stable content identity. Update
channel and programme results, fallback landing, app choose/outcome handling,
multiview picker consumption and source-offset changes in combined browsing
together. Owned semantic keys may remove Copy; update builders/borrows and
resolve against the current schedule immediately before acting.

Regression gates: reorder/delete/filter/refresh a focused result; duplicate names
across sources; combined source offset changes; programme fallback on the same
semantic channel. Assert the selected/played identity, not only row position.

### W7 Native parental authority

Owner: `iptv-runtime`, with narrow store transaction support.
Findings: runtime-001/002/006. Depends on W1 and W4.

Split implementation into three reviewable changes:

1. Integrate current parental policy, membership and captured authority into
   direct inspection, paginated channel/programme/candidate/audit queries and
   affected repair admission. Filter before limits and cursor construction.
   Bind continuation to relevant authorization state, honor valid content/parent
   unlocks and return typed denial or stale continuation after revocation.
   After the last analysis/query await, synchronously recheck the accepting
   owner, grant epoch and expiry immediately before returning inspection data.
   Include an authorized SQL result followed by relock before delivery in tests.
   Merely adding a gate in front of permanently excluding legacy blocked rows is incomplete.
2. Validate original source incarnation and runtime authority inside the same
   transaction that creates/updates/enables recording intent. Design a narrow
   checked store entry point without moving parental policy into views/storage.
   A check immediately before a separate store transaction leaves an admission
   gap. Raw adapter calls remain fail-closed; trusted runtime authority must also
   permit legitimate unlocked legacy content. Do not reauthorize materialization
   or execution of jobs that were durably accepted before the UI relocked.
3. Add the final synchronous grant/owner check after the last await in direct
   VOD and recorded-media runtime results, including episode parents. Dispose
   rejected media capabilities. Preserve the app's existing final publish gate;
   no current shell decoder bypass was established by runtime-006.

Revalidate effective permission, including protection policy, current membership
and any grant actually needed to admit the target. Do not copy native_allowed's
unconditional epoch-mismatch denial: ordinary zapping also changes that epoch.
An unprotected target must remain accessible across unrelated playback changes;
expired/revoked grants must never authorize protected content. Keep continuation
invalidation distinct from rejecting a fresh query or durable request.

Regression gates: locked/adult/category/custom/source blocks, valid unlocks,
known IDs, pagination and stale cursors; writer-barrier relock before durable
acceptance; source recreation; final read/file-lookup revocation; and existing
accepted recording jobs continuing while viewing is locked. The supplied
recording update probe had a faulty title oracle and is not a passing regression.
Include unprotected inspection/recording acceptance during ordinary zapping,
and move duplicate scheduling checks inside the same writer transaction.
Add meaningful deterministic cases in the existing runtime harness.

### W8 Native player completion and notification

Owner: `iptv-player-mpv`. Findings: player-mpv-001/002.

Every refused load must complete exactly one accepted load and emit the intended
error/idle transition while preserving the native guard. Bind completion to its
request/entry identity so older loads cannot consume a newer one's accounting.
Pre-load refusal and post-Loading hook failure are different phases; a blanket
decrementing rejection note can double-account a hook failure.

When command replies are already queued, drain native events without an infinite
wait and notify the UI for the completed batch. Preserve wake coalescing and
quiet idle waiting. Do not weaken relay restrictions to make system-libmpv
baseline tests pass.

Regression gates: every early refusal, two queued loads, failure then successful
load, no upstream access on refusal, and a response-only command on a quiet
handle delivering its notification without a second command. Use deterministic
barriers; run real guarded-runtime decoder/surface checks on affected hosts.

### W9 Shell replies and playback controls

Owner: `iptv-app`. Findings: app-001/002/003/004/005.

Use separate small changes for independent behaviors:

- Send the observed oneshot reply on the queued artwork cache-hit path. A
  serialized cache-hit regression must not accidentally demand a new general
  single-flight download feature.
- Retain the guide assignment request ticket in its completion; old saves may
  complete but cannot close a newer picker.
- For repair callbacks, retain and check the existing cancellation control
  before any success/error state mutation, plus generation/open state. Audit
  preview, history, status, finish and failure paths. Release stale preview
  resources through the accepting runtime owner, not a later ambient owner.
  Introduce a broader screen-instance token only if this existing mechanism
  cannot cover a demonstrated callback path. Do not leave error or finish
  callbacks able to mutate a replacement screen.
- Share the selected programme's real description lookup with Details and
  invalidate it when guide data arrives. Preserve independent Patina wording.
- Route Stop/Long Back and sleep through one complete stop operation. Handle
  live, recording, VOD, early startup and multiview; clear early/resume intent,
  cancel or fence pending resume reads, save progress and release reservations.
  Keep native views mounted and preserve surface 0.

Regression gates: duplicate queued logos with serialized permits; close/reopen
pickers and repair screens while old work completes; distinct synthetic programme
descriptions; remote Stop and sleep across all playback variants, including an
early-resume read completing after Stop. Run targeted headless scenarios and
native host checks where lifecycle behavior changes.

### W10 Secret diagnostic formatting

Owners: `iptv-core`, `iptv-providers`, `iptv-library`, `iptv-guide`.
Findings: core-001, providers-002, library-001, guide-001.

Implement redacting Debug at every identified ownership boundary: resolved URLs,
headers, wizard events, editor/retry text and playback commands/contexts. Prefer
manual Debug over changing wire data or valid endpoint behavior. Redact header
values by default, including arbitrary credential names, and preserve explicit
wire accessors. Do not reject existing supported endpoint userinfo solely to
avoid writing a correct Debug implementation.

Regression gates: unique synthetic markers in nested containers, arbitrary header
names, URL userinfo/path/query, clipboard/password events and retained edit/retry
state; assert markers never appear in formatted output. Do not claim an observed
production log leak or change logging behavior without evidence.

### W11 Portable invariant boundaries

Owners: `iptv-core`, `iptv-settings`.
Findings: core-002/003, settings-002.

Prevent direct rows/query mutation from invalidating Membership's cached indices;
expose narrowly scoped readers/mutations and update current callers. Treat zero
menu movement as an immediate no-op or use a validated direction. For typed keys,
add checked/catalogue-backed alternatives and migrate real ad hoc constructors
before hiding fields. Preserve the supported application API behavior and avoid
an unrelated public-API redesign.

Regression gates: safe invalidation attempts, selection/filter consistency,
heading-only menu zero movement and wrong/unknown typed keys. App and runtime
callers must compile after signature changes. Keep portable wasm builds green.

### W12 Retained media transport

Owner: `iptv-net`. Findings: net-001/002.

Validate the descriptor actually opened for recording reads/resume writes. Use
platform-appropriate no-follow/nonblocking acquisition and regular-file checks
before access or truncation, including Windows reparse handling. Use opened-handle
length for alignment, resume offset and size limits, not pre-open path metadata.
The established
scope is robustness against replacement within private media storage.

Choose a 15-second progress-idle write policy for both response headers and
body chunks: reset the deadline only after bytes are actually written. A timeout
around write_all for a whole chunk is a different per-chunk completion policy;
do not describe it as a progress-reset idle timeout. Preserve healthy long
playback without one total-body limit. Stalled clients must release their four
serving slots and owner teardown must cancel active work.

Regression gates: synchronized synthetic symlink/FIFO replacement without hung
workers, resumed regular files, four stalled authenticated clients followed by a
successful range request, a healthy reader making progress for longer than
15 seconds, HEAD/error response headers, pause/reopen and cancellation. Reuse existing
network/recording harnesses. A changed Windows acquisition path requires an
explicit reparse-point replacement regression on Windows; host source review
alone cannot close that gate.

### W13 Migration ledger validation

Owner: `iptv-store-sqlite`. Finding: store-sqlite-002.

Require the exact valid migration version sequence and identity metadata before
ordinary opens proceed; reject zero/negative replacement versions and gaps.
Retain legacy valid schemas and fail closed without mutating damaged databases.

Regression gates: valid historical schemas, zero/negative versions, gaps with
unchanged count/max, missing/wrong migration names and preserved database/WAL on
failure. Add cases to the existing migration/encryption harness.

### W14 Player teardown investigation

Owner: `iptv-player-mpv`, affected native hosts. Finding: player-mpv-003.
Depends on restoring the intended runtime and completing W8.

Document the control-thread/native-handle, render-context, callback and platform
GL ownership graph. Reproduce component removal while a native control command
is pending, using an existing harness child process with a timeout. Establish
which waits are permitted by the actual libmpv/render contract and what UI
progress is required.

Only then choose a teardown protocol that retires callbacks and render contexts
in valid platform contexts and retains worker/native ownership through completion.
Do not blindly free rendering before joining, detach native ownership, or add
unbounded reaper threads. If no unsafe wait is demonstrated, record the observed join behavior and retained ownership tradeoff without
asserting a time bound that native evidence has not established.

## Implementation order and validation

Execute cohesive changes with one owner per overlapping file; review parallelism
does not imply parallel mutation of the runtime, manifests or CI plan.

| Wave | Packages | Exit condition |
|---|---|---|
| 0 | Restore reproducible baseline and define W14 experiment | Record intended NSH/native runtime/toolchain and existing failures; no guard downgrade |
| 1 | W1, W2, W3 | Portable defects have regression coverage and wasm checks; feed failures propagate safely |
| 2 | W4, W5, W6 | Coherent snapshots and stable guide/search identities pass concurrent refresh tests |
| 3 | W7, W8 | Native authority and exactly-once player completion pass controlled races and host checks |
| 4 | W9, W10, W11, W12, W13 | User controls, UI ownership, redaction and bounded adapters pass affected callers |
| 5 | W14 decision and combined integration | Lifecycle decision supported by native evidence; final affected platform gates complete |

Within Wave 2, W4 lands before the dependent W5/W6 integrations. P1 app
Stop/sleep and repair callback ownership may be pulled forward as separate W9
changes. Re-run Stop/sleep's pending-load and resume scenarios after W8 if it
lands first, since player completion accounting is shared. Independent P2
patches may move earlier when they do not conflict or obscure higher-risk fixes.
Do not wait for a broad cleanup before shipping a small confirmed correctness fix.

For every implemented finding, first add a regression that fails against the
baseline, then apply the smallest fix and inspect its callers/diff. Integration
scenarios belong in existing harness modules; reuse the same executable for
fresh process cases. Ordinary tests use --tests, doctests run separately and
Clippy checks examples with --all-targets. Run strict timing gates alone through
`tools/ci/check-plan.py`.

Before closing an implementation batch, run `bash tools/check.sh`, the required
core tests/core wasm build/app build, and affected fuzz/native/platform checks.
Passing shared-plan results can supply equivalent evidence without redundant runs.
Android changes require host checks and `tab36` and/or `tv36`, with physical-device
requirements retained. Host simulation never establishes actual decoding or
other-platform lifecycle correctness.

Final combined journeys must cover import/failure/cancellation and encrypted
publication, refresh with focus retained by ID, guide/search continuation,
source replacement during playback, PiP/multiview connection accounting,
backup/restore/restart, parental revocation, recording recovery and VOD progress.
Include the independently completed nav fixes and verify the exact final tree.

## Review record and stopping point

The initial reports were challenged by a fresh three-agent team and by Claude
CLI and agy CLI. All three source reviewers approved their corrected plan scopes
with no blocking omissions. Claude's initial review inspected source; agy's
successful initial run reviewed a supplied report packet after a source-tool
attempt produced no substantive output under its sandbox. No permission setting
was changed to obtain that review.

Both CLIs then returned APPROVE with no blocking omissions in separate final
packet reviews of this plan and its consolidation decisions. Their minor wording,
ordering and platform-gate comments were incorporated. Those final checks did
not independently re-run tests or verify source. The per-crate reports preserve
first-pass evidence; this plan's dispositions supersede conflicting sketch fixes
and priority suggestions.

| Reviewer/artifact | Scope and result |
|---|---|
| [Portable challenge](../reviews/rust/peer-review/portable-challenge.md) | Source challenge and corrected plan review; approved |
| [Ingestion challenge](../reviews/rust/peer-review/ingestion-challenge.md) | Source challenge and corrected plan review; approved |
| [Native/runtime challenge](../reviews/rust/peer-review/native-runtime-challenge.md) | Source challenge and corrected plan review; approved |
| [Claude initial](../reviews/rust/peer-review/claude-initial.md) | Source-backed challenge; disagreements reconciled |
| [agy initial](../reviews/rust/peer-review/agy-initial.md) | Report-packet challenge; suggestions reconciled |
| [Consolidation decisions](../reviews/rust/peer-review/consolidation-decisions.md) | Rationale for accepted and rejected review suggestions |
| [Claude final](../reviews/rust/peer-review/claude-final.md) | Final plan packet; APPROVE, no blockers |
| [agy final](../reviews/rust/peer-review/agy-final.md) | Final plan packet; APPROVE, no blockers |

Document checks confirmed fourteen reports, 39 unique IDs and exactly 39 matching
disposition rows, with no missing finding or broken relative link. All 991 tracked
files and the application HEAD remained unchanged. New artifacts are uncommitted
review/plan documents only.

This review task stops after plan review and document integrity checks.
Implementation checkboxes and defect dispositions stay open for the future
fix task. No source change or commit is authorized by finishing this plan.
