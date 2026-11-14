# Foundation F0: application service decisions

Date: 2026-11-14. Status: working design decisions for Stage 1 of the
[delivery plan](2026-11-14-mcp-plan.md). These record the chosen direction;
they do not declare implemented capabilities or a stable API. Reconcile them
against the merged audit I/R4 result before F1, and resolve implementation
questions with evidence before F5. The
[acceptance scenarios](2026-11-14-foundation-acceptance.md) define observable results.

The delivery plan groups F0–F2 in PR 1 and F3–F5 in PR 2. Complete this
reconciliation within PR 1; these notes and the individual work packages do
not require separate PRs. API stability is declared with PR 2 after F5.

## 1. Ownership, scope and the first API slice

Retain the runtime owner thread and local futures. Pure state/identity/matching
logic belongs in portable crates; runtime orchestration uses injected storage,
network, clock and host ports. UI callers pass an explicit owner. Accepted work
keeps that owner's services and generation across suspension, cancellation,
worker execution, shutdown and restore. Constructing a second isolated owner
must never redirect the first one's work.

Queries specify one source, a bounded selection of sources, or an explicit
all-enabled scope. The displayed source and list indices are never implicit
inputs. All-enabled resolves to a captured source set; changes invalidate or
require revalidation of dependent cursors. Reads cannot move browsing focus,
change the shown schedule or issue player commands.

The first F1 slice is channel lookup and bounded channel search for an explicit
source, adopted by an existing UI caller and a runtime-only integration test.
Inventory existing methods after R4 and extend them instead of creating a
parallel facade. Return domain records and typed failures; adapter records such
as `ChannelRow` must not cross the declared facade, including through inferred
return types. R4's naming allowlist alone cannot enforce that guarantee.

An empty successful result differs from source missing, source disabled,
permission denied, store failure and invalid/stale cursor. Commands distinguish
admission from durable commit, effective runtime state and verification when
those milestones apply. Exact Rust names remain provisional until the slice
is implemented. Require finite result/work budgets and cancellation; F1 must
document concrete limits, ordering and deadline behavior before its merge.

Use semantic ordering and keyset cursors with a deterministic identity
tiebreaker, bound to scope, filter, sort and relevant semantic revisions.
Physical table replacement alone does not invalidate an otherwise valid cursor.
Changes that make continuation unsafe return a typed stale result; never
silently skip/repeat rows by continuing with unrelated inputs.

## 2. Content and programme identity

Use existing `(SourceId, ContentId)` domain channel identity within its owning
catalogue. `SourceId` currently identifies a stored source row; this pair is
not a globally unique reference across deletion/recreation or restore.
Outstanding references also need the catalogue/owner validity context.
An unchanged physical reimport preserves content identity. A provider change
that produces a different `ContentId` is a new identity unless a deliberate,
evidenced reconciliation exists. Never infer continuity from display names.

A programme reference includes channel identity, feed incarnation/provenance,
observed UTC interval and evidence revision. Provider event IDs, when present,
are scoped to provider/feed/channel and are supporting identity evidence;
they are not assumed globally unique, immutable or sufficient alone. Numeric
feed IDs can be reused; a changed configured feed must not satisfy an older
candidate reference merely because the number and XMLTV ID match.

Programme queries use half-open UTC intervals `[start, end)` in Unix seconds.
Preserve original timestamp-offset evidence; apply configured feed shifts
exactly once. Display timezone changes do not change the underlying instant.
Invalid or contradictory times are reported rather than silently repaired.

Future scheduling chooses one of two explicit intents: fixed UTC interval, or
follow the selected event. Fixed intent retains its interval after EPG edits.
Follow intent may update only after unique reconciliation using scoped event
identity and compatible evidence; ambiguous, missing or contradictory evidence
requires review. No title-only retargeting. Padding is separate from the event
interval, so corrections cannot accumulate padding repeatedly.

## 3. Playback sessions and capabilities

Identify a logical playback session separately from its selected content,
stream URL, rendered surface and command generation. Preserve the runtime's
owner lifetime and bind late player events to the session/generation that
issued them. A command accepted for an old session cannot certify the new one.

Describe live, provider archive, local buffer and file/VOD playback through
their actual host/content capabilities and observed seek ranges. Provider
archive metadata is advertised support; successful resolution and playback
provide stronger, time-bounded evidence. Availability can expire. Distinguish
accepted play, stream opened and observed playback; a timeout is not success.

F1 read-only inspection must preserve current viewing. Full playback independent
of browsing is P2 behavior work, requiring UI migration and player validation.
Use R4's typed player contract as an adapter contract, then introduce an
application session service when consumed. No speculative universal player API.
Recording writes media independently of rendering. Preserve the persistent
primary surface and existing PiP sharing; multiview owns additional surfaces.

## 4. Shared stream and resource allocation

Budget provider connections by a private account/resource identity shared by
playlists using the same account. It must come from provider configuration or
an explicit account association, not playlist ID, hostname alone or a public
credential hash. Never expose secrets in resource IDs, logs or diagnostics.
For ambiguous account equivalence, retain uncertainty and allow explicit
association rather than guess. Implement normalization with provider-specific
tests when P2 extracts the current viewing/multiview policy.

Represent active use as owner-bound leases: reserve before opening a stream,
release on failure/cancel/terminal close, and reject stale releases after a
replacement. Count an old stream during overlap until actually closed.
Retry must not create duplicate leases. PiP reusing surface 0 uses the same
lease; another multiview stream consumes another one. Catch-up probes also
consume their actual concurrent provider connections.

Track network connections, decoders and media/disk/CPU capacity independently.
Raw recording can need a stream and disk without a decoder. Watching plus
recording consumes their actual separate connections until shared fetching
exists and is tested. Unknown provider limits remain unknown, with an explicit
local policy; zero/unknown must not silently mean unlimited.

Default conflicts are typed refusals; never stop an existing viewer or recorder
implicitly. Any later priority/preemption policy needs an explicit product
decision and tests. DVR joins this allocator before release; it cannot introduce
an independent recording counter that overspends the account.

## 5. Transient operations, durable jobs and host lifecycle

F1–F4 query, import, probe and verification operations are bounded transient
work. They use current cancellation and publication fences. A committed repair
survives loss of its transient verification task; after restart the runtime
recomputes/reports verification for the current revision without replaying the
mutation. Durable inverse records follow F3's separate retention/restore rules.

Reminders/DVR in P3 persist intent before reporting it saved. Jobs have immutable
IDs, selected content/programme reference, intent policy, requested interval,
executing host/capability, resource requirements and outcome. Scheduling state
and execution attempts are distinct: scheduled/eligible, acquiring, running,
then completed, partial, failed, cancelled, missed or needs-review outcomes.
Each attempt records progress and reason; retries preserve job identity and
cannot append duplicate output or start duplicate recordings.

Use UTC instants for execution and persist timezone/rule information for future
recurrence. Wall-clock jumps trigger reconciliation; monotonic time measures
elapsed deadlines. Missed reminders expire; an in-progress recording window may
be offered as a partial attempt under explicit policy. Ended windows are missed,
not successful recordings. Recurrence/DST choices require a later P3 decision.

On restart, reconcile persisted attempts with actual output/host ownership;
never assume a previously running job completed. Cancellation before commit
differs from cancellation after durable commit. A committed effect is reported
even if its reply was lost. Capture real writer/host acknowledgement rather
than infer completion from optimistic UI state.

Publish support per host: app-running-only, validated background host, or
unsupported. Device sleep, app closure and OS termination have distinct tests.
An app-running-only job must expose that condition at creation; an unsupported
request for closed-app execution is refused. No background daemon is required
by the foundation. If DVR later needs one, reuse the storage ownership and native
credential ports; MCP may attach later as a separate consumer.

## 6. Media storage and recovery

Catalogue encryption does not imply media encryption. Recordings and timeshift
use separate destination permissions, quotas, retention and disclosure. Default
catalogue backups exclude recordings, live buffers and probe caches; restored
metadata cannot prove a media file still exists or is accessible.

P3/P4 reserve/check space before work and handle exhaustion during writes.
Use a private partial artifact per attempt, finalize with an acknowledged
destination operation, and retain an honest partial/failed outcome if finalization
fails. Never overwrite unrelated media. Crash recovery checks artifacts rather
than marking them complete from a filename. On destinations without atomic
rename, implement and test an explicit completion protocol before claiming it.

Completed media deletion and job cancellation are separate actions. A bounded
timeshift ring may evict unneeded segments while preserving readable active
segments; storage leases protect files in use. Exact retention defaults,
supported destinations, media formats and partial recovery rules are P3/P4
release decisions, not F1 interface placeholders.

## 7. Shared access policy

Hidden is a presentation preference; blocked is an access decision. Define
policy checks for query metadata, candidate inspection, playback, scheduling
and later recorded/VOD access. A known content ID must not bypass a denied
operation. Establish current behavior explicitly in F1; do not claim complete
parental protection until P6 has PIN, unlock/reset, expiry and lifecycle tests.

Future UI unlock is scoped to an owner/session, content scope, actions and
expiry, and invalidated on the relevant lifecycle changes. External grants
remain separate; an agent cannot inherit a UI unlock. Apply policy again at
execution when long-lived jobs run. Policy revocation prevents new authorized
work; handling an already running protected recording needs a P6 product rule
before that combination ships.

## 8. EPG evidence and transactional repair

Candidate inspection lists alternatives with evidence, without changing the
existing automatic matcher during this refactoring. Preserve manual choices
and intentional no-guide. Treat normalized name/HD equivalence as evidence,
and preserve regional, language and `+1` distinctions. Expose the existing
document-order ambiguity fallback rather than treating it as confidence.

Audit separates unavailable input, unmatched identity, ambiguous identity,
stale/partial coverage and suspicious timing. Coverage over an explicit window
cannot prove station identity. Inspect programme samples from unmatched entries
through bounded staging without adding them to the live catalogue.

F3 validates affected before-values and evidence inside the writer transaction,
commits all changes or none, and records an inverse. Revision counters are
invalidation hints: no-op refresh and unrelated edits alone must not invalidate
a valid plan. Fence old import/cache/verification publication after apply/undo,
and distinguish saved mappings from effective/verified guide results. Undo
compares affected after-values and preserves unrelated edits; conflicts cause
no partial undo. The delivery plan and preserved reference specify staging,
retention and digest details; no external-client receipt/grant machinery enters
the domain transaction.

## F0 reconciliation against the merged implementation

The merged I/R4 baseline is `5904a117`. Its runtime has explicit owner/service
handles and private store lanes; `commands::channel` already returns a neutral
`PlaybackChannel`, so no duplicate extraction is required. JNI/key brokers stay
in the shell; runtime-only tests construct `Ports` with generated private keys.
The shell still owns rendering and the single event sink. Foundation reads use
owned domain values and do not introduce a fan-out hub or a player dependency.

PR 1 extends that owner with source-scoped query/inspection methods and a lazy
analysis reader, preserving I/R4's writer and guide display lanes. Source/feed
incarnations and conditional semantic revisions are maintained in the store;
cached feed evidence also carries its publishing incarnation. Existing import
handoff/supersession remains the basis for PR 2's publication fencing.

The programme/session/job/resource/storage/policy decisions above remain the
chosen extension boundaries. Only implemented inspection operations acquire a
working public facade here; future scheduling/session interfaces are not
placeholder implementations. Current catch-up behavior is retained. The
[inspection guide](../foundation-inspection.md) records the implemented path,
its bounds and honest provider/host limits. F5 and repair validation remain
responsibilities of PR 2.

## Reconciliation and completion

The source reconciliation above completes F0 for the inspection slice. Keep
implementation/test evidence with the relevant PR, and resolve any later
discrepancy before the affected facade is stabilized. F5 alone declares internal
contract v1 for implemented foundation operations. Future P1–P6 details remain
separate decisions and cannot be advertised through capability metadata yet.
