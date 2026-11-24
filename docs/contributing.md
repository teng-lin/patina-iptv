# Changing Patina

Start with [AGENTS.md](../AGENTS.md) for repository-wide rules and
[architecture](architecture.md) for dependency direction. This map locates
the owner and the closest validation for a change; it does not replace the
full checks before finishing or the release gates.
Setup, contribution policy and physical-device validation requirements remain
in [CONTRIBUTING.md](../CONTRIBUTING.md).

Validation scheduling and required-check behavior are documented in [CI tiers](ci.md).
Open pull requests as drafts while pushing fixups (`gh pr create --draft`).
Once the change is complete, mark it ready (`gh pr ready`) to start the complete
validation run. A draft run always fails its gate on purpose and lists the
heavy lanes it deferred.

The [documentation map](index.md), [NSH pin](nsh.md),
[disposable emulator runbook](android-validation.md) and
[release recovery procedure](release.md#release-contract-and-recovery) cover
operational handoff. Keep one Cargo target directory per worktree; record both
application and pinned framework SHAs with validation evidence.

## Find the owner

| Change | State/policy and orchestration | Adapter or host | Closest validation |
| --- | --- | --- | --- |
| Provider parsing/import | [providers](../crates/iptv-providers/src/lib.rs), [import](../crates/iptv-import/src/lib.rs), [runtime imports](../crates/iptv-runtime/src/imports.rs) | [network](../crates/iptv-net/src/lib.rs) | `cargo test -p iptv-providers -p iptv-import -p iptv-net`; synthetic mock import and cancellation scenarios |
| Guide and focus | [core](../crates/iptv-core/src/lib.rs), [guide](../crates/iptv-guide/src/lib.rs), [guide focus](../crates/iptv-presentation/src/guide.rs), [navigation](../crates/iptv-nav/src/lib.rs), [runtime guides](../crates/iptv-runtime/src/guides.rs) | [app input](../crates/iptv-app/src/input.rs) | `cargo test -p iptv-core -p iptv-guide -p iptv-nav -p iptv-presentation`; app guide navigation/refresh scenarios |
| XMLTV and guide repair | [EPG parser](../crates/iptv-epg/src/lib.rs), [runtime checks](../crates/iptv-runtime/src/guide_checks.rs), [repair](../crates/iptv-runtime/src/guide_repair.rs) | [network](../crates/iptv-net/src/lib.rs), [store](../crates/iptv-store-sqlite/src/lib.rs) | `cargo test -p iptv-epg`; runtime guide-check/repair scenarios and the selected parser fuzz workflow |
| Library and search | [library](../crates/iptv-library/src/lib.rs), [search](../crates/iptv-search/src/lib.rs), [runtime search](../crates/iptv-runtime/src/search.rs), [VOD](../crates/iptv-runtime/src/vod.rs) | [store](../crates/iptv-store-sqlite/src/lib.rs) | `cargo test -p iptv-library -p iptv-search`; runtime VOD/source-identity tests; [on-demand discovery store queries](../crates/iptv-store-sqlite/src/vod.rs) and [runtime discovery scenarios](../crates/iptv-runtime/tests/it/vod_library.rs) |
| Settings | [schema](../crates/iptv-settings/src/lib.rs), [runtime](../crates/iptv-runtime/src/settings.rs) | [settings view](../crates/iptv-app/src/settings_view.rs) | `cargo test -p iptv-settings`; app settings, persistence and backup/restore scenarios |
| Persistence and migrations | [service ownership](../crates/iptv-runtime/src/store_service.rs) | [SQLite](../crates/iptv-store-sqlite/src/lib.rs), [SQLCipher initialization](../crates/iptv-sqlcipher-init/src/lib.rs) | `cargo test -p iptv-store-sqlite -p iptv-sqlcipher-init`; [historical migration fixtures](../crates/iptv-store-sqlite/tests/it/migrations.rs); the browser build's tests with [`tools/wasm-test.mjs`](../tools/wasm-test.mjs) |
| Parental policy | [portable policy](../crates/iptv-core/src/parental.rs), [runtime admission](../crates/iptv-runtime/src/parental.rs) | [PIN UI](../crates/iptv-app/src/parental_view.rs) | `cargo test -p iptv-core -p iptv-presentation parental`; [runtime parental-access scenarios](../crates/iptv-runtime/tests/it/parental_access.rs) and app parental scenarios; revocation during queued work |
| Recording lifecycle | [portable model](../crates/iptv-core/src/recording.rs), [runtime](../crates/iptv-runtime/src/recordings.rs), [native ownership](../crates/iptv-runtime/src/recordings/host.rs) | [recording host adapter](../crates/iptv-app/src/host/native/recording_host.rs), [Android host](../hosts/android) | [runtime recording recovery](../crates/iptv-runtime/src/recordings/recovery_tests.rs) and app recorder-startup/transport scenarios; device background/resume checks |
| Playback and native controls | [core playback](../crates/iptv-core/src/playback.rs), [runtime playback](../crates/iptv-runtime/src/playback.rs), [opening](../crates/iptv-runtime/src/opening.rs) | [player adapter](../crates/iptv-player-mpv/src/lib.rs), [persistent surface](../crates/iptv-app/src/video.rs), [Android host](../hosts/android) | `cargo test -p iptv-player-mpv`; app playback/PiP/multiview scenarios; native playback on each changed host |
| Samsung Tizen host (Live TV) | [web application](../crates/iptv-web/src/lib.rs), [guide snapshot](../crates/iptv-epg/src/snapshot.rs) | [Tizen host](../hosts/tizen/README.md) | `cargo test -p iptv-web -p iptv-epg`; `hosts/tizen/build.sh`, then launch, list, zap and relaunch on a Tizen 5.5 TV |
| Localization/accessibility | [portable messages](../crates/iptv-presentation/src/i18n.rs), [contrast policy](../crates/iptv-app/src/theme.rs) | [message adapter](../crates/iptv-app/src/i18n.rs), [assistive-technology checklist](../CONTRIBUTING.md#screen-readers-and-large-text) | presentation catalog tests, app catalog/interface-language/accessibility scenarios; actual assistive-technology checks for host changes |
| Release and packaging | [release contract](release.md), [workflow](../.github/workflows/release.yml) | [CI tools](../tools/ci), [Android hosts](../hosts/android) | affected Python/shell/host tests, package smoke and workflow gates from the release contract |

Runtime services have [a declared compatibility boundary](runtime-api-v1.md).
Read it before changing public APIs: other public helpers may be experimental.
Transport, credentials and backups follow the [security contract](security.md).
Messages and language fallbacks follow the [localization contract](localization.md).

## Choose checks

Run targeted checks while iterating. For app scenarios, use
`cargo test -p iptv-app --features test-support --test app FILTER`, replacing
`FILTER` with a module/test name listed by
`cargo test -p iptv-app --features test-support --test app -- --list`.
Runtime test-support scenarios use
`cargo test -p iptv-runtime --features test-support`.
Useful exact commands for the map's domains include:

```sh
cargo test -p iptv-runtime --test it persistence_safety::
cargo test -p iptv-runtime --test it data_startup::
cargo test -p iptv-runtime --test it parental_access::
cargo test -p iptv-runtime --test it vod_library::
cargo test -p iptv-runtime --lib vod::
cargo test -p iptv-runtime --features test-support --test it vod_import_pipeline::
cargo test -p iptv-store-sqlite --lib vod::
cargo test -p iptv-store-sqlite --lib -- --ignored --nocapture discovery_latency
cargo test -p iptv-app --features test-support --test app vod_library
cargo test -p iptv-app --features test-support --test app library_management
cargo test -p iptv-app --features test-support --test app cov_settings_screen
cargo test -p iptv-app --features test-support --test app backup_restore
cargo test -p iptv-app --features test-support --test app recorder_startup
cargo test -p iptv-app --features test-support --test app recordings_transport
cargo test -p iptv-app --features test-support --test app tv_player
cargo test -p iptv-app --features test-support --test app pip_control
cargo test -p iptv-app --features test-support --test app multiview_menu
cargo test -p iptv-app --features test-support --test app catalog_lint
cargo test -p iptv-app --features test-support --test app interface_languages
cargo test -p iptv-app --features test-support --test app accessibility_visibility
python3 tools/ci/test-release-workflow.py
python3 tools/ci/test-publish-release.py
python3 tools/ci/test-rehearsal.py
```

Headless scenarios prove model, layout and native-prop behavior, not actual
decoding or screen-reader usability.

Examples of impact assessment:

- A settings-only change needs schema/default/serialization checks and the
  affected settings screen. A persisted setting also needs restart and
  backup/restore coverage; a translated label needs catalog/layout checks.
- A persistence change needs real SQLite migrations from historical fixtures,
  repeated startup and failure preservation. Exercise encrypted opens and
  restore when those paths are touched, rather than using SQL mocks alone.
- A playback change needs player-contract and native host checks, including
  stale source events, pause/seek and surface lifetime. PiP must keep using
  surface 0's player without opening another stream.
- An Android-host change needs host unit/lint/build checks and a run on
  `tab36` and/or `tv36`. Exercise the changed lifecycle/input path and identify
  emulator evidence separately from physical hardware evidence. Use the
  [Android smoke workflow](../.github/workflows/android-smoke.yml) as a starting
  point, not proof that every new behavior was exercised. These profiles do not
  replace the [physical-device requirement](../CONTRIBUTING.md#when-a-device-run-is-needed).
  With the documented SDK/NDK prerequisites, the Android workflow's local checks are:

  ```sh
  bash tools/ci/check-android.sh aarch64-linux-android
  bash tools/ci/check-android.sh armv7-linux-androideabi
  ABIS=arm64-v8a,armeabi-v7a bash tools/sync-host.sh
  (cd hosts/android && ./gradlew testDebugUnitTest lintDebug assembleDebug)
  ```

Before finishing, run the required minimum from [AGENTS.md](../AGENTS.md),
plus the affected checks and Android run above.
For host-side merge validation, [tools/check.sh](../tools/check.sh) runs the
full lint, library/app test, dependency, tooling and portable-build suite.
[Windows validation](../tools/build-windows.ps1) uses the same
[Rust command plan](../tools/ci/check-plan.py). Both select the same library
and app test-support features, ordinary app targets, exact isolated repair,
inspection-display, guide-latency and busy-reader scenarios, portable wasm builds
and normal app build.
Ordinary tests share one Cargo invocation and feature graph. Linux CI runs the
same plan as two parallel jobs, `tools/check.sh <tier> app` and
`tools/check.sh <tier> core`: the app part runs iptv-app's `app` and
`native_player` harnesses and the timing checks that reuse them, the core part
everything else. Both use the same packages and features, so they build one
dependency graph. Run `tools/check.sh` without a part to run the whole plan locally.
Examples are checked
by Clippy's `--all-targets` pass, without linking them in the test or coverage pass;
doctests run explicitly in the host checks.
Host workflows upload `rust-timings-*` artifacts with Cargo's compilation/linking
reports so cache and build-plan changes can be compared on subsequent runs.
Every Rust test invocation explicitly uses `--format=pretty`, so Linux, macOS
and Windows logs show each test name and its result instead of quiet progress dots.

Inspect the shared commands without running Cargo:

```sh
python3 tools/ci/check-plan.py list
python3 tools/ci/check-plan.py list --json
```

The runner streams child output directly and stops at the first failed command.
The wrappers retain platform setup and additional gates: Linux/macOS run formatting,
dependency auditing and shell/Python/Kotlin tooling checks; Windows keeps its
Win32 chrome/pointer and executable-icon checks. Equal command plans do not imply
equal native coverage. The real EGL/decode unit tests and the GTK native-player
integration harness are compiled only on Linux; Xvfb/GTK coverage and performance
journeys remain Linux lanes. This consolidation adds no Windows decoder/GUI test.
Packaging/release changes additionally follow the release contract; targeted
commands do not replace those gates.
Additional workflows validate Android builds, Windows, coverage, parser fuzzing,
device smoke and packages when selected. See [coverage and fuzzing](release.md#coverage-and-fuzzing)
and [Android smoke](release.md#android-smoke-test); passing the host script alone
does not prove those workflows passed. Provider/parser changes need their
selected fuzz lane as well as ordinary tests.

The portable crates built by the full check are:

<!-- portable-crates:start -->
`iptv-core`, `iptv-presentation`, `iptv-nav`, `iptv-settings`, `iptv-epg`, `iptv-providers`,
`iptv-guide`, `iptv-import`, `iptv-search`, `iptv-library`.
<!-- portable-crates:end -->

The [planner tests](../tools/ci/test-check-plan.py) check features, locked
commands, named output, exact ignored filters, streamed execution and fail-fast
behavior without compiling Rust. Cargo metadata also enforces at most two
integration-test executables per crate: add scenarios as modules in an existing
harness. Memory cases use child processes to retain isolated allocation counters
while sharing one link. The [check-parity tests](../tools/ci/test-check-parity.py)
verify wrapper delegation, this portable inventory, coverage arguments and this
map's local links. They run on Linux, macOS and Windows; they do not execute
the Rust commands or prove the semantic completeness of an impact assessment.

## Place new behavior

Keep durable selection, transitions and policy in the appropriate portable
domain model. Put native orchestration in a runtime domain module; adapters
own I/O and hosts own OS facts. Views project state and dispatch actions.
Keep the shared `Ui` and `Runtime` owners as wiring: a new feature should not
add a second global registry or scatter its state across unrelated modules.

Before review, identify which of these contracts the change affects:

- Persisted data, historical migrations, repeated startup and restore.
- Source/content identity, cancellation, stale completion and owner teardown.
- Credential/logging boundaries and synthetic test evidence.
- Capability/permission changes and admission checks after UI rendering.
- Focus restoration, accessibility labels, translations and enlarged text.
- Persistent player lifetime and provider connection accounting.

Use the existing [runtime boundary tests](../crates/iptv-runtime/tests/it/architecture.rs)
and [shell boundary tests](../crates/iptv-app/tests/app/architecture.rs) to guard
concrete ownership decisions. Change an allowlist only with an explanation of
why that dependency belongs there. Extract coordination incrementally when a
feature needs it; this map does not prescribe a wholesale rewrite.
