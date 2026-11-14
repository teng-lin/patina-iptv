# Foundation: synthetic scenarios and acceptance criteria

Date: 2026-11-14. Status: scenario specifications, not executed tests or feature
completion. Implements preparation for F0 in the [delivery plan](2026-11-14-mcp-plan.md)
and tests the [design decisions](2026-11-14-foundation-decisions.md).
The prerequisite runtime stack has merged. Write executable tests alongside
the implementing behavior and reuse existing regressions. PR 1 covers F0–F2
and FND-01–13 plus relevant integration checks; PR 2 covers F3–F5 and FND-14–26
while retaining PR 1 coverage. Both include their own tests and required checks;
there is no separate validation PR.

## Fixture vocabulary

Generate two playlists, `source-a` and `source-b`, with channels `Synthetic Amber`,
`Synthetic Amber +1`, `Synthetic Amber East` and `Synthetic Birch`. Include equal
display names and provider IDs across playlists to expose missing source scope.
Use generated XMLTV IDs such as `amber.main` and `amber.east`; no real listings,
station names, account details or provider endpoints.

Create feeds `feed-primary` and `feed-alternate` with a duplicate normalized
name, an exact ID, an unresolved manual override, intentional no-guide,
regional/time-shift alternatives, stale entries and a partial schedule.
Include listed-but-unmatched entries whose programme samples are not in the
normal filtered store. Feed configuration replacement can reuse a numeric feed
ID while changing its incarnation.

Use fixed clock `2026-11-14T12:00:00Z` and audit window
`[2026-11-14T12:00:00Z, 2026-11-14T14:00:00Z)`. Generate adjacent programmes
`Synthetic Segment 01`, `02` and `03`; include equivalent offset timestamps,
invalid intervals and a separate edition shifted by one hour. Use a virtual
clock for advancement and barriers for race ordering; real wall time only
measures responsiveness/deadlines. Do not use sleeps to prove causality.

Network fixtures are local mock servers with explicit test-only address policy.
Use held responses, request counters, malformed/compressed payload generators,
and controlled redirects. Fake native ports capture writes, player commands,
host acknowledgements and failures. Never relax production network policy to
enable a fixture.

## Mandatory foundation scenarios

Each row specifies the implementing gate and the observable oracle. Planned
outcomes are semantic requirements; enum/type names are settled in implementation.

| ID / gate | Trigger | Required observation |
|---|---|---|
| FND-01 / F1 | UI displays A; headless consumer queries/searches B | Only B's records returned; shown source, identity-based focus, active session and player-command count unchanged |
| FND-02 / F1 | Query identical channel IDs in A and B, plus explicit all-enabled scope | Source identity disambiguates records; disabling B affects only scopes including it |
| FND-03 / F1 | Successful empty query, missing source, disabled source, failed store read | Distinct documented outcomes; failed reads never become empty success |
| FND-04 / F1 | Reimport identical logical data between semantic pages | Traversal has no missing/repeated rows from physical row replacement |
| FND-05 / F1 | Change query/filter/scope or relevant ordering between pages | Reject mismatched/stale cursor; deterministic ties; bounded page and total work |
| FND-06 / F1 | Start work on owner A; start/close owner B with separate storage and keys | Work remains with A; no writes, credentials, callbacks or files cross owners |
| FND-07 / F1 | Cancel/deadline a held query; restore or close its owner before result | Work terminates within documented budget; old result cannot install state in the new owner |
| FND-08 / F1 | Request a blocked known ID through UI and headless paths | Both follow the documented current runtime policy; hiding alone is not represented as authorization |
| FND-09 / F2 | Audit failed, unmatched, ambiguous, disabled, stale and partial fixtures | Distinct diagnoses and interval coverage; no candidate is certified solely by coverage or list order |
| FND-10 / F2 | Compare exact ID, duplicate normalized names, region and +1 candidates | All relevant alternatives exposed with provenance/evidence; manual/no-guide choice unchanged |
| FND-11 / F2 | Inspect schedule for a listed but unmatched guide entry | Bounded samples from staged bytes; live catalogue, mapping and source order unchanged |
| FND-12 / F2 | Probe redirect/address-policy refusal, oversized XML/gzip, malformed XML, timeout/cancel | Correct typed failure, finite resource use, temporary artifacts cleaned; existing good data preserved |
| FND-13 / F2 | Compare equivalent timestamp offsets and +1 station; change display timezone | UTC intervals remain correct, shift applied once, shifted edition not collapsed into main station |
| FND-14 / F3 | Stage expires/is evicted or feed incarnation changes before apply | Stale evidence refused with zero writes; reused numeric feed ID does not revive it |
| FND-15 / F3 | Preview a multi-channel repair; conflict one affected mapping | No mappings or feed additions partially commit; conflict identifies affected items |
| FND-16 / F3 | Perform a no-op refresh between preview and apply | Apply remains valid when affected values/evidence agree |
| FND-17 / F3 | Hold old import/download/cache-discard before commit; apply repair, then release old work | Old work cannot overwrite/delete the newer mapping or publish obsolete effective state |
| FND-18 / F3 | Commit mapping; fail cached rebind or refresh; retry verification | Saved/effective/verified states remain distinct; no false success or duplicate mutation |
| FND-19 / F3 | Finish verification after a second mapping edit | Result identifies examined revision; cannot certify the second edit |
| FND-20 / F3 | Undo after unrelated edit; then repeat with an affected-row conflict | Unrelated edit survives successful undo; conflict produces zero partial undo |
| FND-21 / F3 | Restart after commit, before verification; also test restore and undo retention expiry | Committed choice survives; coverage recomputed; previews expired; restore invalidates old undo authority; expired inverse not guessed |
| FND-22 / F4 | Complete inspect/compare/preview/apply/verify/undo in existing UI and runtime-only harness | Same domain results through one facade; harness links no NSH/player and needs no view or daemon |
| FND-23 / F4 | Run bounded analysis/repair against generated 100k-channel/1k-group catalogue while paging guide | Existing guide-input and store-latency budgets hold; cancellation releases workers/snapshots; analysis memory and staging remain capped |
| FND-24 / F4 | Refresh/repair guide while synthetic playback runs | No unintended player command, duplicate stream, hidden/detached/reparented primary surface, or focus jump by list index |
| FND-25 / F5 | Compile UI and independent headless consumer against declared facade | No store adapter types, NSH objects, connection handles, hidden shown-source inputs or presentation indices leak through signatures |
| FND-26 / F5 | Change a declared signature or relevant behavior; inspect capability matrix | Contract gates detect incompatible change; capability states distinguish implemented, unsupported-host, provider-unverified and unimplemented |

For FND-23, F2 records concrete worker/page/staging limits before adding probing;
F4 measures peak use and latency under those limits. Keep existing input and
latency thresholds; increasing waits must not relax those budgets. Use real
player and Android tests for FND-24 where surface/decoder behavior matters.

## Extension scenarios reserved for P1–P6

These shape F0 decisions without blocking the foundation on future features.
They are release gates only when the corresponding capability is implemented.

| ID / workstream | Scenario and required observation |
|---|---|
| APP-01 / P1 | Advertised catch-up fails its first synthetic variant; bounded supported fallback succeeds or returns unavailable; no Stalker archive support claimed before implementation |
| APP-02 / P2 | Browse B while playing A, then deliver an old player event after a new session starts; browsing preserves A and the stale event cannot complete the replacement command |
| APP-03 / P2 | Two playlists share a generated account with one allowed stream; second independent fetch refused; PiP sharing adds no stream, multiview counts actual connections |
| APP-04 / P2 | Cancel/retry during acquisition and overlap stream replacement; no duplicate lease, premature release or oversubscription; unknown limit follows explicit policy |
| APP-05 / P3 | Persist fixed-time and follow-event intents, then correct EPG; fixed interval remains fixed, unique follow reconciliation may move, ambiguity requires review |
| APP-06 / P3 | Crash after saved scheduling intent, during media write and before finalization acknowledgement; recovery records missed/partial/failure honestly and never duplicates an attempt |
| APP-07 / P3 | App-running-only host receives closed-app request; request refused or user explicitly chooses the supported conditional guarantee; device sleep/termination tested separately |
| APP-08 / P3 | Recording competes with viewing on a shared account; explicit conflict outcome preserves current use; recording completes without a hidden renderer or decoder requirement for raw capture |
| APP-09 / P3–P4 | Destination permission loss, disk full, interrupted finalization, quota eviction and active file reader; no false completed file or deletion of in-use/unrelated media |
| APP-10 / P4 | Seek within/behind evicted live buffer, return live, change source and close session; seek bounds accurate, storage capped, leases/segments cleaned |
| APP-11 / P5 | Equal movie/episode names across sources, resume and next episode after reimport; identity remains scoped, progress survives unchanged logical import, missing content reported |
| APP-12 / P6 | PIN grant expires or is revoked between query and playback/scheduled execution; known IDs and future external clients cannot bypass policy; already-running-job policy tested explicitly |

## Evidence and execution record

For each implementation PR, record covered scenario IDs, synthetic generator/
seed and limits, test command, exact commit, observed result and remaining host
gaps. Link existing I/R4 regressions as evidence after confirming they cover the
specified behavior on the merged baseline. Failed tests need a known baseline
comparison or a fix; they cannot be converted to documented success.

Required repository checks remain those in `AGENTS.md` and the delivery plan.
API/portable/runtime checks accompany each relevant slice; actual player,
Android and platform CI evidence accompanies adapter/lifecycle changes.
Documentation preparation does not execute these future acceptance scenarios.
