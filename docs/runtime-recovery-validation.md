# Runtime recovery validation

The runtime's coverage lane measures executable Rust lines, not the fraction of
user journeys or devices tested. Run `bash tools/ci/coverage.sh` for the normal
library feature set, including `iptv-runtime/test-support` and the isolated large
repair scenario. It writes the summary, LCOV data and HTML under
`target/coverage/`. `tools/ci/check-coverage-floors.py` enforces each crate's floor;
raise a floor only after measuring the same lane.

## Coverage gap inventory

The Linux x86_64 baseline at `4c8a9f9`, using Rust 1.99.1 and cargo-llvm-cov 0.9.1,
covered **13,127 / 17,166 runtime lines (76.47%)**. The earlier configured 49.5%
floor was not a current measurement. No source exclusions or assertion changes
were used to obtain this baseline.

The first recovery slice measured **13,802 / 17,305 lines (79.76%)** in the
same lane; its 79.5% floor is an intermediate ratchet while the remaining
failure-path inventory is addressed. The requested target is at least 90%.

| Area | Uncovered baseline lines | User impact and validation focus |
| --- | ---: | --- |
| Commands and operations | 393 + 168 | Persisted custom groups, history, organization edits and source-scoped command admission |
| Aggregate browsing and library orchestration | 246 + 221 + 160 + 157 | Disabling/deleting a source must preserve other sources' identities and committed catalogues; stale completions must not regain presentation authority |
| Import orchestration | 236 | Cancellation between download and writer admission, source-ID reuse, transaction failure and encrypted stage cleanup |
| VOD | 208 | Opaque portal episode coordinates, bounded/repeating discovery, stale details and progress ancestry |
| Favorites | 171 | Accepted writes and content identity across reorder, owner teardown and restart |
| Backup | 151 | Public encrypted export/stage/activation, failed staging preserving an earlier accepted restore, and cleanup after refused private-directory admission |
| Recording | 141 | Detached UI ownership, duplicate claims, interrupted captures, retained bytes and shared connection budgets |
| Parental policy and settings | 80 + 52 | Revocation while work waits, failed writes preserving committed data, and independent owners |

These are line counts, not branch counts. A percentage cannot distinguish a
failed transaction from a successful one, so the scenarios below also assert
prohibited changes. The source-incarnation and cancellation checks at import
publication, and cleanup after refused backup-directory admission, have direct
failure regressions.

## Integration and end-to-end journey matrix

The paths below are relative to the repository. Test names and their assertions
are the evidence; this table does not assign a fictional E2E percentage.

| Journey | Success coverage | Failure or stale-work coverage | Restart/lifecycle coverage |
| --- | --- | --- | --- |
| Startup and combined browsing | `crates/iptv-runtime/tests/library_journeys.rs` | Source disable, re-enable and deletion retain the other source's content keys | `tests/data_startup.rs`, `tests/ownership.rs`, `tests/lifecycle_recovery.rs` under `iptv-runtime` |
| Provider import | `crates/iptv-runtime/tests/vod_import_pipeline.rs` downloads and publishes generated provider catalogues | `crates/iptv-runtime/src/imports/recovery_tests.rs` holds real response bodies and staged publication; checks cancellation, source-ID reuse and failed catalogue transactions | `crates/iptv-runtime/tests/lifecycle_recovery.rs` starts with an abandoned encrypted stage and verifies cleanup and committed source identity |
| Library commands | `crates/iptv-runtime/tests/command_journeys.rs` edits groups, organization and search history | Cross-source group targets are rejected without modifying either source | The same scenarios reopen encrypted storage and verify persisted membership and history |
| Settings and small files | `crates/iptv-runtime/tests/host_lifecycle.rs` and `tests/lifecycle_recovery.rs` | Filesystem publication failure retains committed bytes; an acknowledged blocked writer makes a zero-budget flush fail | Accepted database work survives dropped UI replies and shutdown; restarted owners read committed selections |
| Whole-app backup | `crates/iptv-runtime/tests/native_backup.rs` uses the public archive worker with encrypted stores and managed documents | Malformed staging preserves an accepted pending restore; locked administration never enters the worker; refused backup exclusion leaves destination bytes intact | Activation reencrypts documents under the destination owner and preserves source/catalogue/favorite content; `src/backup/tests.rs` covers interrupted swaps |
| Recording | `crates/iptv-runtime/src/recordings/tests.rs` verifies durable intent, actual synthetic capture, playback and shared budgets | Connection conflict, final-name failure, cancellation and reused source IDs | The native host continues after its UI owner drains/drops; scheduler publication acknowledges completion; `src/recordings/recovery_tests.rs` checks retained partial media |
| Parental admission | `crates/iptv-runtime/tests/parental_access.rs` | Direct IDs, forged membership and queued administrative revocation | Native verifier/attempt state persists; source and owner changes invalidate stale grants |
| Movies and series | `crates/iptv-runtime/tests/vod_library.rs` and `src/vod/recovery_tests.rs` | Forged detail incarnations, wrong progress parents and repeated portal pages; exact episode coordinates reach the generated create-link request | Movie and episode progress survive owner restart; stale source observations cannot overwrite successors |
| Guide inspection and repair | `crates/iptv-runtime/tests/foundation_inspection.rs` and `src/guide_repair/tests.rs` | Deadlines, cancellation, concurrent edits and publication fences | `tests/foundation_repair_fences.rs` verifies failed postcommit refresh recovery; durable inverse receipts survive restart |
| Rendered application navigation | `crates/iptv-app/tests/app/` drives the headless shell, input and native props | Existing focus, surface-lifetime, parental and stale-event scenarios | Host startup/recorder scenarios exercise UI reconstruction, independently of native decoding |
| Native playback and OS integration | `crates/iptv-player-mpv/src/mpv/tests.rs` decodes generated streams through the real libmpv render API | Transport identity, reconnect, stale-source and player teardown checks | Android lifecycle/input/background behavior additionally requires the affected `tab36` / `tv36` smoke/device run |

The shorter paths in a row refer to that row's crate. Runtime scenarios use real
temporary SQLCipher stores at persistence boundaries. Network responses, media,
credentials and identifiers are generated. Worker gates signal actual entry;
their timeouts detect a missing acknowledgement rather than establish readiness.
Dropping a test gate always releases its worker during assertion unwinding.

## Commands and limits of the evidence

Target the relevant runtime scenarios during development:

```sh
cargo test -p iptv-runtime --features test-support --lib imports::recovery_tests
cargo test -p iptv-runtime --features test-support --lib vod::recovery_tests
cargo test -p iptv-runtime --features test-support --lib recordings::tests
cargo test -p iptv-runtime --features test-support --test lifecycle_recovery
cargo test -p iptv-runtime --features test-support --test native_backup
cargo test -p iptv-runtime --features test-support --test command_journeys
cargo test -p iptv-runtime --features test-support --test library_journeys
```

Run `cargo test -p iptv-runtime --features test-support`, the affected SQLite
tests and the normal coverage lane before raising the floor. The minimum core
test, portable build and application build remain required by
[AGENTS.md](../AGENTS.md). [CONTRIBUTING.md](../CONTRIBUTING.md) defines the
physical-device checks; targeted Rust tests do not replace them.

Headless runtime integration proves service/persistence behavior. Headless app
scenarios additionally prove UI model/layout/native-prop behavior. Native mpv
tests exercise an actual decoder. Android emulator smoke exercises the APK and
Media3 host but is separate from physical TV/tablet evidence. None of these
establishes screen-reader usability, provider compatibility beyond the generated
protocol cases, sleep/wake behavior on every OS, or performance on real TV
hardware. Record each changed host's command, result and device/emulator identity
in the PR instead of inferring those results from line coverage.
