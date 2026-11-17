# Validation tiers

[Validation](../.github/workflows/validation.yml) owns PR, master-push, nightly
and release-candidate checks. Reusable workflows have no independent PR/push
subscriptions, so each selected lane runs once. Selection and the aggregate use
[the same tested plan](../tools/ci/validation-plan.py).

| Validation | PR and master push | Nightly | Release candidate |
|---|---|---|---|
| Formatting, lint, dependency policy and CI helper tests | Relevant code/build changes; documentation-only changes run tier-contract tests | Full | Full |
| Linux unit/integration correctness and doctests | Full suite for application/Rust changes | Full | Full |
| iOS device/simulator adapters | Compile shared Rust/iOS input changes with verified MPVKit | Compile | Compile |
| Production Linux credential broker | Credential/storage/build changes, isolated DBus and disposable keyring | Full | Full |
| Nine portable crates built for wasm | Once, on Linux | Once, on Linux | Once, on Linux |
| macOS/Windows | Compile all test targets, native player tests, Windows chrome tests, production app build | Full host suites | Full host suites |
| Linux GTK PiP/context-loss and correlated first-render journeys | Ordinary host checks; no dependency on coverage | Full | Full |
| Reader/writer/analysis independence gates | Exact isolated gates on Linux | Exact gates on every host | Exact gates on every host |
| Android compile | Both ARM ABIs for Rust/dependency changes; packaging replaces duplicate compilation | Packaging compiles both | Shipping package jobs compile both |
| Tablet/TV playback, lifecycle and remote-input smoke | UI/Android/runtime/player/input/network changes | Both forms | Both forms for candidate code |
| Installers and APK/AAB | Actual packaging/runtime/dependency inputs; desktop platform-specific scripts select that platform | All supported formats | Final shipping artifacts in clean release jobs |
| Coverage and floors | Coverage/validation infrastructure changes; manual run available | Full workspace | Full report for this candidate SHA |
| Fuzz | Affected targets, 30 seconds each; common dependency changes select all | All targets, 300 seconds each | All targets, 30 seconds each and retained corpus |
| Allocation and large EPG/catalog budgets | Storage/import/EPG and their parser, schedule and model dependencies; small functional regressions stay in Linux tests | Full release-mode fixture, including ignored million-programme EPG case | Full release-mode fixture |

Ordinary PR correctness tests still cover the whole Linux workspace. Only the
three allocation-measurement modules are deferred to the explicit stress lane;
functional streaming, import, encryption, cancellation and storage tests stay in
the ordinary suite. The stress lane uses the existing `memory` executable and
`--include-ignored`, retaining the original allocation assertions.

The PR native tier checks every test target, executes the native player suite
and Windows chrome tests, and builds the production app without test-support.
Nightly and release tiers also execute shared suites on macOS and Windows,
covering filesystem, process and platform behavior beyond Linux. wasm output
and ordinary doctests are checked once on Linux rather than on three hosts.

Performance journeys retain their approved three repetitions and exact
store-read/player-open ceilings. Their wall-clock values remain informational.
Changing repetitions requires an explicit fixture-policy change; this tier
split does not weaken that policy or existing strict display latency budgets.

Fuzz targets run in one job, sharing the pinned tool, sanitizer dependencies
and full corpus cache. New/shared parser inputs select all targets; unknown
fuzz target names fail before execution. Only master refreshes reusable caches. Host, coverage and stress lanes retain
compiled dependencies even when a later assertion or coverage floor fails. PRs
restore those caches without writing them, and clean release packaging keeps
its cache opt-out.

## Required checks and migration

The `validation` job runs even when selection or another lane fails. It accepts
only successful selected jobs and deliberate skips, and rejects canceled,
missing and unexpectedly skipped selected jobs. It also requires helper tests
and successful change detection. This closes the previous gap where the actual
source-only `android-check` or Windows lane could fail without blocking merge.

The current protected contexts are `test (ubuntu-24.04)`, `test (macos-15)` and
`android`. Temporary compatibility jobs retain those exact names and each
mirrors the whole aggregate. Reusable host jobs have distinct prefixed check
names; the old independent event subscriptions are removed to avoid ambiguous
or duplicate protected checks.

After observing the aggregate on code, docs-only, native, dependency and
packaging PRs, an administrator can replace the three required contexts with
`validation`, preserving existing GitHub Actions app binding and the other
protection settings. Remove compatibility jobs only after that protection
change is effective. This implementation does not modify repository settings.

## Release artifacts

The release workflow explicitly requests `tier: release`, which runs full
candidate validation at the exact checked-out commit. It cannot inherit a lean
PR default or reuse a green nightly from another commit. Full nightly validation
also builds preview installers; the candidate tier delegates those builds to
the existing clean shipping-package jobs, avoiding a second throwaway installer
build before signing. Build/sign job isolation and signing environments remain
in place.

Android's final signed APK/AAB retain native ABI, notices and selected-certificate
verification. macOS checks the final disk image, and a fresh Windows job without
signing credentials installs, launches and uninstalls the final installer before
publication. The x86_64 development smoke APK exercises candidate behavior; it
is not evidence of shipping ARM playback. Tablet/TV device validation remains
necessary for that architecture-specific behavior, without adding a new CI
approval or environment gate.

After a tagged release is published, a fresh job downloads the public assets,
checks their exact inventory and hashes, and verifies provenance against the
release workflow, tag ref and candidate commit. It performs no compilation and
does not replace or mutate a public release. A failure reports a verification
problem with the published bytes.

## Local commands

```bash
# Existing complete Linux validation, including isolated GTK lifecycle smoke.
bash tools/check.sh
# PR correctness tier: allocation measurements run through the selected stress lane.
bash tools/check.sh pr
# Inspect exact commands without compiling.
python3 tools/ci/check-plan.py list --tier native --host macos --json
# Nightly/release allocation fixtures, including the ignored large EPG case.
bash tools/ci/import-stress.sh
```

Linux native lifecycle checks require `Xvfb`, software GL, and the pinned media
runtime. All fixtures and network traffic are synthetic. Hosted CI uploads Cargo
timing reports, coverage and performance reports. Compare critical-path duration
and total runner-minutes after master caches warm; cold runs and queue time are
not steady-state measurements.


## Executable inputs and production-only behavior

`docs/capabilities.md` is consumed by a core test and selects host validation;
`docs/contributing.md` is consumed by helper tests and selects those tests. Changes
to lint and line-length wrappers execute the wrappers in the host lane. Other
prose-only changes retain inexpensive planner validation. Every workspace crate
must have a coverage floor or a nonempty documented exemption; SQLCipher's small
process-global initializer is explicitly exempt while its isolated store tests run.
Linux also builds workspace rustdoc with warnings denied, keeping API links valid.

The iOS lane prepares the checksum-pinned MPVKit frameworks and checks both device
and simulator Rust targets without signing credentials. It establishes compilation,
not simulator execution, signing or iPad playback. Linux production credential
validation compiles without test-support, checks that the exact ignored broker test
exists, then runs it inside a fresh D-Bus session and disposable Secret Service home.
It never inherits the login bus, display or keyring. Package window startup remains
separate from persistence/reopen evidence.

Standalone host and Android jobs now have distinct display names; compatibility
contexts continue to mirror the aggregate. Migration to the unique `validation`
check remains a separate hosted setting change. Test a prose-only PR and a failing
selected lane before removing old required contexts, and keep the previous
protection configuration available for rollback.

Full nightly and release tiers select every validation lane (shipping packaging
replaces duplicate Android/desktop package lanes for releases). The release depends
on the candidate's full validation, never a different nightly SHA. Workflow failures
remain visible in Actions; maintainers should enable GitHub workflow-failure
notifications and inspect the full nightly result before release preparation.
No automatic issue posting or account notification setting is configured here.

## Dependency update candidates

Read-only inspection on November 17, 2026 confirmed Cargo Dependabot run
43337334688 failed while fetching the sibling `../nsh` manifests; this was a
file-fetch error, not a guessed resolver failure. The native Cargo Dependabot
version-update entry is replaced by the weekly/manual `dependency-candidate.yml`.
It checks out the pinned sibling, updates Cargo.lock, checks dependency policy and
runs the host PR plan, then retains the candidate lockfile and patch even on failure.
It has read-only permissions and never pushes, posts or opens a PR. Maintainers
review the diff, reconcile generated shipping-runner locks and submit it through
normal PR/native/package gates before adoption. Actions and Gradle updates remain
on Dependabot. A workflow definition is not evidence its first scheduled run passed.
