# Patina IPTV consolidated repository audit — 2026-11-16

**Overall: 7.3/10.** Strong core architecture, synthetic testing and release
infrastructure coexist with unfinished product behavior and incomplete delivery
assurance. The first priority is correcting the released-artifact provenance promise.
Player/cancellation correctness, recording state integrity, content identity and CI
selection follow.

## Scope, sources and evidence

Consolidates [Codex audit](2026-11-16-codex.md) and
[Claude audit](2026-11-16-claude.md), the two input files matching the requested date
before this document was created. Both target commit
`b289c58717fa16679a5cba53bade53300a358b0c`, workspace 0.1.4, Rust 2024 / 1.99.1.
This is an internal library/native application review, not certification of a
published general-purpose Rust API. Original audits remain unchanged.

This report deduplicates overlapping findings, retains distinct consequential
findings, and separates source-confirmed defects, reproduced defects, risks and
validation gaps. Minor style/metrics observations remain in the source reports;
they are not silently promoted into defects. References are repository-relative
`path:line` locations at the audited revision. P1 means high priority, P2 medium,
P3 follow-up; neither audit establishes a P0 exploit. C10 is a next-release
assurance gate, not a claim that every current artifact is unusable or malicious.

The synthesis was reviewed by three native reviewers and by Claude CLI and
agy CLI. Final review decisions are recorded below. No
application fixes, real-provider access, release publication or account changes are
authorized by this document task. Synthetic data only.

## Ratings and reconciliation

Scores assess implementation plus demonstrated evidence; 9–10 exceptional, 7–8.9
strong with material gaps, 5–6.9 useful but uneven. The mean below is equally weighted,
rounded half up to one decimal. It is a new assessment, not an average of incompatible
source rubrics. Source scores were Codex 7.8 and Claude 7.0; Claude's own listed ten
scores average exactly 7.0. Differences reflect both evidence scope and judgment;
the source dimensions are not interchangeable.

| Dimension | /10 | Reason |
|---|---:|---|
| Architecture | 8.0 | Real portable boundaries; index-keyed settings and large owners weaken state discipline |
| Code quality | 7.0 | Strong errors/FFI contracts; cancellation and player completion defects |
| Testing | 8.0 | Broad behavior tests, but production-key and device acceptance gaps |
| CI/CD | 7.5 | Strong aggregate design; selection and required-context gaps |
| Deployment/release | 6.5 | Mature machinery; release provenance and rehearsal evidence need reconciliation |
| Reproducibility/supply chain | 8.0 | Pins, guarded native runtime and inventories; local/environment drift |
| UX/UI and product finish | 6.5 | Original consistent design; inert settings and missing interactions |
| Accessibility | 6.5 | Semantics and layout work; incomplete actual AT validation |
| Localization | 7.0 | Extensive catalogue; fluent review and native notification coverage incomplete |
| Maintainability | 6.5 | Good contracts; orchestration, framework-fork and maintainer concentration risks |
| Documentation/onboarding | 7.5 | Detailed guides with important stale operational claims |
| Security/privacy engineering | 8.0 | Layered transport/storage protections; no penetration test or current advisory verdict |

## Validation provenance: do not combine incompatible runs

The Codex audit reports 376 core unit tests plus 3 integration tests, formatting, workspace Clippy with
`test-support`, native app build and 74 CI-helper tests passing. Its wasm build was
blocked by a missing target and unavailable rustup. Its NSH checkout differed from
`nsh.rev`; the pin guard failed correctly. These passes are local observations,
not exact pinned-release-graph validation.

The Claude audit separately reports clean-target validation: all nine portable wasm
builds, 1,848 workspace tests (12 ignored), 60 pinned-runtime player tests, 13
doctests (2 ignored), four timing gates, three allocation tests, and 356 Python tests
(2 skipped). It reports ten player timeouts with system mpv, resolved with the pinned
runtime. These are **source-audit-reported results**, not reruns or independently
verified logs during consolidation. Its default-target contamination explanation
is inferred from observed stale-artifact symptoms; do not assume every Codex pass
was false. Its sibling NSH mismatch also limits an exact pinned-commit claim. Consolidation
independently compared local NSH commit `32769658ad34ad364b960a134cc73c754d50f2d9`
and pinned commit `9ed782cbf362ab63ba18c47dc657fe944c405e50`: both tree hashes are
`6c276b4ced6f592ff159878d341a55a2ae0c14f2`. The clean source trees are identical,
which narrows source-drift concerns while leaving the commit-pin guard failure real.

Both reports' missing-target and successful-wasm observations can coexist in
different toolchain environments. Shared target-directory interference is a local
risk to investigate, not an established Cargo defect. Claude also reports hosted CI at HEAD green in run `43992652245`, including 1,867
Linux tests. This is source-reported hosted evidence, not a locally repeated run or
independent verification of all its lanes during consolidation. It is compatible
with the separate observation that the current **release** pipeline has not completed.
Neither audit ran a live UI,
physical TV, ARM release APK, desktop screen reader or credential-bearing signing
path. Offline cargo-deny results in Claude's report use a November 13 advisory
snapshot and say nothing definitive about current advisories.

## Priority order

**First: C10, a P1 release-assurance mismatch.** Correct the public version-specific
build-provenance promise and rehearse the release pipeline before the next release.
This document does not itself authorize publication or artifact changes.

**Next: confirmed P2 correctness findings F1/F2 and C1/C2/C3/C5**, then C6's
source-traced resume path and F3/C4 lifecycle/resource risks. C8/C9/C12 close
validation gaps. P2 missing features and ownership risks are less urgent than
correctness failures; unreleased iOS coverage is P3 unless its support commitment
changes. These labels do not equate a missing feature with a crash or exploit.

F IDs retain the original Codex numbering. C IDs are new consolidated identifiers
with the crosswalk below; they are not Claude’s original C-series IDs or priority labels. The detailed
“Must fix” section expands two correctness defects; C-table correctness defects
also require fixes and are not optional merely because their evidence fits a table.

## Additional high-value findings from the Claude audit

The following supplement F1–F13 below. Status labels distinguish code tracing from
runtime reproduction; fixes require regression tests in existing harnesses. Native
reviewers independently checked these paths during consolidation. Their additions
include the refusal reset boundary, relay-thread isolation, Android smoke feature
scope and sampled cancellation deadlines; these are new checks, not claims that
both source audits already agreed. Hosted observations are separately timestamped.

| ID / priority | Evidence and consequence | Remediation / acceptance |
|---|---|---|
| C1 / P2 — refused mpv load completion (source trace) | `crates/iptv-player-mpv/src/mpv/worker.rs:328`, `:349`, `:372` return only TransportError after load admission; `mpv/mod.rs:595` increments outstanding loads and `:865` records only a clue. An unpatched/system libmpv can trigger refusal; refusal leaves loading accounting unresolved until control-worker reset (`mpv/mod.rs:854` clears loads) or player recreation. Reset does not repair a missing relay-guard capability. The source audit’s system-mpv timeouts are consistent with this path, not independently proved to have this sole cause. Add a fail-fast guard preflight. | Emit exactly one terminal completion keyed to each admitted load identity (ignore stale replies), preserve error detail, and test refusal followed by successful load. |
| C2 / P2 — command notification ordering (source-confirmed scheduling defect; delay not measured) | `mpv/worker.rs:232` queues replies before `mpv_wait_event` with an infinite timeout; notification follows the wait. A quiet handle may delay replies. | Notify before blocking or use a nonblocking wait after queuing notes; prove behavior with a controlled quiet handle. |
| C3 / P2 — ignored recording transition result (source trace) | `crates/iptv-runtime/src/recordings.rs:963` discards store errors and compare-and-set failure. Finalized files can disagree with catalog state. | Distinguish retryable write errors from stale CAS results; never overwrite a legitimate newer state. Use bounded retry/backoff, idempotent terminal reconciliation and a recoverable completion marker; inject both failure modes. |
| C4 / P2 — blocking capture writes (source trace; latency unmeasured) | `crates/iptv-net/src/recording.rs:241` calls synchronous file write on the two-worker runtime used by captures and shared live ingest. PlaybackRelay HTTP serving has its own thread/runtime (`crates/iptv-net/src/playback.rs:289`); shared ingest uses tokio::spawn (`live.rs:144`). | Move writes behind a bounded writer with acknowledged drain/stop before finalization or deletion; apply bounded backpressure or fail capture explicitly (never silently discard media), and measure ingest/task responsiveness under slow storage. Simply detaching writes would violate the current cancellation safety contract. |
| C5 / P2 — picture size keyed by position (source trace) | `crates/iptv-core/src/player_options.rs:62` stores per-channel settings by usize; `crates/iptv-app/src/chooser.rs:237` supplies playing index or zero. Reorder can transfer a setting to another channel. | Use stable source/channel identity and explicit VOD scope; test reorder, removal, refresh and no-live-channel selection (which currently aliases index zero). |
| C6 / P2 — resume expressed as failure (source trace; journey not run) | Android `hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/PlayerComponent.kt:576` emits synthetic errors on foreground; `crates/iptv-core/src/playback/health.rs:266` can reject retries for not-yet-ready non-live media. | Model reauthorization separately from playback failure; test background/resume while VOD buffers and preserve retry accounting. Fresh parental authorization and stream reservation must remain mandatory; expired grants/denied capacity must fail closed. Never blindly reopen a retained URL. |
| C7 / P2 — inert settings and missing digit entry (product gap) | `crates/iptv-app/src/settings.rs:194` defaults unwired keys to NotYet; `settings_view.rs:330` displays unavailable rows. `crates/iptv-nav/src/keymap.rs:254` binds digits and `settings_view/remote_keys.rs:172` labels Channel number, but `input.rs:453` logs unimplemented actions; guide/player digit entry lacks a completed flow. | Implement privacy controls such as clear-history rather than merely hiding them; consistently hide or explain other unavailable settings in test and release builds. Implement identity-safe numeric channel selection with timeout/cancel tests. Source estimate of 86/149 inert rows is approximate, not a measured runtime inventory. |
| C8 / P2 — CI input-selection holes (source trace) | `tools/ci/validation-plan.py:53` excludes docs; `crates/iptv-core/src/capabilities.rs:177` consumes a doc with include_str. Helper path exclusions omit execution of changed lint wrappers; shellcheck/helper tests still run. | Treat test-consumed docs and executable tooling as dependencies; assert selection for each input. Aggregate fail-closed behavior does not repair an omitted lane. |
| C9 / P2 — desktop credential persistence/reopen lacks automated behavior coverage | Shared tests enable test-support; `crates/iptv-app/src/credential_key.rs:153` has a non-test-support ignored broker integration test. Production binaries are built and package-smoke-launched, and Android smoke runs without test-support, but those checks do not establish desktop database-key persistence/reopen behavior. A broker regression can make the encrypted catalog inaccessible. | Run targeted no-test-support integration inside isolated DBus and a throwaway keyring; never touch the login keyring. |
| C10 / P1 — released artifacts lack promised build provenance (independently checked) | Read-only API inspection found release-membership attestations for all 14 v0.1.4 asset digests, but no SLSA build provenance. Publication preceded the automated job's existing-release rejection. Current HEAD release pipeline had no completed run in returned recent history; see timestamped evidence below. | Verify actual assets/digests/attestations and workflow history, correct version-specific claims, rehearse next release. Do not overwrite immutable public assets or assume a cause from timing alone. |
| C11 / P2 — release changelog omission (source trace) | `CHANGELOG.md:8` has Unreleased followed by 0.1.3 at `:50`, without a 0.1.4 entry; release tooling does not require one. | Require a matching release section and artifact-specific preview/signing/provenance notes before publishing. |
| C12 / P2 — required-check migration (configuration verified; bypass inferred) | API confirms legacy names test (ubuntu-24.04), test (macos-15), android; standalone workflow_dispatch jobs emit overlapping names. `docs/ci.md:55` acknowledges migration. No merge bypass was exercised. | Fix C8 first, verify aggregate reporting on zero-lane PRs, then require a unique intended-app aggregate with a documented rollback. Verify with a controlled nonproduction PR; no bypass has been demonstrated. |

Rule basis for C1–C6: Rust Future/async completion contracts, nonblocking executor
work, explicit error handling, and the repository's content-identity rule. C7–C12
are product/validation contracts rather than Rust-style violations.

## Must fix

### F1 — P2: cancellation silently drops live waiters

**Evidence:** `crates/iptv-import/src/cancel.rs:74` evicts the oldest waker once 32
are registered. `Cancelled` at line 57 has neither a registration identity nor
unregister-on-drop behavior. The original Codex audit’s isolated executable imported this exact source with
a minimal error-type stub, retained 33 pending futures with distinct wake counters,
and cancelled the token: only 32 were woken, with the first counter remaining zero.

**Consequence:** a pending future can remain asleep indefinitely unless another
event polls it. Abandoned registrations can also displace a live one. Shared
admission tokens in `crates/iptv-runtime/src/store_service.rs:65` and `:160` make
fanout relevant, but no concrete UI journey with 33 live waiters was established.
This is a reproduced public API defect, not a reproduced application hang. Traced
inspection/repair callers have deadlines and recorder waits periodically wake; these
mitigations justify P2 application priority without repairing the API contract.

**Rule:** Rust Future wake contract; Rust best practices concurrency and behavioral
testing guidance. Returning `Pending` must arrange notification when progress is
possible.

**Fix:** assign each future a stable registration, update its waker on repoll, and
remove the registration on completion/drop. Never silently evict a live waiter.
Prefer registration cleanup that preserves the existing Future output contract.
A deliberately bounded admission API would require an explicit fallible API design
and migration; changing this future to return Result is not a drop-in fix.

```rust
pub struct Cancelled<'a> {
    token: &'a CancelToken,
    registration: Option<RegistrationId>,
}
// poll: insert/update this registration; Drop: unregister it.
// cancel: wake every live registration.
```

This is a design sketch, not a complete patch. Add 33+ live-waiter, abandoned-waiter,
waker-replacement and cancellation/registration-race scenarios to the existing
import harness; preserve the portable-core constraint.

### F2 — P2: programme Details renders production placeholder text

**Evidence:** `crates/iptv-app/src/menu_view.rs:177` opens the selected programme's
description overlay. Its body at `:486` always displays a synthetic test
description, without a demo/test guard. Actual description retrieval already
exists in `crates/iptv-app/src/header.rs:172` and
`crates/iptv-guide/src/lib.rs:510`.

**Consequence:** Details cannot show the chosen programme's EPG description, even
when the header can. The fixed English text also appears in other interface
languages. `crates/iptv-app/tests/app/cov_settings_screen.rs:26` expects the same
placeholder, and its XMLTV at `:41` supplies no description. The catalogue allowlist
at `crates/iptv-app/tests/app/catalogue_allowlist.txt:32` classifies it as demo text.

**Rule:** repository views-render-state rule; Rust best practices §11, test
observable behavior rather than reproducing implementation choices.

**Fix:** share description lookup between header and overlay; preserve provider
text and use a translated missing-description message. The intended rendering is:

```rust
// Sketch: shared helper and catalogue key must be added.
text(move || selected_description(ui).unwrap_or_else(|| tr("No description")))
```

Replace the placeholder expectation with two distinct generated `<desc>` values,
combined-source selection and missing-description cases in the existing app
harness. Remove the obsolete allowlist entry.

## Should fix

### F3 — P2: player teardown can block the UI on native work

**Evidence:** `crates/iptv-player-mpv/src/mpv/mod.rs:1098` drops the worker before
freeing the render context. `mpv/worker.rs:209` implements `Drop`, with an
unconditional thread join at `:214`; the worker executes synchronous native
commands. The blocking path is confirmed; an actual stall/deadlock was not
reproduced.

**Consequence:** closing a multiview tile or destroying its UI owner may wait
without a deadline for in-flight native work. Waking the mpv event loop does not
bound arbitrary native calls.

**Rule:** Rust best practices §5, destructors should not block; the player module's
own main-thread control-work separation at `mpv/mod.rs:49`.

**Fix:** introduce explicit shutdown and background reclamation while preserving
render callback/context/handle ordering and ownership:

```rust
worker.request_shutdown();
// Release render resources only in the platform-valid order.
reaper.enqueue(worker); // Retain dependencies until native work terminates.
```

This is a protocol sketch. Simply detaching the thread while freeing resources is
unsafe. Add a controlled blocked-worker scenario proving prompt UI return and
eventual cleanup; preserve the persistent main player surface. The process-exit
protocol must also retain native dependencies until workers stop, before native
global teardown; a reaper merely moves the wait and does not solve shutdown by itself.
Bound and expose the number of retiring workers. Track native handles and stream
reservations until confirmed termination; release reservations exactly once when
upstream use stops, never early merely because the UI disappeared.

### F4 — P2: release documentation gives obsolete merge-check instructions

**Evidence:** `docs/release.md:3`, `:38` and `:45` describe full host PR checks,
nonblocking smoke/fuzz/coverage, and coverage excluding the app and allowed to fail.
Current `.github/workflows/validation.yml:117`,
`tools/ci/validation-plan.py:139`, `tools/ci/check-plan.py:57` and
`tools/ci/check-coverage-floors.py:22` implement selected aggregate gates, reduced
native PR tiers, Linux-only wasm checks and application coverage.

**Consequence:** maintainers can configure branch protection or interpret a green
PR using the wrong validation model. This is confirmed documentation drift, not a
failure of the current aggregate implementation.

**Fix:** make `docs/ci.md` the authoritative check matrix and link it from the
release guide. Keep release-specific instructions in `docs/release.md`. Also update
`crates/iptv-app/src/player_panels.rs:5`, whose description of TV controls conflicts
with the shared primary bar documented in `docs/tv-controls.md:3`.

### F5 — P2: shipping-player and accessibility acceptance evidence is incomplete

**Evidence:** `.github/workflows/android-smoke.yml:31` tests x86_64 debug APKs;
`tools/ci/package-android.sh:12` prepares shipping ARM packages without a final
signed-package playback run. `docs/release.md:562` acknowledges native desktop
decoder evidence limits. `docs/accessibility.md:130` and `:149` explicitly leave
speech, touch exploration, physical hardware and desktop screen readers unverified.

**Consequence:** release-only, ABI, decoder, remote-input and assistive-technology
faults can survive substantial headless and emulator coverage. These are validation
gaps, not demonstrated broken platforms.

**Fix:** attach a repeatable synthetic-media acceptance record to candidate commit
and artifact SHA-256: ARM TV/tablet playback and resume, listened-to TalkBack speech,
touch exploration, physical remote input during decoded playback, and supported
desktop screen-reader journeys. Extend native playback/context-loss tests where
practical. Preserve explicit unsupported or untested labels until evidence exists.

### F6 — P2: runtime lifecycle state is a maintainability hotspot

**Evidence:** `crates/iptv-runtime/src/lib.rs:102`–`:159` holds roughly 55 separately
mutable fields across imports, settings, playback resolution, search, organization,
recordings and scheduling. Related generations, cancellation tokens, tickets and
maps are independently updated across modules.

**Consequence:** changes require understanding combinations of state that the types
do not enforce. Moving state out of views is correct; the remaining concentration
in one owner raises regression and onboarding costs. This is a design risk, not a
demonstrated invalid transition.

**Rule:** Rust best practices, make invalid states unrepresentable and keep modules
cohesive.

**Fix:** retain the public runtime facade and extract cohesive state/transition
objects incrementally, starting with imports or search, rather than splitting
crates speculatively:

```rust
struct ImportState {
    generation: u64,
    tickets: u64,
    active: HashMap<SourceId, ActiveImport>,
}
```

Methods should perform whole start/cancel/publish transitions. Existing stale-owner,
recovery and generation tests should remain the behavioral guardrails.

## Consider and track explicitly

| ID | Finding and evidence | Recommended action |
|---|---|---|
| F7 | **P2 linguistic acceptance gap:** `docs/localization.md:42` says all ten non-English locales await fluent review. No individual mistranslation was established. | Track human review by locale and critical setup/recovery/recording journey, including small-viewport long text; keep catalogue coverage separate from language quality. |
| F8 | **P3 incomplete menu actions:** `crates/iptv-core/src/menu.rs:142` adds external player, reminder and save-to-list choices; `crates/iptv-app/src/menu_view.rs:20` excludes them from implemented actions. Disabled rows give no reason. | Hide globally unimplemented choices or explain availability; reserve unexplained disabled states less broadly. This appears deliberate, not an activation defect. |
| F9 | **P3 coverage-policy membership gap:** `tools/ci/check-coverage-floors.py:8` has floors for 14 of 15 crates; `:35` ignores unlisted crates, including `iptv-sqlcipher-init`. | Check workspace membership against floors plus explicit exemptions. Existing crypto tests remain valuable; this is not an absence-of-tests finding. |
| F10 | **P3 iOS validation scope:** `tools/ci/validation-plan.py:15` has no iOS lane; `tools/ci/release-plan.py:64` includes preparation in manual all-platform mode. | If actively supporting iOS, add credential-free compile/simulator preparation for relevant changes and scheduled runs; otherwise keep its preview validation limits prominent. |
| F11 | **P3 remaining unbounded queues:** `crates/iptv-runtime/src/store_service.rs:615`, `file_writer.rs:166`, and `crates/iptv-player-mpv/src/mpv/worker.rs:201` allow queued jobs, reply registrations or commands to accumulate. Existing producer bounds materially mitigate this; no exhaustion was reproduced. | Measure queue depth/age, then coalesce superseded work and introduce explicit admission limits without blocking the UI or losing durable writes. |
| F12 | **P3 locale threading assumption:** `crates/iptv-player-mpv/src/mpv/handle.rs:44` lazily changes process-global numeric locale; absence of mpv threads alone does not establish absence of other native locale users. No race was reproduced. | Establish a host startup initialization point or document/enforce the full threading precondition. `Once` serializes only this call site. |
| F13 | **P3 environment reproducibility:** `.github/actions/setup/action.yml:32` resolves current apt/Homebrew inputs. Graph pins and provenance do not imply byte-identical historical rebuilds. | Retain native package/compiler/image inventories; use snapshot repositories or digest-pinned build images if binary reproducibility becomes a requirement. |

F11 follows Rust resource-budget and measured-performance guidance; F12 follows its
unsafe/FFI requirement to establish the complete safety precondition. Neither should
be promoted to a confirmed exploit or memory-safety defect without further evidence.

## What is already good

- **Architecture is enforced, not merely described.** The core depends only on
  `zeroize` (`crates/iptv-core/Cargo.toml:10`); transport/time are adapter concerns;
  runtime boundary tests live in `crates/iptv-runtime/tests/it/architecture.rs:9`.
  Identity-based state and persistent player ownership are explicit project rules.
- **Synthetic behavioral testing is extensive.** Historical migration tests
  (`crates/iptv-store-sqlite/tests/it/migrations.rs:560`), headless app scenarios,
  manifest-discovered fuzz targets, allocation checks and isolated latency gates
  cover far more than happy paths. Shared plans enforce test-executable budgets and
  guard against stale exact filters selecting zero tests.
- **Selected CI results fail closed.** `tools/ci/validation-plan.py:139` rejects missing,
  failed, canceled or unexpectedly skipped selected lanes. `tools/ci/check-plan.py`
  centralizes package inventory, feature selection, locked builds, doctests and wasm.
  C8 records input-selection holes; result handling cannot compensate for them.
  Configured line-coverage floors are 88–97%; these are policy thresholds, not a
  coverage measurement from this review.
- **Release machinery is carefully separated.** Builds precede credential-bearing
  signing; final Windows installers receive fresh-job verification. Linux package
  smoke covers multiple formats. Publication code uses drafts, checksums, attestations,
  a refusal to overwrite public releases and subsequent public-download verification.
These are pipeline design properties, not an assertion that admins cannot retag or
otherwise bypass the pipeline. Selected branch
  protection and release history were inspected during consolidation; signing secrets,
  complete permissions and all historical workflows were not audited. C10 records the
  distinction between implemented release safeguards and the observed v0.1.4 release.
- **Privacy defenses are deliberate.** SQLCipher storage, injected credential
  brokers, redacted structured errors, redirect/header restrictions and guarded
  playback form layered boundaries. `crates/iptv-net/src/public_resolver.rs:9`
  validates DNS answers before returning the same address set to the client.
  Portable plaintext backups are an explicitly documented tradeoff, not a hidden
  encryption guarantee. No dependency-vulnerability verdict is implied.
- **The independent UI system has useful guardrails.** Palette contrast tests at
  `crates/iptv-app/src/theme.rs:213`, shared semantic buttons at `ui_kit.rs:187` and
  measured scroll-into-view behavior at `:235` support consistent interaction.
  Capability checks are repeated when actions execute, reducing stale-UI mistakes.
- **Documentation is unusually candid.** `docs/contributing.md:14` maps ownership
  and validation; capabilities, recording limits, signing previews, localization
  quality and accessibility evidence are documented with meaningful distinctions.
  Preserve that honesty while removing contradictory duplicate instructions.


## Additional actionable follow-ups retained from peer review

| Source / priority | Reconciled finding | Next action and limit |
|---|---|---|
| Claude A3 / P2 risk | `crates/iptv-app/src/desktop/player_windows.rs:310` copies parent Ui then replaces window-local signals individually. New fields can accidentally remain shared. | Split shared application and per-window presentation state, and test isolation when new state is added. No actual unintended cross-window mutation was reproduced. |
| Claude C5 / P2 quota behavior | `crates/iptv-runtime/src/recordings.rs:1332` reserves active-count × 8 GiB, then adds existing file sizes against 32 GiB. With four reservations, any retained bytes exceed the budget. | Define whether reservations represent full maxima or remaining bytes, avoid double-counting active media, and test boundary admission. Four is a concurrency ceiling, not an unconditional capacity guarantee; clarify documentation accordingly. |
| Claude A5 / P3 risk | `crates/iptv-app/src/lib.rs:489` logs and constructs an unstarted thread-local owner on an off-thread release call. | Make owner/thread requirements explicit and reject misuse safely. No current erroneous caller was established. |
| Claude T1 / P2 local-validation risk | Source report observes shared-target stale artifacts; mechanism is not independently reproduced. | Use per-worktree target directories and verify effective target ownership/configuration before trusting runs; do not delete another session’s artifacts or assert a Cargo bug. |
| Claude D2 / P2 documentation gap | `docs/security.md` omits recording-media and parental-PIN threat boundaries covered elsewhere. | Document plaintext recording files, export boundaries, local authorization scope and recovery assumptions in one security model. |

## Additional confirmed accessibility omission

**C13 / P2 — multiview tiles lack an explicit semantic contract.**
`crates/iptv-app/src/multiview_view.rs:130`–`:153` attaches press/long-press handlers
without tile role, accessible name or selected state; selection is shown by the
border. Native peer review confirmed this source omission. Add tile labels and
selection state through the shared semantics primitives and test native traversal
and activation. This is not a claim that actual screen-reader failure was observed.

## Reconciliation of disputed or lower-confidence claims

- **Cancellation priority:** F1 is a reproduced API defect with P2 application
  priority. Traced inspection/repair calls have deadlines and recorder waits have
  periodic wakes; this mitigates production hangs without fixing lost notifications.
  No universal guarantee about all callers or actual user-visible hang is established.
- **iOS:** lack of automatic compile coverage is real; calling the entire app
  "scaffolding" is not supported by this evidence. iPad is documented as implemented
  but unreleased. Keep F10 as a support-scope decision, not a release blocker.
- **Portable backups:** plaintext exports and warnings are explicit design choices
  (`docs/security.md:46`). The export dialog (`crates/iptv-app/src/backup.rs:19`) warns about credentials
  but does not explicitly say unencrypted; add that fact. Optional encryption is a security/product enhancement,
  not an undocumented bypass of catalog encryption. Do not prescribe a custom
  cryptographic construction without a separate reviewed format design.
- **Android cleartext:** Java network security denies non-loopback cleartext;
  native Rust transport may support explicitly entered HTTP. The XML policy is not
  proof that the whole application denies HTTP. Correct the documentation's layer
  description rather than changing policy based on this audit.
- **Maintainer/fork risk:** concentrated ownership and substantial NSH coupling are
  relevant, but commit velocity does not prove poor review and a closed upstream
  request does not prove a permanently unmaintainable fork. Document maintenance,
  rebase and succession policy; avoid treating recruiting another maintainer as a
  code fix with an enforceable deadline.
- **Security follow-ups:** logo decompression bounds remain inferred. Claude S3/S6
  report source-confirmed Xtream private-address policy and an ungated development
  playlist hook; exploitability/exposure remains hypothetical and these code paths
  were not independently rechecked in consolidation. Do not turn them into confirmed
  exploits or erase the distinction between reported code facts and impact. Native HTTP stream
  warnings and recording/PIN security documentation deserve explicit follow-up.
- **Historical licensing claims:** missing assets alone do not establish a license
  violation. Claude R3 needs artifact/component/source-offer verification before a
  compliance conclusion; retain it as an investigation, not a legal finding.
- **UX measurements:** dimmed-cell contrast, text scaling, touch targets
  and overscan claims require current-state verification. `grid.rs:54` describes
  dimmed cells as unavailable historical programmes that key navigation never focuses;
  a focused-contrast defect needs a reachable transition, not just colour arithmetic. Source-level
  omissions can be actionable without asserting measured real-device usability.
- **Additional backlog:** Claude C4–C9, A3–A8, T3–T9, I4–I6, R5–R10, U4–U11 and
  S2–S8 contain distinct lower-priority risks or investigations. Their original
  evidence labels and limits remain preserved as source claims, not endorsements; they are not
  all implicitly accepted by this consolidation. The claim-level decision ledger below records
  the consequential revisions; other source claims remain explicitly unverified.

## Timestamped hosted evidence

Read-only GitHub API inspection during consolidation at **2026-11-17 02:24–02:28
UTC** (November 16 evening in New York) established:

- [v0.1.4](https://github.com/teng-lin/patina-iptv/releases/tag/v0.1.4) was latest,
  published at 2026-11-16 21:37:22 UTC. All 14 asset digest responses contained a
  `https://in-toto.io/attestation/release/v0.2` predicate and no SLSA build-provenance
  predicate. Release membership is not build-workflow provenance. Large artifacts
  were not downloaded or cryptographically verified during this pass.
- [Run 43941768397, attempt 3](https://github.com/teng-lin/patina-iptv/actions/runs/43941768397/attempts/3)
  targets `74561dce73b2eb7589c281d97cc21163df1fff72`. Its publish job rejected the
  already-public release at 21:38:01 UTC and skipped both build-provenance steps.
  The release was published outside that job's successful completion; the actor's
  exact human/tool mechanism is not proved. A separate
  [macOS release](https://github.com/teng-lin/patina-iptv/releases/tag/macos-v0.1.4)
  contains the DMG, media SBOM and checksums.
- The returned recent workflow history had no completed end-to-end release run for
  reviewed HEAD. This is a timestamped evidence gap, not proof the pipeline fails.
- Branch protection required the three legacy names in C12, bound to app ID 15368,
  with `strict=false`, no required reviews and admin enforcement enabled. The
  alternative-satisfaction risk remains inferred; no bypass or settings change was
  attempted.

No recommendation here authorizes a release, retroactive asset replacement or
branch-protection change. Correct the version-specific provenance claim, rehearse,
then publish a future version through the approved workflow when separately tasked.

## Source crosswalk and claim-level decisions

| Consolidated IDs | Original source IDs / relationship |
|---|---|
| F1–F13 | Codex F1–F13, with priority and evidence qualifications above |
| C1, C2, C6 | Claude P1, P2, P4 respectively |
| C3, C4, C5 | Claude C1, C2, A1 respectively |
| C7 | Claude U1/U2; F8 overlaps Claude U3 |
| C8, C9, C10, C11, C12, C13 | Claude I3, T2, R1/R2, R8, I2, U8 respectively |
| F2 / F3 / F4 / F6 | Also overlap Claude P3 / P5 / D1,R7 / A2,M3 |
| Additional follow-ups table | Claude A3,C5,A5,T1,D2 explicitly retain their original source IDs |

| Challenged claim | Final disposition |
|---|---|
| Missing build provenance means no attestation of any kind / hand publication proved | Reject. Release attestations exist; exact publication mechanism is unproved. Keep C10 with timestamped API evidence. |
| Aggregate fail-closed means all inputs get adequate checks | Reject. Keep result-handling strength and confirmed C8 selection omissions separately. |
| Production code is never executed | Reject generalization; retain C9 desktop credential persistence/reopen coverage gap. |
| Cancellation guarantees an application hang | Reject. Keep reproduced API loss, sampled deadline mitigations and P2 priority. |
| All playback relay work shares capture workers | Reject. Native source tracing establishes a separate HTTP-serving runtime and shared ingest scope. |
| More than half of currently visible settings are inert | Do not assert the numeric proportion without host/state enumeration; keep C7 qualitative gap, P2. |
| No iOS compile lane is P1 by itself | Keep F10 P3 under explicitly unreleased scope; raise to P2 before promising regular iOS support. Add a credential-free macOS iOS compile lane when supported, not merely a packaging check. |
| Single contributor implies permanent fork or closed upstream path | Reject categorical claim; retain P2 ownership/upgrade risk and maintenance/runbook actions. |
| Dimmed focused cells are a demonstrated contrast failure | Not established without a reachable focus transition; retain investigation. |
| Backup export silently bypasses encryption | Reject. Export is intentionally plaintext with a credential warning; add explicit unencrypted wording and consider separately reviewed encryption. |
| New consolidation facts appear in neither source | Expected where identified as native/API follow-up evidence; label them and cite source locations rather than attributing them to the inputs. |

Other source findings remain visible through this backlog inventory, without being
accepted solely because a source labels them confirmed:

| Source groups | Retained investigation / scope |
|---|---|
| Claude A4,A6–A8,C4,C6–C9 | Core coupling, shell state/order, Schedule/provider API spread, EPG anchoring, polling/shutdown budgets, error-cause loss, retry/hash/private-symbol risks. Verify before refactoring. |
| Claude T3–T9,I4–I6 | Crash injection, Android adapter tests, fuzz oracles/sanitizers, real-time megatests, fixture cleanup, nightly alerting, dependency-update failures and helper maintenance. Full coverage runs are selected nightly/release or infrastructure changes, not every ordinary code PR. Current hosted failure/update statistics were not rechecked. |
| Claude R3,R5,R6,R9,R10 | Historical corresponding-source offers, update discovery, signing preview selection, historical retagging, upgrade/yank procedures. No legal or credential-policy conclusion is made from source reports alone; separate macOS release assets were not included in the 14-asset attestation check. |
| Claude P6–P8,U4–U7,U9–U11 | Windows render cost, ambient view state, Kotlin/native-notification polish, contrast/scaling/targets/overscan, screenshots and preferences. Current device behavior remains unmeasured. |
| Claude D3–D6,M2–M5,S2–S8 | Documentation organization, onboarding, rustdoc, process weight, dependency hygiene, network/image/dev-hook risks, privacy disclosure and advisory/fuzz scope. Counts and historical claims remain source-reported unless specifically corroborated above. |

## Review decisions

| Reviewer | Review performed | Disposition in this revision |
|---|---|---|
| Native architecture/code reviewer | Reconciled both inputs against current source, then reviewed the consolidated draft | Accepted load-completion, CAS and identity findings; corrected relay threading; retained shutdown ownership and resume authorization requirements; narrowed cancellation to P2 application impact. |
| Native testing/delivery reviewer | Local selector checks and read-only hosted API inspection, followed by draft review | Confirmed selector omissions, legacy protection and missing build provenance; distinguished release attestations, production smoke execution and exact API observation time; corrected validation provenance. |
| Native UX/docs reviewer | Source reconciliation and draft review | Qualified settings counts, missing digits and fork claims; promoted multiview semantics; distinguished credential warning from explicit unencrypted-export disclosure; rejected unreachable focused-contrast conclusions. |
| agy CLI | Completed text-only review of the full draft plus both input audits | Accepted priority/arithmetic clarity, reaper exit protocol, capture backpressure and explicit retained follow-ups. Source-verification questions were resolved by native reviewers/coordinator; claims that several citations were absent from both inputs were rejected because the Codex input contains them. |
| Claude CLI | Completed text-only review of the revised draft plus both source audits; verdict “accept with revisions” | Added source crosswalk and claim ledger, corrected sequence/provenance wording, bounded completion/retry/reaper remedies, and expanded attributed backlog. Questions already resolved by native/API checks remain qualified rather than treated as newly reproduced. |

agy's first source-reading attempt could not run a command in noninteractive plan
mode. It was rerun successfully with all three documents inline and explicit
instructions to use no tools. Claude’s initial Read/Glob/Grep invocation exited
without a written review; a no-tools retry with all documents inline returned a
successful written JSON result. Both completed CLI reviews therefore assess
synthesis quality, not independent code execution. The native team supplied source
and hosted-state verification. No reviewer was authorized to edit application code
or perform writes to services.
The coordinator adjudicates review suggestions; completion does not imply unanimous
approval or independent reproduction of every finding.

No Cargo suite was rerun for this documentation-only consolidation. Earlier runs
remain attributed above. Direct follow-up checks were source inspection, CI-selector
invocation, hosted metadata inspection and comparison of NSH tree identities. Final
document checks cover score arithmetic, Markdown structure, whitespace and source
report preservation.

## Recommended sequence

1. **C10/C11:** correct v0.1.4-specific provenance wording, prepare the changelog
   gate and rehearse before the next release. Preserve the future build-provenance
   commitment and immutable release artifacts; publication is a separate task.
2. **F1/F2, C1/C2/C3/C5/C6:** fix cancellation, programme Details, load completion
   and notification ordering, recording terminal state, picture identity and the
   authorization-preserving resume path. Add regressions in existing harnesses.
3. **C8 before C12, plus C9/F4:** fix selector dependencies before required-check
   migration; validate zero-lane aggregate reporting and rollback, exercise isolated
   credential persistence, and align release/CI documentation.
4. **F3/C4:** bound native retirement and move capture writes with backpressure and
   acknowledged shutdown. Test slow/blocked work without freeing resources early.
5. **C7/C13, F5/F7/F8:** implement or accurately expose product capabilities,
   including privacy controls; add multiview semantics and device/AT/language
   acceptance evidence. Keep test and release visibility behavior consistent.
6. **F6 and remaining follow-ups:** reduce state coupling incrementally, correct
   quota accounting, measure queues, and document fork ownership and support scope.
7. Run the shared check plan with pinned dependencies, isolated target directories
   and the required wasm target; perform native journeys for affected platform fixes.
