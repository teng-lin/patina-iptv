# Validation tiers

[Validation](../.github/workflows/validation.yml) owns PR, master-push, nightly
and release-candidate checks. Reusable workflows have no independent PR/push
subscriptions, so each selected lane runs once. Selection and the aggregate use
[the same tested plan](../tools/ci/validation-plan.py).

| Validation | PR | Nightly | Master/release candidate |
|---|---|---|---|
| Formatting, lint, dependency policy and CI helper tests | Relevant code/build changes; documentation-only changes run tier-contract tests | Full | Full |
| Linux unit/integration correctness and doctests | Full suite for application/Rust changes | Full | Full |
| iOS device/simulator adapters | Compile shared production Rust/iOS input changes with verified MPVKit | Compile | Compile |
| Production Linux credential broker | Credential/storage/build changes, isolated DBus and disposable keyring | Full | Full |
| Nine portable crates and the Tizen core (`iptv-web`) built for wasm | Once, on Linux | Once, on Linux | Once, on Linux |
| macOS/Windows | Compile all test targets, native player tests, Windows chrome tests, production app build | Full host suites | Full host suites |
| Linux GTK PiP/context-loss and correlated first-render journeys | Ordinary host checks; no dependency on coverage | Full | Full |
| Reader/writer/analysis independence gates | Exact isolated gates on Linux | Exact gates on every host | Exact gates on every host |
| Android compile | Both ARM ABIs for production Rust/dependency/Android changes | Both ARM ABIs | Shipping package jobs compile both |
| Tablet/TV playback, lifecycle and remote-input smoke | UI/Android/runtime/player/input/network changes | Both forms | Both forms for candidate code |
| Installers and APK/AAB | Desktop installers for packaging scripts, resources and bundled runtime changes; one x86_64 debug APK for selected emulator smoke | Desktop preview formats; ARM compilation and emulator smoke | Unsigned release ARM APK/AAB and desktop installers, built once and retained |
| Coverage and floors | Coverage/validation infrastructure changes; manual run available | Full workspace | Full report for this candidate SHA |
| Fuzz | Affected targets, 30 seconds each; common dependency changes select all | All targets, 300 seconds each | All targets, 30 seconds each and retained corpus |
| Allocation and large EPG/catalog budgets | Storage/import/EPG and their parser, schedule and model dependencies; small functional regressions stay in Linux tests | Full release-mode fixture, including ignored million-program EPG case | Full release-mode fixture |

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
Cargo manifest and workspace lockfile changes still run native compilation on
all desktop hosts and Android. The desktop installer jobs run when the PR also
changes a packaging recipe or a shipped resource. Updating the generated
package lockfile still runs the lock-graph consistency checks.

Performance journeys retain their approved three repetitions and exact
store-read/player-open ceilings. Their wall-clock values remain informational.
Changing repetitions requires an explicit fixture-policy change. The busy-guide
test requires navigation to mount while another connection holds the SQLite
writer lock, and requires the queued write to save after the lock is released.
It reports maximum key-to-frame latency. On a controlled performance host, run
`IPTV_STRICT_GUIDE_LATENCY=1 bash tools/check.sh pr` to also enforce the 50 ms limit.
The Linux job reuses the app and native player test executables that Cargo
reported during workspace testing. Its scenario timing artifact records
individual child process durations in milliseconds. The manual **Tests**
workflow's `app-test-jobs` input compares four, six, or eight Rust test threads
and sets the same cap on Linux app scenario subprocesses. The dedicated timing
checks run separately.

Fuzz targets run in one job, sharing the pinned tool, sanitizer dependencies
and full corpus cache. Five-minute runs build the instrumented targets before
running up to three targets concurrently. New/shared parser inputs select all
targets, and unknown fuzz target names fail before execution.
Only master refreshes shared Cargo caches. Host, coverage and stress lanes retain
compiled dependencies when a later assertion or coverage floor fails. macOS desktop
previews use a dependency cache for the generated NSH runner workspace. The
scheduled Validation run warms this cache on master. Windows native tests and
installers use separate caches keyed by the MSVC toolset, Windows SDK, Rust
and dependency inputs. The unsigned Windows installer and macOS package jobs
on master can seed the preview caches from their fresh builds. Those jobs look
up cache keys without downloading compiled application dependencies.
Linux desktop packaging does not retain a Cargo cache: packaging runs only for
relevant PR changes or scheduled/full validation, and its large cache competes
with the frequently used host and tool caches. Master candidates do not seed
it, and successful master pruning retires unused entries from its old namespace.

The macOS package setup uses the runner's pkg-config and the pinned libmpv
verified by the package script. Host player tests still have FFmpeg for media
fixtures. On a macOS tool cache miss, packaging tries a 30-day build artifact
for the exact NSH CLI or cargo-about identity. It checks that the artifact
came from a trusted master run and verifies the archive digest before use.
A missing or invalid artifact triggers the locked source build.

Master also refreshes the exact-key NSH runtime and Gradle dependency caches.
Unsigned shipping builds use fresh app output directories and may restore
the pinned tools and runtimes. Android candidates compile ARM64 and ARMv7 in
parallel, each with its own pinned NSH runtime cache. Both jobs can reuse the
same Ubuntu package snapshot and start with fresh app build output. A single Gradle job
checks the library architectures, source commit and matching NSH assets,
then packages both ABIs with the generated notices. Signing jobs use fresh
runners.

Ubuntu package snapshots can install offline across runner image refreshes
within the same Ubuntu release, architecture and package list. If the installed
base needs a different package version, apt refreshes the signed indexes and
downloads the missing archives. Master saves the refreshed snapshot. Unsigned
Linux candidates can restore these downloads while compiling the app in a fresh
output directory.

The Android smoke builder links NSH's Linux describe program against Ubuntu's
libmpv runtime and uses FFmpeg to generate media fixtures. Dispatch **Android
smoke** with `cold: true` to bypass apt, Rust, NSH and Gradle caches for a
comparison run. The ARM source check and import stress lanes install compiler
prerequisites. The iOS compile and Rust SBOM lanes use metadata setup, with
MPVKit installed by the iOS check itself.

When the plan selects both production credential persistence and host tests,
the Linux host job runs the credential script with its disposable D-Bus session
and keyring. The aggregate requires explicit success from that step. A
credential-only change still selects the standalone credential job.
When host and iOS checks are both selected, the macOS host job compiles the
iOS device and simulator targets and checks the simulator entitlements. An
iOS-only change or manual dispatch can use the standalone workflow. PRs that edit
either standalone workflow run that workflow even when host tests also run.

Changes confined to the Rust app's test directory run host correctness and
Windows test compilation. Edits to a leaf validation workflow select its
own lane. Shared workflow and shipping dependency changes use broad validation.

## Required checks and migration

The `validation` job runs even when selection or another lane fails. For master
and manual candidates it also requires the selected unsigned shipping build;
a failed package cannot leave the aggregate or protected aliases green. It accepts
only successful selected jobs and deliberate skips, and rejects canceled,
missing and unexpectedly skipped selected jobs. It also requires helper tests
and successful change detection. This closes the previous gap where the actual
source-only `android-check` or Windows lane could fail without blocking merge.

The current protected contexts are `test (ubuntu-24.04)`, `test (macos-15)` and
`android`. Temporary compatibility jobs retain those exact names and each
mirrors the whole aggregate. Reusable host jobs have distinct prefixed check
names; the old independent event subscriptions are removed to avoid ambiguous
or duplicate protected checks.

Verify the aggregate check on PRs and merge-queue runs, then add `validation`
as a required check with the existing GitHub Actions app binding. Once that
requirement is effective, remove the three alias requirements and set the
repository variable `PATINA_REQUIRE_AGGREGATE_ONLY` to `true` to skip the alias
jobs. Master pushes and scheduled runs use the aggregate directly.

## Release artifacts

Master pushes run the complete `release` validation tier alongside
[Unsigned candidate packages](../.github/workflows/unsigned-candidate.yml). This
replaces master's selective preview installer jobs; it does not add another copy
of packaging. Every candidate has coverage, all native host suites, both emulator
forms, stress, fuzz, iOS adapter compilation and production credential checks.
Unsigned builds check ARM APK/AAB contents, installer integrity and startup, native
sources and SBOMs without release credentials. PRs retain selective desktop
installer checks, ARM source compilation and a single x86_64 debug APK shared by
the tablet/TV emulators. ARM debug APK packaging is manual only; no debug AAB is
built. Gradle unit tests and lint run in the smoke and unsigned-release builds.

The final master job seals a 30-day `release-candidate` artifact: exact source SHA,
workflow run and attempt, full validation plan and successful aggregate/build
checks, source and runner locks, NSH/compiler/recipe input hashes, build profile
and ABI selection, and SHA-256 of every unsigned input, SBOM and source archive.
Desktop build-environment inventories retain the actual compiler/tool and system
package versions. This bundle is immutable within the run. When promoted, `candidate-manifest.json`
is published with the final release, covered by its checksums and provenance,
so the build/check/digest record survives Actions artifact expiration.

A tag selects only a successful **master push** of the Validation workflow in this
repository at the **identical commit SHA**. It rechecks manifest identities,
mandatory artifact groups, file inventory and all digests before uploading the
same bytes into the release's isolated signing jobs. PR artifacts, another SHA
(including a squash-equivalent source tree), a failed run or an earlier attempt
cannot qualify. Full candidate tests and unsigned compilation are skipped only
after this verification succeeds. Without a candidate, unsigned builds can run
concurrently with full checks; signing waits for both. Master sealing likewise
waits for validation and unsigned packaging, so a failed check cannot authorize
promotion. Missing/expired evidence runs complete release
validation and the same unsigned build workflow; malformed evidence fails closed.
A manual Validation dispatch with `tier=candidate` exercises full checks and the
unsigned recipe on a branch without sealing trusted evidence or signing. Manual
release/rehearsal runs always exercise the full path and requested matrix,
including optional iOS packaging. Publication still attests the final signed
bytes under the release workflow/tag/SHA and verifies public bytes afterwards.

PR validation cancels older runs of that PR. A draft PR runs only the fast lanes;
its packaging, Windows installer, coverage and emulator smoke lanes wait until it
is marked ready for review, which starts a complete run. Open a PR as a draft
while pushing fixups. A draft run always fails its gate, naming the lanes it
deferred, so its result never satisfies the required checks for that commit.
Master validation lets a running candidate finish and keeps only the newest
pending master push. Called release validation
has an independent run-specific group and cannot be canceled by a master push.
On a master candidate with a failed validation or package job, the seal job
reports the failed gate before downloading artifacts. Re-running failed jobs
retries the seal after the gate passes and reuses successful package jobs in the
same run.

Release and master share the unsigned package recipe. The release retains signing, final artifact smoke,
attestation and publication. Failed stages can be rerun without rebuilding passed
package jobs; never use **Re-run all jobs** when preserving those artifacts.

The helper lane owns Python/shell lint and helper regressions once per validation.
It runs in parallel with the host lane, and the aggregate requires both.
`tools/check.sh` retains all helper and lint checks locally. A docs-only selection still exercises
the planner and aggregate contract without setting up compilers.

Performance policy uses the PR/merge-group base or master's `before` commit. Tag,
manual and scheduled runs use the checked-out source commit's first parent, never
a tag push's previous target. Shallow history is fetched before deciding whether
the source is a root; an explicit missing base is fetched by SHA or fails closed.
Only an actual root or a base predating performance policy bootstraps the policy;
missing explicit objects cannot waive baseline review.

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


## Runner setup and re-runs

Hosted runner setup must fail fast, not consume a job's whole timeout (#159).
`.github/actions/setup` installs Linux packages through `tools/ci/apt-install.sh`.
Each download attempt is bounded to four minutes, each list refresh or dpkg
repair to 90 seconds, and each offline install to three minutes. On GitHub's
Ubuntu runners, known cloud archive URLs use Canonical's HTTPS archive first.
A failed fetch retries once with the configured mirror. apt refreshes signed
indexes and can reuse completed downloads across attempts. The install starts after the download
finishes, and a failed install gets one offline retry after dpkg repair.
apt gives up on a connection that sends nothing for 30 seconds.
The script skips the image's initramfs and
man-db rebuilds, which GTK's udev/systemd upgrade otherwise triggers. Every
workflow step using the setup action has its own `timeout-minutes`, and a job
needing further Linux packages names them in the action's `apt-packages`
input, or calls the script, rather than running its own `apt-get install`.

Package lists and downloaded `.deb` files are cached under a key naming the
runner image (`ImageOS`/`ImageVersion`), the Ubuntu release, the architecture,
the package list and the snapshot format version. A new runner image can reuse
the archives for the same Ubuntu release and package list. Each snapshot
records when to use the Canonical archive's indexes. A gap in a restored cache
triggers a refreshed download. Master and nightly
populate shared entries for each runner image and package list. Cold installs
use the mirror's current packages. Linux packaging keeps recommended packages because it bundles runtime modules and copyright
files from the build host. Android ARM type checking and SBOM generation use
smaller prerequisite sets.
The Linux host cache also covers `hosts/tizen-nsh`, which has its own workspace
and lockfile.

The Linux source archive collector runs up to three source packages at a time,
bounds apt and download calls, and reports each source package as it starts.
If the exact cache misses, it can restore the previous archive snapshot. Apt
downloads the missing files and the collector verifies each file against the
source metadata. The saved snapshot holds only the files used by this build.

The Android smoke build caches dependencies for the generated NSH runner
workspace using the committed runner lock, NSH pin, compiler, NDK, host library
versions, ABI, and profile. The generic pinned NSH runtime has a separate cache.
The development APK disables LTO and records Cargo timings. A manual Android
smoke run can select ThinLTO for comparison.

The `android-check` loss attributed to post-run cleanup on #143 (run 44780222843,
attempt 1) was in fact this download: its setup step spent 40 minutes fetching
193 MB from the mirror, and its post steps took about one second.

When a hosted runner or mirror still stalls, re-run only the failed jobs:

```bash
gh run rerun <run-id> --failed
```

This keeps successful jobs and their artifacts. Use the run's **Re-run failed
jobs** button in the Actions UI, never **Re-run all jobs**.

## Executable inputs and production-only behavior

`docs/capabilities.md` is consumed by a core test and selects host validation;
`docs/contributing.md` is consumed by helper tests and selects those tests. Changes
to lint and line-length wrappers execute the wrappers in the host lane. Other
prose-only changes retain inexpensive planner validation. Every workspace crate
has a Linux line coverage floor. SQLCipher coverage includes its initializer
suite and the store's isolated process tests.
Linux also builds workspace rustdoc with warnings denied, keeping API links valid.

The iOS lane prepares the checksum-pinned MPVKit frameworks and checks both device
and simulator Rust targets without signing credentials. It builds the simulator
library and checks it carries the Keychain entitlements the simulator needs
([iOS simulator validation](ios-validation.md#entitlements)). It establishes
compilation, not simulator execution, signing or iPad playback. Linux production credential
validation compiles without test-support, checks that the exact ignored broker test
exists, then runs it inside a fresh D-Bus session and disposable Secret Service home.
It never inherits the login bus, display or keyring. Package window startup remains
separate from persistence/reopen evidence.

Standalone host and Android jobs now have distinct display names; compatibility
contexts continue to mirror the aggregate. Migration to the unique `validation`
check remains a separate hosted setting change. Test a prose-only PR and a failing
selected lane before removing old required contexts, and keep the previous
protection configuration available for rollback.

Full nightly validation selects native, portable and behavior lanes plus desktop
preview packaging. Master/release candidates replace preview packaging and ARM
compilation with the checked unsigned shipping artifacts. The release depends
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
