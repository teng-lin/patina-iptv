# Consolidation decisions

Baseline: `0e197073a272eceb0631e4d53b08d2823950c416`. All changes in this
review run are documents. Review suggestions are evidence, not automatic
permission to apply their illustrative patches.

The fresh portable, ingestion and native/runtime agents inspected source and
reviewed the consolidated plan after corrections. Claude CLI independently read
source for its initial challenge. agy CLI reviewed the supplied report packet;
its attempted source-tool review produced no substantive output because its
sandbox denied a required command. No permission setting was changed. The
successful packet review is explicitly narrower than source verification.

## Material decisions

- **Store writer contention:** retain store-sqlite-003. Claude rejected it on
  a single-process-writer premise. A Service owns its own writer
  (`store_service.rs:97`), the UI creates a Service (`lib.rs:168`), and the
  background recording host creates another (`recordings/host.rs:61`). The
  process lock does not serialize those lanes. The second native reviewer
  independently accepted this supported interleaving. Apply write intent only
  to actual mutations; keep readers deferred.
- **Parental epoch semantics:** accept Claude's correction. Recheck effective
  permission and grants actually needed, rather than blindly reusing
  native_allowed's unconditional epoch denial. Zapping must not deny unrelated
  unprotected queries/requests. Retain the team's final post-await publication
  check and same-transaction recording admission, including legacy unlocks.
- **Recording authority:** new schedule/update/repeat/enable acceptance needs
  validation. Previously accepted jobs and recurring materialization retain
  their authority after UI relock. Stop/cancel remains available.
- **Player accounting:** both CLI sketches included hook substitution failure
  in a shared refusal-decrement path. Reject that part: Loading has already
  consumed the load count in the later hook path. W8 requires phase-aware,
  exactly-once completion and request identity. Do not fix one refusal by
  corrupting accounting on a subsequent one.
- **Teardown:** accept the need for a bounded native experiment; reject a
  predetermined render/worker destruction reorder. Source proves the UI join,
  not a production deadlock or a safe alternative lifecycle. W14 is an
  investigation and ownership decision, not an approved destructor patch.
- **Guide/search:** retain exact page-owner/per-attempt identities through
  successful and failed completion; equal source/window is insufficient.
  Failed reads must complete catch-up with failure and avoid render retry
  loops. Search must resolve semantic identity immediately before Play,
  including multiview consumption, not only restore focus.
- **Guide feed IDs:** a cap alone does not account for persisted/custom/repair
  IDs. W3 coordinates allocation, header ingestion before fetching, repair
  reservations, exhaustion and old-catalog retention. A warned ingestion cap
  may preserve valid playlist content; it may not hide identity collisions.
- **URL replies:** preserve the existing raw append contract, duplicate order,
  encoding, fragments and portal path prefixes. Reject Claude's suggested
  reply-wins replacement without provider evidence and separate review.
- **Invalid seconds:** retain epg-003 against the documented parser contract
  (`iptv-epg/src/time.rs:32`): impossible calendar values return None. Seconds
  61–99 are rejected; leap-second 60 keeps its established folding. A tolerant
  parser policy would need separate compatibility evidence.
- **Numeric fixes:** validate before narrowing, check UTF-8 before byte splits,
  and widen complete arithmetic before clamping. A saturating intermediate
  multiplication alone can change a mathematically representable grid result.
  Reject silent maximum-offset rewrites for corrupt schedules.
- **Media writes:** choose an actual 15-second progress-idle policy, not a
  mislabeled whole-chunk timeout. Require progress beyond 15 seconds and a
  paused recording's reconnect/resume evidence before closing W12.
- **API/diagnostics:** migrate real ad hoc key constructors before restricting
  fields. Redact leaf and containing Debug types without refusing supported
  userinfo or changing serialization. No observed production secret log leak
  was established.
- **Repair UI:** retain captured cancellation controls plus generation/open
  checks before all callback state changes and stale-preview cleanup. Add a
  new shell instance token only if existing ownership cannot provide the gate.

## Priority and scope

The final plan uses two implementation tiers rather than mechanically adopting
every reviewer's P0–P4 scale. P1 covers incorrect operation/content identity,
lost guide state, stalled accepted controls and runtime access contracts. P2
covers bounded robustness, recoverable field/UI defects, diagnostics and narrow
availability issues. Reports preserve their original evidence; the consolidated
table is the authoritative implementation disposition.

No observed remote exploit, universal release crash, native production
deadlock, exhaustive test audit or other-platform execution is claimed. Native
decoder failures, the exact NSH revision gate and missing wasm standard library
remain baseline limitations, not successfully repaired gates.

## Review artifacts

- [Portable source challenge and final plan verdict](portable-challenge.md)
- [Ingestion source challenge and final plan verdict](ingestion-challenge.md)
- [Native/runtime source challenge and final plan verdict](native-runtime-challenge.md)
- [Claude initial source review](claude-initial.md)
- [agy initial packet review](agy-initial.md)

- [Claude final plan review](claude-final.md): APPROVE, no blocking omissions.
- [agy final plan review](agy-final.md): APPROVE, no blocking omissions.

Both final reviews were packet-only. Their nonblocking suggestions were applied
to table wording, intra-wave dependencies, pull-forward P1 app work, W4's pass
condition, Windows reparse validation, persisted-repair decision recording and
Stop/sleep revalidation after player completion changes. This clerical completion
does not approve implementation or close any defect.
