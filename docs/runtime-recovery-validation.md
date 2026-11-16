# Runtime recovery validation

The runtime's coverage lane measures executable Rust lines, not the fraction of
user journeys or devices tested. Run `bash tools/ci/coverage.sh` for every
workspace crate, including `iptv-runtime/test-support`, `iptv-app/test-support`
and the isolated large repair scenario. The headless application suite exercises
the shell's real runtime handoffs and isolated child processes. These tests share
one checkout and one instrumented build with the library scenarios; reports from
other revisions or worktrees are not combined. It writes the summary, LCOV data and HTML under
`target/coverage/`. `tools/ci/check-coverage-floors.py` enforces each crate's floor;
raise a floor only after measuring the same lane.

## Coverage gap inventory

The Linux x86_64 baseline at `4c8a9f9`, using Rust 1.99.1 and cargo-llvm-cov 0.9.1,
covered **13,127 / 17,166 runtime lines (76.47%)**. The earlier configured 49.5%
floor was not a current measurement. No source exclusions or assertion changes
were used to obtain this baseline.

The first recovery slice measured **13,802 / 17,305 lines (79.76%)** in the
library-only lane. The expanded service, headless-shell and native GTK lane at
`7a506cc`
measured **16,093 / 17,476 runtime lines (92.09%)** and
**23,168 / 25,409 application-source lines (91.18%)**. Both crates now enforce a
90% floor. The broader application build also exposed native GTK/X11 player
lines absent from the earlier unit-only denominator. The actual GTK/GL scenario
and compositor failure tests raised the combined player result to
**3,705 / 3,855 lines (96.11%)**, preserving its existing 94% floor.

After integration with the shared capability model, a fresh complete Linux lane
at `12f06a3` measured **16,124 / 17,507 runtime lines (92.10%)**,
**23,183 / 25,461 application-source lines (91.05%)** and
**3,706 / 3,855 player lines (96.13%)**. Every measured crate met its floor.
The later repair-test fixture adjustment only acknowledges debounce registration
before releasing its injected timer; it retains the ten-second watchdog and
all domain assertions. Five fresh-process repeats passed on both Linux and macOS.

The lane changes are explicit: service-only, rendered headless application and
native GTK scenarios exercise different boundaries. These measurements include
all of their instrumented source lines, with the existing source-filter rules
unchanged. No foreign worktree profiles or earlier revision reports are merged.

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
| Provider import | `crates/iptv-runtime/tests/vod_import_pipeline.rs` downloads and publishes generated provider catalogues | `crates/iptv-runtime/src/imports/recovery_tests.rs` holds real response bodies and staged publication; checks cancellation, source-ID reuse, configuration changes before/after worker start, retired owners, empty responses and failed catalogue transactions; a stale queued refresh opens no HTTP connection | `crates/iptv-runtime/tests/lifecycle_recovery.rs` starts with an abandoned encrypted stage and verifies cleanup and committed source identity |
| Library commands | `crates/iptv-runtime/tests/command_journeys.rs` edits groups, organization and search history | Cross-source group targets are rejected without modifying either source | The same scenarios reopen encrypted storage and verify persisted membership and history |
| Settings and small files | `crates/iptv-runtime/tests/host_lifecycle.rs` and `tests/lifecycle_recovery.rs` | Filesystem publication failure retains committed bytes; an acknowledged blocked writer makes a zero-budget flush fail | Accepted database work survives dropped UI replies and shutdown; implicit last-owner disposal acknowledges connection closure before returning and restarted owners read committed selections |
| Whole-app backup | `crates/iptv-runtime/tests/native_backup.rs` uses the public archive worker with encrypted stores and managed documents | Malformed staging preserves an accepted pending restore; locked administration never enters the worker; refused backup exclusion leaves destination bytes intact | Activation reencrypts documents under the destination owner and preserves source/catalogue/favorite content; `src/backup/tests.rs` covers interrupted swaps |
| Recording | `crates/iptv-runtime/src/recordings/tests.rs` verifies durable intent, actual synthetic capture, playback and shared budgets | Connection conflict, final-name failure, cancellation and reused source IDs | The native host continues after its UI owner drains/drops; scheduler publication acknowledges completion; `src/recordings/recovery_tests.rs` checks retained partial media |
| Parental admission | `crates/iptv-runtime/tests/parental_access.rs` and `tests/parental_recovery.rs` | Direct IDs, forged membership, queued administrative revocation and a rejected recovery code leave committed policy intact | PIN/recovery rotation invalidates old credentials and content grants; native verifier and policy survive restart; content/session/background scope controls revoke grants |
| Cached guide and manual matching | `crates/iptv-runtime/tests/guide_journeys.rs` | Bounded pool refusal leaves channel identity intact; no-guide and explicit-channel decisions rebind without stale programmes | Cached startup and older-day pages use stable content keys, descriptions arrive through acknowledged events, and retired reads return empty results |
| Movies and series | `crates/iptv-runtime/tests/vod_library.rs` and `src/vod/recovery_tests.rs` | Forged detail incarnations, wrong progress parents and repeated portal pages; exact episode coordinates reach the generated create-link request | Movie and episode progress survive owner restart; stale source observations cannot overwrite successors |
| Guide inspection and repair | `crates/iptv-runtime/tests/foundation_inspection.rs` and `src/guide_repair/tests.rs` | Deadlines, cancellation, concurrent edits and publication fences | `tests/foundation_repair_fences.rs` verifies failed postcommit refresh recovery; durable inverse receipts survive restart |
| Rendered application navigation | `crates/iptv-app/tests/app/` drives the headless shell, input and native props | Existing focus, surface-lifetime, parental and stale-event scenarios | Host startup/recorder scenarios exercise UI reconstruction, independently of native decoding |
| Native playback and OS integration | `crates/iptv-player-mpv/src/mpv/tests.rs` decodes generated streams through the real libmpv render API; `crates/iptv-app/tests/native_player.rs` runs real NSH/GTK GL surfaces on a private Xvfb display | PiP shares the original context/player without an extra HTTP request; local UnixStream compositor fixtures test malformed input, refusal, timeouts, cached placement and fallback dispatch | Real native context loss resumes decoding; explicit source stop and GTK teardown close the surfaces. Android lifecycle/input/background behavior additionally requires the affected `tab36` / `tv36` smoke/device run |

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
cargo test -p iptv-runtime --features test-support --test guide_journeys
cargo test -p iptv-runtime --features test-support --test parental_recovery
python3 tools/ci/run-xvfb.py cargo test -p iptv-app --features test-support --test native_player -- --ignored
```

Run `cargo test -p iptv-runtime --features test-support`, the affected SQLite
tests and the normal coverage lane before raising the floor. The minimum core
test, portable build and application build remain required by
[AGENTS.md](../AGENTS.md). [CONTRIBUTING.md](../CONTRIBUTING.md) defines the
physical-device checks; targeted Rust tests do not replace them.

The full default lane additionally needs `Xvfb` and a software OpenGL driver on
Linux; the coverage workflow installs both. The private display's readiness
comes from Xvfb's `-displayfd` acknowledgement. Its wrapper clears the Wayland and
Hyprland instance selectors, disables TCP listeners, and terminates that display
in `finally`. It never uses or rearranges the user's desktop. The native test is
ignored in ordinary display-independent runs and explicitly mandatory in this
coverage lane; missing GTK/GL/ffmpeg prerequisites fail it.

Headless runtime integration proves service/persistence behavior. Headless app
scenarios additionally prove UI model/layout/native-prop behavior. Native mpv
tests exercise an actual decoder. Android emulator smoke exercises the APK and
Media3 host but is separate from physical TV/tablet evidence. None of these
establishes screen-reader usability, provider compatibility beyond the generated
protocol cases, sleep/wake behavior on every OS, or performance on real TV
hardware. Record each changed host's command, result and device/emulator identity
in the PR instead of inferring those results from line coverage.

Three fresh processes each repeated the import failure/cancellation group, VOD
identity/paging group, independent background capture, implicit-owner persistence
group and held-broker closure scenario: fifteen subprocess runs passed. Stable
copies of the built executables prevented shared build-cache replacement during
these runs. This evidence is separate from the fresh default coverage run.

## Safety mutation evidence

Fourteen deliberate safety removals were checked in fresh test processes during
this change. Each compiled, ran exactly its selected scenario and failed an
invariant assertion; the original production source was restored after every
run. This distinguishes a missing safety check from a test that merely executes
its branch.

| Deliberately removed behavior | Failing scenario |
| --- | --- |
| Cancellation fence at catalogue publication | `imports::recovery_tests::cancellation_after_staging_cannot_publish_or_mark_a_failed_refresh` |
| Original source incarnation check | `imports::recovery_tests::an_old_staged_completion_cannot_replace_a_recreated_source_with_the_same_config` |
| Atomic source edit/catalogue transaction (source edit moved before transaction) | `imports::recovery_tests::a_failed_catalogue_transaction_preserves_source_details_and_survives_restart` |
| File-flush acknowledgement (zero-budget flush reports success) | `host_flush_acknowledges_the_actual_file_worker_and_dropped_reply` |
| Abandoned catalogue-stage startup cleanup | `restart_cleans_interrupted_catalogue_work_without_touching_committed_content` |
| Refused backup scratch cleanup | `failed_private_scratch_admission_preserves_the_destination_and_cleans_its_directory` |
| Pending restore document encryption | `native_backup_round_trip_reencrypts_documents_and_preserves_other_owner_events` |
| VOD detail incarnation check | `vod::recovery_tests::embedded_stalker_episodes_keep_coordinates_and_reject_forged_or_stale_details` |
| Background recording ownership (UI detach unconditionally stops host) | `recordings::tests::native_host_captures_after_its_ui_owner_is_drained_and_dropped` |
| Implicit UI-owner connection drain | `dropping_the_last_ui_owner_acknowledges_closed_lanes_and_keeps_accepted_writes` |
| Immediate owner-closure notification before draining a held credential broker | `closing_an_owner_with_a_held_open_rejects_its_late_query` |
| Accepted-write epoch permission after admission closes (retire the epoch early) | `shutdown_drains_accepted_database_work_even_when_its_reply_is_dropped` |
| One store-drain budget per explicit shutdown attempt | `tests::a_failed_explicit_drain_does_not_spend_its_budget_again_in_drop` |
| Prompt owner-closure wake (remove polling the closure future) | `closing_an_owner_with_a_held_open_rejects_its_late_query` fails its response budget before the normal query deadline |

The last-owner drain addresses a reproduced native exit race: a store reader
was closing its connection in `sqlite3FreeCodecArg` after SQLCipher's process
cleanup had freed its private heap. Explicit host shutdown remains the primary
path. Runtime disposal also attempts to drain its UI-owned native lanes (up to
ten seconds per lane) and flush accepted small-file writes (up to two seconds)
before retiring services. It does not repeat the store-drain budget after an
explicit attempt, including one that timed out. A failed acknowledgement is logged without paths or
credentials; a timed-out or noncooperative worker is not guaranteed closed.
`store_service::tests::a_timed_out_drain_does_not_acknowledge_connection_closure`
holds the real writer, verifies a zero-budget drain reports failure, then releases
it and observes successful closure. The independent recording host keeps its own
services and continues after UI disposal. Explicit desktop Quit and window-close
requests both require successful recorder shutdown before closing; a refusal
keeps the window open and displays the existing localized retry notice.

Closure has two phases. An epoch-bound token immediately closes admission and
wakes owner-bound inspection queries. Already-admitted FIFO writer jobs retain
their epoch until the writer barrier closes it, so abandoned replies do not
discard accepted persistence. The held-credential regression releases its broker
only after the query returns `OwnerClosed`; neither a sleep nor an elapsed drain
timeout establishes readiness. Held-write regressions release the writer only
after a real admission-close acknowledgement, so queued writes cannot finish
before the boundary being checked. Superseded import authority reports
cancellation, while a real transaction failure still reports a storage error.
A restart installs a new token and epoch, so
old handles remain closed when the same service owner starts again.
