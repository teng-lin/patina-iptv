# Changing Patina

Start with [AGENTS.md](../AGENTS.md) for repository-wide rules and
[architecture](architecture.md) for dependency direction. This map locates
the owner and the closest validation for a change; it does not replace the
full checks before finishing or the release gates.

## Find the owner

| Change | State/policy and orchestration | Adapter or host | Closest validation |
| --- | --- | --- | --- |
| Provider parsing/import | [providers](../crates/iptv-providers/src/lib.rs), [import](../crates/iptv-import/src/lib.rs), [runtime imports](../crates/iptv-runtime/src/imports.rs) | [network](../crates/iptv-net/src/lib.rs) | `cargo test -p iptv-providers -p iptv-import -p iptv-net`; synthetic mock import and cancellation scenarios |
| Guide and focus | [core](../crates/iptv-core/src/lib.rs), [guide](../crates/iptv-guide/src/lib.rs), [navigation](../crates/iptv-nav/src/lib.rs) | [app input](../crates/iptv-app/src/input.rs) | `cargo test -p iptv-core -p iptv-guide -p iptv-nav`; app guide navigation/refresh scenarios |
| Library and search | [library](../crates/iptv-library/src/lib.rs), [search](../crates/iptv-search/src/lib.rs), [runtime search](../crates/iptv-runtime/src/search.rs), [VOD](../crates/iptv-runtime/src/vod.rs) | [store](../crates/iptv-store-sqlite/src/lib.rs) | `cargo test -p iptv-library -p iptv-search`; runtime VOD/source-identity tests |
| Settings | [schema](../crates/iptv-settings/src/lib.rs), [runtime](../crates/iptv-runtime/src/settings.rs) | [settings view](../crates/iptv-app/src/settings_view.rs) | `cargo test -p iptv-settings`; app settings, persistence and backup/restore scenarios |
| Persistence and migrations | [service ownership](../crates/iptv-runtime/src/store_service.rs) | [SQLite](../crates/iptv-store-sqlite/src/lib.rs), [SQLCipher initialization](../crates/iptv-sqlcipher-init/src/lib.rs) | `cargo test -p iptv-store-sqlite -p iptv-sqlcipher-init`; [historical migration fixtures](../crates/iptv-store-sqlite/tests/it/migrations.rs) |
| Parental policy | [runtime admission](../crates/iptv-runtime/src/parental.rs) | [PIN UI](../crates/iptv-app/src/parental_view.rs) | runtime parental-access and app parental scenarios; revocation during queued work |
| Recording lifecycle | [portable model](../crates/iptv-core/src/recording.rs), [runtime](../crates/iptv-runtime/src/recordings.rs) | [recording host](../crates/iptv-app/src/recording_host.rs), [Android host](../hosts/android) | runtime recording recovery and app recorder-startup/transport scenarios; device background/resume checks |
| Playback and native controls | [core playback](../crates/iptv-core/src/playback.rs), [runtime opening](../crates/iptv-runtime/src/opening.rs) | [player adapter](../crates/iptv-player-mpv/src/lib.rs), [persistent surface](../crates/iptv-app/src/video.rs), [Android host](../hosts/android) | player and app playback/PiP/multiview scenarios; native playback on each changed host |
| Localization/accessibility | [portable messages](../crates/iptv-core/src/i18n.rs), [theme](../crates/iptv-app/src/theme.rs) | [app scenarios](../crates/iptv-app/tests/app/main.rs) | core catalogue tests, app catalogue/interface-language/accessibility scenarios; actual assistive-technology checks for host changes |
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
Headless scenarios prove model, layout and native-prop behavior, not actual
decoding or screen-reader usability.

Examples of impact assessment:

- A settings-only change needs schema/default/serialization checks and the
  affected settings screen. A persisted setting also needs restart and
  backup/restore coverage; a translated label needs catalogue/layout checks.
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
  point, not proof that every new behavior was exercised.

Before finishing, the required minimum is `cargo test -p iptv-core`,
`cargo build -p iptv-core --target wasm32-unknown-unknown`, and
`cargo build -p iptv-app`, plus the affected checks and Android run above.
For host-side merge validation, [tools/check.sh](../tools/check.sh) runs the
full lint, library/app test, dependency, tooling and portable-build suite.
[Windows validation](../tools/build-windows.ps1) is kept in parity with it.
Packaging/release changes additionally follow the release contract; targeted
commands do not replace those gates.

The portable crates built by the full check are:

<!-- portable-crates:start -->
`iptv-core`, `iptv-nav`, `iptv-settings`, `iptv-epg`, `iptv-providers`,
`iptv-guide`, `iptv-import`, `iptv-search`, `iptv-library`.
<!-- portable-crates:end -->

The [check-parity tests](../tools/ci/test-check-parity.py) verify this list
against the actual build loop and check this map's local file links. They run
on Linux, macOS and Windows; they do not execute the commands printed here or
prove the semantic completeness of an impact assessment.

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

Use the existing [runtime boundary tests](../crates/iptv-runtime/tests/architecture.rs)
and [shell boundary tests](../crates/iptv-app/tests/app/architecture.rs) to guard
concrete ownership decisions. Change an allowlist only with an explanation of
why that dependency belongs there. Extract coordination incrementally when a
feature needs it; this map does not prescribe a wholesale rewrite.
