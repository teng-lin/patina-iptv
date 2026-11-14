# MCP plan review record

Date: 2026-11-14. Current roadmap: [Patina application foundation and optional MCP support](2026-11-14-mcp-plan.md). Reviewed historical candidate: [original technical design](2026-11-14-mcp-design-reference.md).

## Subsequent two-stage delivery revision

After the reviews below, the user requested a foundation stage useful without MCP and a second, deferrable MCP stage. The current roadmap now implements that split:

- Stage 1 F0–F5 follows #37/R4 and delivers source-explicit services, typed results, EPG diagnosis/repair, integration into the current UI, headless runtime tests and an explicit internal API-stability gate. It can ship without an MCP dependency, daemon or endpoint.
- Playback decoupling, bulk library editing and guide drift reporting are optional application extensions; they do not gate the foundation release.
- Stage 2 C0–C5 owns external-client mailboxes, wire IDs, client/receipt namespaces, transports, grants, optional standalone hosting and capability-specific MCP tools. It may be deferred indefinitely.
- The original detailed design is preserved byte-for-byte in the reference linked above, including the final reviewed SHA-256. Its M0–M6 sequence is historical and is superseded by the current roadmap. The technical contracts remain available for the corresponding capabilities.

The convergence verdicts below apply to that preserved original design. This later delivery split was checked for stage dependencies, links and consistency, but was not submitted to another three-reviewer round. It does not claim renewed independent sign-off.

### Subsequent API-stability and feature-roadmap update

The user then requested the plan incorporate the discussion of API stability, recording/catch-up status and missing application features. The current roadmap now includes:

- A source-grounded current-capability matrix: M3U/Xtream catch-up and archive seeking are implemented, Stalker archive support and Xtream variant probing are incomplete, and DVR/reminders/local live buffering/full VOD and parental workflows remain feature work.
- Required F0 decisions for programme identity and EPG retiming, session/capability semantics, account-wide stream allocation, durable jobs and host lifecycle, media storage and access policy.
- Required F5 declaration of internal service contract v1 for implemented foundation capabilities, with documented semantics, UI plus independent headless consumers, signature/behavior contract tests, a compatibility-change policy and honest capability states. Public MCP/wire compatibility remains a separate Stage 2 gate.
- P1–P6 application workstreams for catch-up compatibility, playback/resources, reminders/DVR, local timeshift, movies/series and parental controls. Their design impact must be considered before stabilization; full implementation does not gate the core foundation or depend on MCP.
- Explicit handling of a future recording background host independently of MCP, and reuse of such a host in C4 instead of introducing another storage owner.

This is a further user-directed revision, not a claim that the historical reviewers approved these new requirements. The preserved technical reference remains unchanged. Current source anchors, cross-references, stage boundaries and required repository checks were checked for this documentation update; no feature implementation or provider testing was performed.

The user requested independent reviews by a native agent, Claude CLI and agy CLI, followed by revisions and repeated reviews until convergence. Convergence means all three reviewers find no unresolved blocker or major design/implementation gap in the same candidate document. It does not mean the proposed implementation has been built or tested.

## Scope and method

### Subsequent F0 preparation

After checking the ongoing PR dependencies, the user authorized starting the
recommended preparation while runtime implementation waits. The delivery plan
now records the merged #37/#39 baseline, the pending #42 → #45 → #48 stack,
separate player/UI/CI dependencies and an explicit F1 entry gate. Companion
[decision notes](2026-11-14-foundation-decisions.md) and
[synthetic scenario specifications](2026-11-14-foundation-acceptance.md) make
F0 reviewable without changing active runtime branches. Final-source
reconciliation remains pending; these additions have not received another
independent three-reviewer round. Historical convergence below still applies
only to the preserved technical reference.

Preparation validation on local master `9b086515` passed: 271 core unit tests
and 2 integration tests, the core wasm build using pinned rustup 1.99.1, and
the app build. All five plan documents passed local-link, fence and whitespace
checks; the reference hash is unchanged. The scenario inventory has 26 mandatory
foundation cases and 12 future application-extension cases. These are planned
acceptance specifications, not newly executed feature tests. Application code
and Android adapters were unchanged.

### Subsequent consolidation to two PRs

The user requested minimizing Stage 1 to one or two PRs. The delivery plan now
targets two: F0–F2 with existing-UI inspection adoption, followed by F3–F5 with
the full repair UI and stable internal contract. A single combined PR is also
allowed if all gates pass before merge. All 26 mandatory scenarios and existing
validation requirements remain; no separate documentation/test/stability PR is
required. The prerequisite PRs are merged in local baseline `5904a117`. Final
F0 source reconciliation remains work inside PR 1. This delivery consolidation
has not received another independent reviewer round and does not alter the
preserved reviewed technical reference.

### Historical review method

- Native reviewer: `/root/mcp_plan_review`, with read-only access to the repository and #37 baseline. It also inspected active R4 changes without editing them.
- Claude CLI: local `claude` 2.1.325 in print/plan mode, restricted to Read/Grep/Glob, with external MCP servers disabled. It read the plan, the #37 baseline and audit fix plan. No model override was supplied.
- agy CLI: local `agy` in print/plan mode, using the complete inline plan and source-derived baseline notes. Its first attempt produced no review because headless mode denied a command permission. It was rerun with tools prohibited and completed a text-only review; no permission bypass was enabled. It did not independently inspect repository files. No model override was supplied.
- Every review round used an immutable snapshot; the parent reconciled findings against the source, edited the plan and sent the full revision to all three reviewers. Reviewers were asked to find substantive gaps, without inventing issues or requiring additional features just to prolong review.
- No reviewer changed application code, active R4 work, credentials, GitHub state or provider data.

## Round 1

Draft SHA-256: `18f766507a589e008aeaf128da59fa547acfee67fc3c8e4afba04eff4f1482e4`.

| Reviewer | Verdict | Blockers / major findings |
|---|---|---|
| Native | REVISE | No blockers; define persistent logical client identity across reconnects; fence stale import/verification publication after mutations |
| Claude CLI | REVISE | No blockers; reuse existing lock and define profile scope; schedule new persistence/revision work; isolate/cancel external reads; specify actual blocked-content policy; decouple playback from the displayed schedule |
| agy CLI | CONVERGED | No blockers or majors; suggested minor SDK, scoring, undo and endpoint clarifications |

### Dispositions incorporated into draft v2

| Finding | Disposition |
|---|---|
| Client identity and lost responses | Persistent local logical UUID, distinct from session/clientInfo, with receipt lookup by original idempotency key across bridge restart |
| Late import or verification overwrites new mappings | Capture revisions, invalidate existing work and check at transactional publication and in-memory installation; retain old effective revision honestly |
| Existing lock is process-global and not per root | Extend `backup/instance.rs`, keep its sibling lock location, use root-keyed ownership and worker lifetime retention; test multi-process and multiple synthetic roots |
| No existing user-profile feature / fixed default key item | v1 uses the app's existing default directory and broker; remove invented profile selector; defer multi-profile credential migration |
| No persistent revisions or plan/receipt tables | Explicit M1b and M3b migrations, trigger coverage for existing writers, cursor snapshots, restore rules and a queued-favorites integration requirement |
| UI reader starvation / ineffective SQL timeout | Separate runtime-owned external reader connection/lane, progress/interrupt cancellation, bounded snapshots and writer batches, concurrent audit/guide-page latency test |
| Parental-control assumptions unsupported by baseline | Define blocked-content omission and denial, hidden-content rules, no MCP unlock/PIN changes, future UI/MCP shared playback block check |
| Playback coupled to shown source | M4 owns session identity and decoupling, explicit off-screen playback and multiview target semantics; no early playback capability claim |
| Stage and preview expiry / changing live feed bytes | Retain a bounded encrypted snapshot, bind plan lifetime to stage lifetime, distinguish snapshot provenance from a requirement that a live feed never changes |
| Headless scheduler and key prompt | Periodic headless refresh off by default; noninteractive broker fails promptly when locked |
| Existing assignment helpers swallow failures | New typed batch path with distinct persistence, effective-binding and verification outcomes |
| CPU matching on owner loop / undo completion | Worker-scored snapshots and revision-checked publication; undo returns optional follow-up operation ID |
| Endpoint hardening | Private owner-verified Unix directory; Windows ACL/remote-client rejection and client impersonation limits, checked against Microsoft documentation |
| Probe dependency and egress accountability | Explicit audit G transport-fix gate, URL cap and sanitized origin audit |
| M1 too broad / pre-R4 wire names | Split M1a ownership/bootstrap from M1b facade/storage/read-lane work; schema names provisional until R4 D10 |

One agy suggestion was rejected on evidence: it suggested `rmcp` was not the official SDK crate and proposed changing the dependency name. The [official SDK manifest](https://github.com/modelcontextprotocol/rust-sdk/blob/main/crates/rmcp/Cargo.toml) declares `name = "rmcp"`. The plan now makes that relationship explicit. This factual correction was included in the next review packet.

## Round 2

Draft SHA-256: `20e3f9b92e4a36fd49a9b119786936592ec69a8838625342c808cd62bf5e4f71`.

All three reviewers received the complete revised draft and the preceding finding dispositions.

| Reviewer | Verdict | Findings |
|---|---|---|
| Native | CONVERGED | No blockers or majors; confirmed client identity, publication fences, audit G dependency and stage-expiry rules resolved |
| Claude CLI | REVISE | One major: replace-all imports and coarse revision checks can invalidate harmless refreshes, reject unrelated-edit undo, burden bulk writes and churn IDs |
| agy CLI | CONVERGED | No blockers or majors; accepted SDK correction and revised ownership/storage/playback design |

agy's remaining suggestions concern staging-key selection, exact prelude framing, blocking-worker placement and progress presentation. These are implementation details under existing milestone gates, not scope additions. The owner must already obtain its database credential before readiness, external SQL runs on its own worker lane, and protocol messages must come from the negotiated SDK contract. Its praise that network budgets “eliminate” risk is not adopted as a security guarantee.

### Dispositions incorporated into round 3

- Replaced blanket table triggers with guarded column-specific triggers for small edited tables and one semantic-change bump inside each enumerated bulk publisher/maintenance transaction. No per-programme revision trigger.
- Defined catalogue/feed-configuration digest comparisons so no-op refresh does not masquerade as a change. Visibility/organization revisions remain separate from EPG mapping changes.
- Defined generations as invalidation hints/fast paths; apply/undo validate the affected rows and evidence inside the writer transaction. Unrelated edits and no-op refresh do not reject a valid plan.
- Chose all-or-nothing undo: compare committed after-values, preserve unrelated edits, and report conflicts without mutating a subset. A new explicit inverse preview can exclude conflicting rows.
- Keyed opaque IDs by `(source_id, ContentId)` independently of replace-all rowids and cascades, retaining IDs across temporary omissions.
- Added the two regression gates explicitly: no-op refresh between preview/apply succeeds; unrelated edit between commit/undo succeeds and survives.
- Clarified native host-support crate, actual G/D02 dependency, eager/deadline-bounded reader open, nonrepeating owner epoch, app-close plan expiry, stage-to-FeedId allocation, desktop feature gating, short socket paths and offline grant configuration.
- Added explicit Linux-first acceptance and packaged Windows/macOS credential gates. No new-key fallback if a separate signed headless binary cannot access the existing credential.
- Incorporated bounded prelude framing and disposable staging encryption with a random secret key independent of the public owner epoch. Existing SQL worker/progress contracts remain.

## Round 3

Candidate SHA-256: `2718dead4bd01ce34279e6de427beea4ff95b92d64809f5bcc7859b7edc89cb5`.

The full candidate, including the round-2 corrections, was sent to all three reviewers.

| Reviewer | Verdict | Findings |
|---|---|---|
| Native | CONVERGED | No blockers or majors; confirmed affected-row CAS, stable IDs and retained asynchronous publication fences |
| Claude CLI | CONVERGED | No blockers or majors; checked revised publishers, overlays, ID registry and assignment contracts against #37 source |
| agy CLI | CONVERGED | No blockers or majors; confirmed revision/identity corrections and host/transport contracts; remaining notes are implementation checks |

All three reviewers converged in this round. Claude supplied additional nonblocking acceptance details. These were incorporated before a final same-version check: semantic cursor keys across physical no-op refresh, canonical staged SHA-256 digests, an explicit writer inventory, feed-incarnation identity despite reused numeric IDs, identity retention, low-precedence feed additions and indirect effects, explicit-versus-automatic undo conflicts, inverse retention across restart, coalesced batch imports, policy outside the restore set, and backup allowlist updates. A stage lease closes the cache-eviction/apply race mentioned by agy.

## Round 4: exact saved version

Candidate SHA-256: `2340af33ec893e82b95d5b2b0e8a77292482abe0ae35e157ccd52c7759db3de9`.

This check asks whether the incorporated nonblocking details preserve the already converged design. All three receive the complete final candidate, with no architecture or scope expansion.

| Reviewer | Verdict | Findings |
|---|---|---|
| Native | CONVERGED | Hash verified; no blockers or majors; final identity/cursor/staging/undo/restore refinements preserve the agreed guarantees |
| Claude CLI | CONVERGED | No blockers or majors; checked final contracts and spot-checked named source anchors on master, relying on earlier #37 inspection for baseline details |
| agy CLI | CONVERGED | No blockers or majors; confirmed all final acceptance refinements preserve architecture, scope and sequencing |

**Original design result: converged.** All three reviewers approved the same candidate, whose SHA-256 is recorded above and whose bytes are now preserved in the technical reference. There were no unresolved blocker or major findings at the end of that review. Three substantive review rounds were followed by this fourth exact-version check after incorporating nonblocking details. The subsequent delivery revision is recorded separately at the top of this log.

Final nonblocking implementation notes remain under the existing milestone gates: pin stage TTL as well as eviction during apply; create backup schema/trigger validation if the final baseline has no allowlist to update; interpret policy storage as per-profile in the explicitly specified sibling control directory; a token-changing feed identity may conservatively require a new preview. These do not change scope or the accepted architecture. Reconcile all source anchors with final #37/R4 before implementation, as the plan requires.

## Validation of this documentation change

- `cargo test -p iptv-core --quiet`: passed, 271 unit tests and 2 integration tests.
- `rustup run 1.99.1 cargo build -p iptv-core --target wasm32-unknown-unknown --quiet`: passed using `/home/blackmyth/.cargo/bin/rustup`.
- `cargo build -p iptv-app --quiet`: passed.
- The initial wasm invocation through the system Cargo failed because that compiler installation lacked the wasm standard library. The repository-pinned rustup toolchain has the target and passed; no toolchain or system configuration was changed.
- These checks validate the current repository baseline, not unimplemented MCP features. Android was not changed and no device run was needed for this documentation-only task.
- Markdown fence/whitespace and local-link checks passed for the original documents; the preserved reference matches the final reviewed hash. Existing untracked audit documents and active implementation worktrees were left alone.
