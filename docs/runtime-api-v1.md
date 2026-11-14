# Internal application service contract v1

Status: candidate during Stage 1 implementation. Declare this stable only after
F5's integrated acceptance and platform gates pass; a public Rust item alone
is insufficient. This document versions application semantics independently
of the package version. It describes the boundary used by Patina's UI and an
independent native test consumer. It establishes no binary ABI, external SDK
or MCP wire-schema promise.

The [delivery plan](plans/2026-11-14-mcp-plan.md),
[F0 decisions](plans/2026-11-14-foundation-decisions.md) and
[acceptance inventory](plans/2026-11-14-foundation-acceptance.md) define the
requirements behind this contract. The preserved technical reference supplies
transaction/staging details; its external-client machinery belongs to Stage 2.

## Public boundary and ownership

The supported foundation boundary consists of `iptv_runtime::Runtime`'s query,
inspection and repair methods listed below, portable value/error types in
`iptv_library::inspection` and `iptv_library::guide_repair`, and the runtime's
inspection request/control types. Native hosts construct an owner using `Ports`,
start its services, supply task/event ports where orchestration requires them,
and drain/retire that owner on shutdown. The credential port's opaque redacting
`DatabaseKey` is an explicit construction input; it is not a query record.

Keep the owner on its application thread. Local futures and `Rc` state have no
general `Send` guarantee. Work that leaves the thread carries captured services
for its accepting owner/epoch. A new owner cannot redirect old work to its
storage, keys, settings or callbacks. Tests invoke these services without an
NSH root, view, player, endpoint or daemon.

Store adapters, SQL rows/connections, staging databases, task executors and
presentation indices are implementation details. Existing helpers outside the
listed boundary remain experimental. A public type alias or inferred return
type does not permit a store record to cross the declared boundary. Views
render portable state and issue actions; focus uses content identity.

## Queries and EPG inspection

| Runtime method | Result and meaning |
|---|---|
| `inspect_channel` | One source-scoped channel or a typed missing/access/storage outcome |
| `query_channels` | Bounded semantic page matching explicit source scope and text/visibility filters |
| `query_programmes` | Bounded programme page in a supplied UTC interval, optionally for one channel |
| `epg_audit` | Coverage/diagnosis over an explicit interval; schedule coverage never proves broadcast identity |
| `epg_candidates` | Configured-feed alternatives and matching evidence without mutating automatic choices |
| `probe_feed` | Bounded inspection of a configured or supplied feed into disposable private staging |
| `probe_candidates` | A bounded page from the exact staged channel index |
| `probe_schedule` | Bounded programme samples from the exact retained snapshot for a candidate |

Requests select one source, selected sources, or all enabled sources explicitly.
There is no implicit shown-source fallback. Reading another source changes no
shown schedule, browsing focus, playback session or player surface. Disabled
explicit sources fail distinctly; all-enabled scopes omit disabled sources.
Hidden content is a visibility preference. Blocked content follows runtime
access policy and cannot be reached by bypassing search with a known ID.
Full PIN/unlock and parental-control behavior remains separately scheduled P6.

Channel identity is `(SourceId, ContentId)` within its current catalogue context.
Names, list positions and physical row IDs are not identities. Source deletion,
recreation and restore invalidate retained authority; IDs alone do not promise
continuity across those events. Source/feed incarnations distinguish reused
numeric IDs. Programme references preserve channel/feed provenance, source
incarnation, evidence revision and observed interval; provider event IDs are
optional and scoped evidence rather than global identity.

All timestamps use Unix seconds. Query intervals are half-open `[from, to)` in
UTC: a programme ending exactly at `from`, or starting exactly at `to`, is out.
Parse supplied XMLTV offsets and apply configured shifts once. Display timezone
changes cannot move stored instants. A shifted edition is distinct from a
feed-wide timing problem. Missing/contradictory evidence is reported.

Catalogue ordering is deterministic by semantic identity; programme continuation
also includes its semantic time key. Cursors bind owner, scope, filter, ordering
and relevant semantic evidence. Unchanged physical reimport preserves valid
continuation; unsafe input/data changes return `StaleCursor`. Staged continuation
is bound to the exact evidence and filter. Immutable repair-preview offsets are
positions within one fixed, digest-bound plan, not physical catalogue row IDs.

An empty successful page is different from an unreadable store, missing/disabled
source, blocked channel, invalid request, stale evidence or owner retirement.
Domain errors contain no raw SQL, response body, credential or secret-bearing
location. Error classification is part of compatibility, not just formatting.

## Bounds and cancellation

The current foundation limits are:

| Work | Limit |
|---|---|
| Query page / preview page | 200 returned items per page |
| Query text | 256 UTF-8 bytes |
| Explicit query source set | 64 sources |
| Programme/audit window | Positive interval, at most 31 days |
| Query deadline | At most 3 seconds end to end, including admission/queue time |
| Probe request | At most 60 seconds, 3 redirects, 32 MiB input, 256 MiB expanded |
| Probe XML | Finite record/text/nesting and item limits; external entities disabled |
| Staging | At most 512 MiB aggregate, including reserved work and indexes |
| Evidence / uncommitted plan lifetime | At most 24 hours; plans cannot outlive prerequisite stages |
| Repair matching metadata | At most 100k playlist rows and 100k listed guide rows; at most 64 MiB text in each snapshot |
| Repair batch | At most 500 explicit mappings and 64 feed changes/additions |
| Live preview / transient operation state | At most 32 previews, 200k total preview items and 128 operation/admitted-command identities per owner |
| Undo eligibility | Seven days after durable commit, subject to affected-row preconditions |

Callers may request a shorter budget through `InspectionControl`. Queueing,
network reads, XML parsing, SQL analysis and result publication observe finite
budgets and owner/cancellation state. Heavy analysis runs separately from guide
page reads. Cancelling or dropping one operation must not cancel sibling work
merely because a caller shares a cancellation control; ordinary completion
must not cancel the caller's token. Expired/cancelled work releases its worker,
snapshot and temporary-artifact resources.

A deadline is not a rollback acknowledgement for a committed mutation. Reads
and previews can fail without effects. Apply/undo have a durable commit boundary:
check cancellation/deadline before mutation, then preserve/report committed
results or reconcile through the same commit/operation identity after a lost
reply. `Error::CompletionUnknown { operation }` identifies such an uncertain effect.
A caller must not create a new repair to retry an uncertain old effect.
Recovery returns the durable receipt and leaves effective state pending when
post-commit publication was never observed. It does not replay the mutation or
promise background work after the caller/owner disappeared. In that case,
request a normal guide refresh (Guide sources → Update the guide now in the UI,
or the host's normal refresh path), then verify again. Durable undo remains
available subject to its affected-row preconditions and retention window.

## Repair lifecycle and completion

| Runtime method | Result and meaning |
|---|---|
| `preview_guide_repair` | Immutable plan, digest, before/after mappings/configuration, protected choices, indirect effects and expiry |
| `guide_repair_plan` | Bounded mapping/indirect-effect pages for that same plan and digest |
| `release_guide_repair_plan` | Release an abandoned uncommitted preview without changing stored choices |
| `apply_guide_repair` | Atomic durable configuration/mapping commit, effective-state outcome and verification operation |
| `undo_guide_repair` | Conditional inverse commit or a typed conflict/expiry result |
| `verify_guide_repair` | Observed bindings/coverage tied to examined, expected and current revisions |
| `guide_repair_operation` | Current transient status or reconstruction from durable commit state after restart |
| `recent_guide_repairs` | Bounded source-scoped discovery of persisted commits/inverses after restart; at most 50 |
| `cancel_guide_repair_operation` | Stop remaining transient work; committed choices remain committed |

Each plan concerns one source and one explicit independently reviewable batch.
Cross-source work uses separate explicit batches; a purportedly atomic plan
never silently applies a subset. Manual choices and intentional no-guide are
protected by default. Overriding them requires explicit opt-in for those rows.
Preview shows added feeds, settings/order changes and indirect automatic-match
effects, including effects outside the mapped batch. Incomplete impact evidence
is labelled; it is not represented as a complete preview. Indirect effects use
actual retained before-bindings. `availability_unknown` distinguishes an
unproven after-binding from a known absence; channel metadata alone cannot
prove the fallback chain has retained programmes.

A staged target carries exact staged evidence, not a guessed future feed ID.
Apply pins/validates its stage, allocates any new feed IDs in the writer
transaction, then commits feed configuration, mappings and inverse together.
New feeds enter at lowest precedence unless an explicit reviewed ordering
change says otherwise. Any affected-row/evidence failure changes nothing.

No-op refresh and unrelated edits alone cannot invalidate apply or undo.
Apply checks relevant before-values/evidence inside the transaction; undo
compares all affected committed after-values. Unrelated edits survive. Removing
an added feed conflicts if another explicit choice now uses it. An undo conflict
changes no subset. Undo restores configuration and schedules refresh as needed;
it cannot promise resurrection of replaced cached programme bytes.

Completion distinguishes durable configuration/mapping, installed effective
bindings, available programmes, pending verification and verified coverage.
A saved choice can require refresh; cache/refresh failure preserves the saved
choice and reports the incomplete effective outcome. Old import/cache/verification
work cannot overwrite a newer commit or certify a revision it never examined.
Durable commit revisions remain exact after later edits. Verification exposes
`evidence_revision` and `current_evidence_revision` in addition to configuration
and mapping revisions: a later programme or binding refresh with identical
configuration cannot certify an observation from an earlier snapshot.

Ordinary restart retains committed choices and eligible inverses, reconstructs
operation observations and expires uncommitted previews. Restore invalidates
old undo authority, cursors, previews and active work, then recomputes current
coverage. Probe caches are disposable, encrypted with a random secret for their
owning epoch, excluded from catalogue backups, and removed/invalidated when the
owner retires or stale crash artifacts are found.

## Capability and compatibility policy

`Runtime::foundation_capabilities()` returns portable capability names/states.
This describes implemented service surfaces; credentials, current owner readiness
and a particular provider's availability still require actual typed operations.
The capability/status surface must distinguish implemented behavior,
unsupported-on-this-host behavior, provider-unverified availability and features
not implemented. Stage 1 stabilizes queries/EPG inspection/repair and their
operation-status interfaces only after real consumers and contract tests pass.
The app's existing M3U/Xtream catch-up behavior remains provider-dependent;
Stalker archive support and Xtream probing are separate compatibility work.
Playback/session APIs, reminders, DVR, local timeshift, VOD and full parental
APIs are outside this contract until separately implemented and stabilized.
MCP transport, wire IDs, external grants and reconnect receipts are Stage 2.

Breaking signatures or behavioral semantics require a recorded contract
revision, migration/release notes and coordinated consumer updates. Deprecate
or forward old entry points where practical. Enum variants, public struct fields
and required trait methods are not automatically nonbreaking Rust changes.
Error meanings, identity lifetime, ordering, bounds, cursor invalidation,
completion, cancellation and publication guarantees also require review.

Before declaring v1 stable, record the integrated commit and evidence for all
FND-01–26, UI and independent headless consumers, architecture/signature checks,
portable builds, full repository checks, contention/scale budgets and affected
host/device tests. A missing, skipped or narrow test cannot establish a broader
capability. Keep platform/provider gaps explicit in release evidence.
