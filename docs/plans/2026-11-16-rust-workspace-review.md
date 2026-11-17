# Rust workspace review and fix plan

Created: 2026-11-16. Status: planned; baseline pending.

Review and fix all fourteen workspace crates other than `iptv-nav`, working
from portable foundations toward native adapters, runtime services and the
shell. Complete small review and fix batches with regression coverage before
moving to dependent code. Track file coverage and finding dispositions so
completion means every crate has been examined and each finding resolved.

`iptv-nav` is already being fixed by another agent. Keep its implementation
under that agent's ownership; incorporate its completed changes into final
integration validation.

## Scope and constraints

Apply the `rust-review` and `rust-best-practices` skills throughout execution.
Read [AGENTS.md](../../AGENTS.md), the [architecture](../architecture.md),
[contributor map](../contributing.md), [security contract](../security.md),
and [runtime v1 contract](../runtime-api-v1.md) before changing their boundaries.

- Use synthetic playlists, EPG, credentials and local mock servers for all
  fixtures, tests, reports and evidence.
- Keep portable crates free of NSH, platform APIs, blocking I/O and runtime
  assumptions. Preserve their wasm builds.
- Keep domain state and policy in portable models. Views render state and
  dispatch actions; restore focus through content identity.
- Preserve the persistent player surface. Mode changes affect layout;
  desktop PiP reuses surface 0 without another player or stream.
- Preserve deliberate local futures and `Rc` ownership. Require `Send` only
  where work crosses a thread boundary.
- Respect the stable internal runtime contract and distinguish it from
  experimental helpers. Justify API changes through actual callers and
  update affected callers and contract tests together.
- Preserve Patina's independent theme and design. Consult the behavioral
  reference only when needed to understand intended behavior.
- Keep any necessary NSH changes small, tested and separate in the `tv-keys`
  fork. Avoid unrelated refactors and dependency upgrades.

## Baseline and coordination

- [ ] Record the application commit, working tree state, toolchain version,
      NSH revision and host used for the baseline.
- [ ] Record the `iptv-nav` agent's changed files and integration status.
      Preserve its edits and unrelated local work. Use an isolated worktree
      if overlapping changes would otherwise be necessary.
- [ ] Run `bash tools/check.sh` once and record existing failures separately
      from regressions introduced by review fixes.
- [ ] Inventory each crate's source files, build scripts, examples, public
      APIs, features, platform branches and existing test harnesses.
- [ ] Briefly inspect native FFI, credential handling and persistence for
      urgent safety issues. A confirmed soundness, credential exposure or
      data loss defect takes priority over the normal batch order.

Execute one implementation batch at a time. Treat shared files such as
`Cargo.toml`, `Cargo.lock` and CI plans as explicitly owned changes. Recheck
the diff before editing files that another task may have changed.

## Review order and tracking

Start every row as pending. Use the states `pending`, `reviewed`, `fixing`,
`verified`, or `blocked`. A blocked row records the exact missing dependency
or evidence and the next action; it does not count as complete.

| Batch | Crate | Review focus | Status |
|---|---|---|---|
| 1 | `iptv-core` | State invariants, focus identity, reachable panics, arithmetic and time boundaries, playback and access policy | pending |
| 1 | `iptv-settings` | Schema validation, scopes, defaults, versioning and backup compatibility | pending |
| 2 | `iptv-providers` | Malformed input, stable IDs, URL construction, session behavior and credential redaction | pending |
| 2 | `iptv-epg` | Incremental XML/gzip parsing, resource bounds, timestamps, matching and schedule consistency | pending |
| 3 | `iptv-library` | Source/content identity, organization, guide assignment, repair models and selection restoration | pending |
| 3 | `iptv-guide` | Combined schedules, paging, held snapshots, indexing and playback lookup | pending |
| 3 | `iptv-search` | Normalization, deterministic results, history, index invalidation and stale selections | pending |
| 4 | `iptv-import` | Streaming bounds, cancellation, deadlines, sink failures, publication prerequisites and refresh planning | pending |
| 5 | `iptv-net` | TLS, redirects, origin scoping, relay capabilities, HLS/DASH rewriting and backpressure | pending |
| 6 | `iptv-sqlcipher-init` | Pinned FFI contract, concurrent initialization, failure handling and readiness caching | pending |
| 6 | `iptv-store-sqlite` | Encryption, migrations, atomic staging/publication, recovery, filesystem safety and bounded queries | pending |
| 7 | `iptv-player-mpv` | FFI ownership, callbacks, thread lifetime, teardown, wire compatibility and native surfaces | pending |
| 8 | `iptv-runtime` | Owner/epoch isolation, workers, shutdown, stale completion, publication and application services | pending |
| 9 | `iptv-app` | State ownership, action/event wiring, focus, host credentials/JNI, lifecycle and accessibility | pending |

The order follows production dependencies. Include development dependencies
and callers when validating a fix, even if their full review comes later.

### Units for large crates

Maintain a file checklist in each crate's report. Assign every file and
conditional platform branch to a unit. Include private tests, build scripts
and examples; complete the checklist rather than sampling representative files.

| Crate | Review units |
|---|---|
| `iptv-core` | Guide, lists and focus; playback, tracks, panels, seek, PiP and multiview; parental, recording and VOD; wizard, editors, capabilities and localization |
| `iptv-store-sqlite` | Initialization, encryption, private files and migrations; catalog, EPG and staging; organization, search, history and preferences; backup, parental, recordings and VOD |
| `iptv-player-mpv` | Props, events, tracks and wire contract; handles, worker and callbacks; Linux surfaces; macOS/iOS surfaces; Windows surfaces and shared PiP behavior |
| `iptv-runtime` | Ports, ownership, executor, workers, storage and teardown; import, guides, inspection and repair; library, settings and search; playback, opening and parental; backup, recording and VOD |
| `iptv-app` | Startup, data, platform credentials and host wiring; input, guide, lists and focus; library, search, wizard and settings; playback, seek, PiP and multiview; parental, recordings and VOD; shared views, accessibility and localization |

Split smaller crates further when a unit cannot be examined thoroughly in one
pass. Keep fixes tied to a finding or a cohesive group of related findings.

## Review and fix procedure

Repeat this sequence for each unit.

1. **Identify contracts and callers.** Classify library versus application
   surfaces, read the crate manifest and relevant tests, and record invariants.
   Use the pinned edition and Rust version when recommending changes.
2. **Run targeted tools.** Check formatting, Clippy and existing tests before
   editing. Include the feature combinations that expose relevant code.
   Use pedantic Clippy diagnostics as review input, grouping repeats and
   retaining only findings with a concrete benefit. Inspect platform branches
   that the current host cannot compile.
3. **Review in priority order.** Examine correctness and soundness, errors,
   API contracts, ownership and concurrency, then clarity, documentation and
   test gaps. For unsafe code, verify actual lifetime and threading invariants
   in addition to checking safety comments.
4. **Write findings.** Follow the skill's report structure: Must fix,
   Should fix, Consider, and What's already good. Each finding names the
   location, rule and source, triggering behavior, consequence, minimal code
   fix and relevant validation.
5. **Fix confirmed defects.** Add a failing regression test first. Cover
   observable behavior and properties using existing harnesses. Preserve
   documented quirks until their contract is understood. Resolve API and
   maintainability findings with small changes justified by current callers.
6. **Verify and inspect the diff.** Run targeted checks, review the final
   change for regressions and inspect affected callers. Record results against
   the exact revision or working tree. Update findings and file coverage.
7. **Close the batch.** Run the full host checks and required platform checks
   below, then advance the tracking rows. Make cohesive commits when committing
   is part of the execution task; exclude unrelated and other-agent changes.

Keep reports at `docs/reviews/rust/<crate>.md`, creating that directory when
execution begins. A report contains its baseline, file checklist, skill-format
findings and a resolution table. Give findings stable IDs within the crate.

| Finding | Severity | Disposition | Fix revision | Regression coverage | Validation evidence |
|---|---|---|---|---|---|
| Assigned during review | Must fix, Should fix or Consider | Open, fixed, retained with rationale or deferred with follow-up | Recorded after fix | Test or justified alternative | Command, host and result |

Fix all confirmed Must fix defects. Address Should fix findings before closing
the crate unless a specific compatibility or implementation constraint requires
a recorded follow-up. Consider findings may remain with an explicit disposition.
Do not mechanically apply API guidelines where they conflict with an intentional
application or framework contract. The workspace already has shared lints;
change them only for demonstrated gaps.

## Validation gates

### During each fix

Select targeted checks from the [contributor map](../contributing.md).
Use `--locked`, named test output and the existing test-support features where
required. Run ordinary tests with `--tests` and doctests separately. Check
examples with Clippy's `--all-targets` rather than linking them as test targets.

- Run affected crate tests and directly affected caller scenarios.
- Build each affected portable crate for `wasm32-unknown-unknown`.
- Check library documentation for broken links and changed failure contracts.
- For parsers and manifest rewriting, run the relevant existing fuzz targets
  through the selected fuzz lane. Add minimized synthetic regressions.
- For persistence, exercise real SQLite/SQLCipher with historical synthetic
  migration fixtures, restart and failure preservation; SQL mocks alone do
  not establish those properties.
- Use Miri for compatible Rust-only unsafe abstractions. Validate native FFI
  through native harnesses and actual host behavior where Miri cannot run it.

Keep private unit tests in their owning crate. Add integration scenarios as
modules in existing harnesses, normally `tests/it/` or `tests/app/`. Use child
processes of the same executable when fresh process state is needed. Preserve
the integration executable budget enforced by `tools/ci/test-check-plan.py`.

### Before closing a batch

Run `bash tools/check.sh`. It delegates Rust checks to the shared
[CI plan](../../tools/ci/check-plan.py), including workspace Clippy, ordinary
tests, separate doctests, portable wasm builds, isolated timing scenarios and
the normal app build. Run strict timing gates alone through that plan.
The required minimum from AGENTS.md remains:

```sh
cargo test --locked -p iptv-core -- --format=pretty
cargo build --locked -p iptv-core --target wasm32-unknown-unknown
cargo build --locked -p iptv-app
```

Full-plan results may supply the same evidence without rerunning equivalent
checks. Do not repeat passed checks unless the tree changes or a failure needs
investigation.

Native changes also require the affected platform lanes. Player changes need
actual decoding and surface/lifecycle evidence on each changed host, including
PiP and multiview when affected. Android changes need host unit/lint/build
checks and a run on `tab36` and/or `tv36`; follow the physical-device requirements
in [CONTRIBUTING.md](../../CONTRIBUTING.md). Distinguish emulator, headless,
decoder and physical-device evidence. Record unavailable validation as pending.

Credential broker tests use isolated test stores. In particular, follow the
security contract's isolated DBus/keyring setup for Linux broker integration.

## Final integration pass

After all batches and the independent `iptv-nav` fixes are incorporated,
validate the combined revision using synthetic end-to-end journeys:

- [ ] Import into encrypted staging, publish, then browse guide and search.
- [ ] Cancel or fail an import and retain the previous catalog and guide.
- [ ] Refresh, reorder or filter data while retaining focus by content ID.
- [ ] Replace a source, retry playback and reject stale callbacks without
      recreating the persistent surface or leaking transport credentials.
- [ ] Enter and leave PiP/multiview while preserving player ownership and
      provider connection accounting.
- [ ] Export, restore and restart; retain settings and durable user state,
      preserve encrypted production storage and recover interrupted work.
- [ ] Revoke parental access during queued work and enforce final admission.
- [ ] Recover recording jobs and VOD progress across owner retirement/restart.
- [ ] Exercise navigation through the completed `iptv-nav` changes in affected
      guide, menu, editor and player contexts.
- [ ] Run full host checks and the affected platform/fuzz lanes on the combined
      tree; attach evidence to the final revision.

## Completion criteria

- [ ] Every in-scope crate and file has a completed coverage checklist.
- [ ] All confirmed Must fix findings are fixed and regression-tested.
- [ ] Every Should fix and Consider finding has an explicit disposition;
      deferred work has a concrete reason and follow-up.
- [ ] Required host, wasm and affected platform checks pass. Missing device or
      platform evidence remains visible and prevents declaring that gate done.
- [ ] Stable runtime semantics, synthetic-data rules, portable state ownership
      and persistent player invariants remain intact.
- [ ] Final integration includes `iptv-nav` and passes on the combined tree.
