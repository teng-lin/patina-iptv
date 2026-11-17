# Rust review: final native and security implementation

Independent source review of the library adapters and application integration in
W4/W7/W8/W9/W12/W13/W14, against the working tree based on PR #95. The workspace
uses edition 2024 and Rust 1.99.1. No actionable correctness or security blocker
was identified in the reviewed changes. Builds, Clippy, tests and device runs
were deliberately not executed by this reviewer; the integration owner runs
those centrally. This report is source evidence, not a merge or validation pass.

## Must fix

None identified in the inspected implementation.

## Should fix

None identified in the inspected implementation.

## Consider

- The historical W8 checkpoint in `implementation/native-lifecycle.md` is now
  explicitly labeled as predating the later pending-command experiment. The
  earlier documentation ambiguity is resolved.
- W14 remains an investigated ownership risk. The new same-executable child
  experiment acknowledges a deliberately held test command and actual worker
  drop, releases it independently, and bounds the child. It does not reproduce
  an ordinary libmpv/render dependency cycle or establish a production teardown
  time bound. Retaining the production ownership order is consistent with the
  recorded evidence; do not describe this finding as a proven deadlock fixed.

## What's already good

- Runtime recording admission retains its original owner, source incarnation
  and parental snapshot. Checked store entry points acquire `BEGIN IMMEDIATE`
  before invoking that authority check, including duplicate acceptance and
  rule re-enabling; accepted recurring materialization remains independent of
  later transient UI grants.
- Inspection authorization is translated to source-qualified SQL predicates
  before paging and lookahead. Publication checks the owner and policy revision,
  then only the grants actually required by returned subjects. The optional
  known programme subject is retained even for an empty result. Ordinary
  unprotected zapping is separated from actual policy changes.
- Native load completions carry retained operation identity, distinguish early
  rejection from later guard failure, and ignore superseded/stopped completions.
  Response-only work drains native events without blocking indefinitely before
  notifying the UI; idle workers retain their ordinary blocking wait.
- Private recording access validates the acquired descriptor before reads or
  truncation. Positive partial writes reset one response-wide idle deadline;
  pending or zero writes do not extend it. Observers are scoped to the actual
  relay owner and gated from production APIs.
- Pending catch-up retries now retain content identity, source incarnation and
  provider configuration plus absolute UTC start and offset. Replacement remaps
  before retrying, and missing or recreated ownership fails closed. Stop/sleep
  cancel early resume and pending playback while leaving the persistent surface
  ownership intact. Picker and repair callbacks retain operation lifetime fences.

## Scope and limits

Read the changed runtime parental/inspection/recording/rule/VOD paths and nearby
service/callers; checked store admission, inspection filters, snapshot helpers,
migration ledger checks and affected read-then-write mutators; network private
file acquisition and recording response writes; player load/reply/lifecycle
paths and test scaffolding; application Stop/resume, pending catch-up, image
cache, picker and guide-repair completion paths. Read the W14 ownership report
and new bounded experiment, including the corrected stacked-relay decoder
observation. The latest pending-catch-up correction was included in this review.

No source edits, tool-based validation, native execution, platform SDK checks,
real-provider operations or claims about uninspected platform behavior were
made. The existing workspace lint tables remain applicable. Central exact-tree
checks, required guarded native execution, Android/device gates and external
review findings remain the integration owner's responsibility.

Selected security/native source content fingerprint (ordered path, NUL, bytes,
NUL; SHA-256): `2ccb67aefc390d00fa83e41ef8883f6abad671c16d2eabec8fa4598d837eb520`.

## Claude-finding correction recheck

A subsequent narrow source recheck covered the accepted corrections from
`claude-implementation-review.md`, including the capture-time inconsistency not
identified in this review's first pass. No remaining blocker was found in these
corrections. This recheck did not run Cargo or any tests.

- Full parental refresh now obtains blocks, ancestry, names and credential
  presence within one `read_snapshot`. Both full and targeted parent lists are
  sorted and deduplicated; comparison distinguishes known authority changes from
  an empty targeted-cache entry omitted by the full membership query. The prior
  completeness state is read before setting `complete = true`. Real known block
  or ancestry changes still advance the policy fence. The integration owner
  reports the new empty-parent regression and ten parental tests passed; that
  execution was not repeated by this reviewer.
- Removing the two legacy overlay checks is restricted to the private background
  recording resolver. Its sole production caller is `record`, reached after the
  scheduler conditionally claims durable recording intent. Enabled source,
  original incarnation, source configuration and channel-content equality remain
  checked, including after provider-link awaits; connection/capture ownership is
  unchanged. New recording admission still uses the checked transaction and
  current grant, raw entry points retain their legacy denial, and recorded-media
  playback retains the final parental check. Thus accepted recording authority
  does not become a fresh viewing or scheduling grant.
- The new tests exercise actual transport-stream bytes for an unlocked legacy
  block followed by relock and for a newly discovered occurrence of an accepted
  series after block/relock. They also assert new scheduling and recorded-media
  playback remain denied while locked. These are meaningful behavior regressions,
  not only checks of the helper that changed. Execution remains central.
- The W3 portable reservation helper derives its initial floor from the maximum
  retained row as well as `reserved_through`, including when the supplied
  historical floor is zero. Allocation remains checked and does not reuse a
  retained identity. Store publication/edit paths reserve the durable floor in
  their existing writer transaction. No additional issue was found in the
  inspected reservation changes.
