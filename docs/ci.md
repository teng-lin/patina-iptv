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
| Portable crates and the Tizen core (`iptv-web`) built for wasm | Once, on Linux | Once, on Linux | Once, on Linux |
| macOS/Windows | Compile all test targets, native player tests, Windows chrome tests, production app build | Full host suites | Full host suites |
| Linux GTK PiP/context-loss and correlated first-render journeys | Ordinary host checks; no dependency on coverage | Full | Full |
| Reader/writer/analysis independence gates | Exact isolated gates on Linux | Exact gates on every host | Exact gates on every host |
| Android compile | Both ARM ABIs for production Rust/dependency/Android changes | Both ARM ABIs | Shipping package jobs compile both |
| Tablet/TV playback, lifecycle and remote-input smoke | UI/Android/runtime/player/input/network changes | Both forms | Both forms for candidate code |
| Installers and APK/AAB | Desktop installers for packaging scripts, resources and bundled runtime changes; one x86_64 debug APK for selected emulator smoke | Desktop preview formats; ARM compilation and emulator smoke | Unsigned release ARM APK/AAB and desktop installers, built once and retained |
| Coverage and floors | Rust source changes and coverage/validation infrastructure changes (deferred on drafts); manual run available | Full workspace | Full report for this candidate SHA |
| Fuzz | Affected targets, 30 seconds each; common dependency changes select all | All targets, 300 seconds each | All targets, 30 seconds each from the restored corpus (only nightly saves it) |
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

The cache pruner runs after successful master validation and every four hours.
The scheduled run uses the latest successful master push validation's time
window, so it can remove superseded entries when master has advanced.
It skips families whose newest cache was saved after that run, and uses the
same last-access checks as completion-triggered pruning. Run
**Prune superseded CI caches** on `master` to use the same cleanup on demand.

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
own lane. Validation's transitive local reusable-workflow call graph determines
which workflows participate; workflows outside that graph run only helper checks.
Unrecognised or missing local workflow references keep broad validation. Composite
actions remain shared CI inputs. Workflow/action edits also run pinned actionlint over all workflow consumers.

Markdown at any depth is documentation unless it is embedded or packaged.
`docs/platform-support.md` keeps host checks; font provenance, vendored Markdown and
`tools/mpv-runtime*/**` keep their build/package consumers. Repository issue/PR
templates, CODEOWNERS and Dependabot configuration run helper checks. Unknown
non-documentation inputs and `.gitattributes` retain native validation. A scanner
regression checks Rust `include_str!`/`include_bytes!` Markdown consumers so adding
an embedded document requires an explicit planner exception.

## Windows and Linux-only checks

The planner and parity tests enforce more than crate names: features, target
selection, exact ignored filters, output flags, wrapper delegation and failure
propagation. Windows retains its Win32 chrome/pointer and executable-icon checks.
Linux's EGL/decode unit tests and GTK native-player harness are Linux-only;
the Xvfb/GTK coverage and performance lanes do not establish Windows or macOS
real-decoder coverage. `IPTV_REQUIRE_PLAYER_TESTS` does not enable those tests
on Windows. A Windows native decoder/GUI test is outside this shared plan.

The Windows setup action uses the same Rust build cache as the other hosts;
only master saves it. Formatting, shell/Python/Kotlin tooling and workspace-wide
`cargo deny` remain additional Linux/macOS wrapper checks. The untargeted deny
audit includes Windows dependencies; `audit-runners.sh` additionally audits the
graph the Windows executable ships, for its own target.

## Required checks

The `validation` job runs even when selection or another lane fails. For master
and manual candidates it also requires the selected unsigned shipping build;
a failed package cannot leave the aggregate green. It accepts
only successful selected jobs and deliberate skips, and rejects canceled,
missing and unexpectedly skipped selected jobs. It also requires helper tests
and successful change detection.

`validation` is the required check on `master`. The `protected-contexts` jobs,
named `test (ubuntu-24.04)`, `test (macos-15)` and `android`, each mirror the
aggregate and are skipped while the repository variable
`PATINA_REQUIRE_AGGREGATE_ONLY` is `true`. Changing branch protection or that
variable requires explicit repository-owner approval.

To roll back to the legacy requirements, first set the variable to false or
remove it, start a fresh PR validation run and verify all three alias jobs
report. Then restore the legacy requirements and verify them before optionally
removing the `validation` requirement. Requiring them while the aliases are
skipped blocks PRs.

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

## Measuring CI changes

`tools/ci/validation-timeline.py report` analyzes downloaded run, job and log
snapshots of successful PR runs offline, and `compare` compares two reports (see
`--help`). A speedup claim needs actual before and after cohorts of at least 20
comparable runs each, captured with their query; replays and models are not
after measurements. Keep raw snapshots outside the repository.

Any change that overlaps the Android smoke build with its emulator jobs, or
reuses an APK, must bind the APK bytes to the checked-out commit, producer run,
attempt and digest, handle failed-job reruns, fail closed on stale inputs,
cancel promptly when the producer fails, keep cold-build deadlines and exercise
both tablet and TV lanes.

### Candidate handoff pilot

[`candidate-handoff-pilot.yml`](../.github/workflows/candidate-handoff-pilot.yml)
is an experiment, not a production lane
([#336](https://github.com/teng-lin/patina-iptv/issues/336)). It builds a Windows
release executable on one runner and packages it on another, beside a cold
control. Its jobs run only on the `ci/candidate-artifact-handoff-336` branch, by
push or manual dispatch, with read-only permissions and no secrets, publication
or callers; artifacts are prefixed `pilot336-`.

The consumer takes expected digests from trusted `needs` outputs, verifies the
producer job, attempt and artifact through the API, and independently
fingerprints source, locks, recipe, toolchain, native inputs and runtime. Only
an explicit `cold` request or a proven absent or expired artifact falls back to
a local build; every mismatch, unsafe archive or API error fails. The one
normalization is the line order of Rustup's `lib/rustlib/components` receipt.
`tools/ci/candidate-handoff.py` implements this contract. Adopting it needs a
separate change and measurements of the integrated critical path.


## Runner setup and re-runs

Hosted runner setup must fail fast, not consume a job's whole timeout (#159).
`.github/actions/setup` installs Linux packages through `tools/ci/apt-install.sh`.
Each download attempt is bounded to four minutes, each list refresh or dpkg
repair to 90 seconds, and each offline install to three minutes. On GitHub's
Ubuntu runners, known cloud archive URLs use Canonical's HTTPS archive first.
Where the image keeps Ubuntu's entries in `ubuntu.sources`, the script and the
source index refresh (`tools/ci/enable-deb-src.sh`) read that file alone: the
jobs need only Ubuntu packages, and an outage of one of the image's vendor
feeds would fail the refresh. A job needing a vendor package must change this.
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

Two other downloads have refused hosted runners for a while. The native runtime
fetch (`tools/ci/native_candidates.py`) retries a published release asset after
5, 15 and 45 seconds on HTTP 403, 408, 429, 500, 502, 503 and 504 and on
connection errors. A 404 still means an unpublished pin and selects the tested
candidate; a refusal never does, and the checksum applies to every download.
Maven Central has answered builds without a Gradle cache with HTTP 429, so on
GitHub Actions `hosts/android/settings.gradle.kts` asks Google's mirror of it
first, with Maven Central serving what the mirror lacks. Strict dependency
verification applies to both. Gradle does not fall back to the next repository
when one answers with an error, so a mirror outage fails the build.

When a hosted runner or mirror still stalls, re-run only the failed jobs:

```bash
gh run rerun <run-id> --failed
```

This keeps successful jobs and their artifacts. Use the run's **Re-run failed
jobs** button in the Actions UI, never **Re-run all jobs**.

## Executable inputs and production-only behavior

`docs/platform-support.md` is consumed by a core test and selects host validation;
`docs/development.md` is consumed by helper tests and selects those tests. Changes
to lint and line-length wrappers execute the wrappers in the host lane. Other
prose-only changes retain inexpensive planner validation. Every workspace crate
has a Linux line coverage floor. SQLCipher coverage includes its initializer
suite and the store's isolated process tests.
Linux also builds workspace rustdoc with warnings denied, keeping API links valid.

The iOS lane prepares the checksum-pinned MPVKit frameworks and checks both device
and simulator Rust targets without signing credentials. It builds the simulator
library and checks it carries the Keychain entitlements the simulator needs
([iOS simulator validation](testing.md#entitlements)). It establishes
compilation, not simulator execution, signing or iPad playback. Linux production credential
validation compiles without test-support, checks that the exact ignored broker test
exists, then runs it inside a fresh D-Bus session and disposable Secret Service home.
It never inherits the login bus, display or keyring. Package window startup remains
separate from persistence/reopen evidence.

Full nightly validation selects native, portable and behavior lanes plus desktop
preview packaging. Master/release candidates replace preview packaging and ARM
compilation with the checked unsigned shipping artifacts. The release depends
on the candidate's full validation, never a different nightly SHA. Workflow failures
remain visible in Actions; maintainers should enable GitHub workflow-failure
notifications and inspect the full nightly result before release preparation.
The [nightly observer](#nightly-failure-signal-and-bounded-infrastructure-retry)
maintains a tracking issue for scheduled failures. Account notification settings
remain the maintainer's choice.

## Dependency update candidates

Cargo has no Dependabot version-update entry, because Dependabot cannot fetch the
sibling NSH manifests. The weekly and manual
[`dependency-candidate.yml`](../.github/workflows/dependency-candidate.yml)
checks out the pinned sibling, updates Cargo.lock, checks dependency policy and
runs the host PR plan, then retains the candidate lockfile and patch even on failure.
It has read-only permissions and never pushes, posts or opens a PR. Maintainers
review the diff, reconcile generated shipping-runner locks and submit it through
normal PR/native/package gates before adoption. Actions and Gradle updates remain
on Dependabot. A workflow definition is not evidence its scheduled run passed.

## Nightly failure signal and bounded infrastructure retry

[Nightly Validation alert](../.github/workflows/nightly-alert.yml) observes
completed scheduled Validation runs on master. It executes only the observer
from the trusted default branch, without downloading run artifacts. Its token
can read contents/checks, update issues and re-run Actions. It does not modify
the Validation plan, selected lanes, aggregate gate or required checks.

The observer creates one bot-owned tracking issue with a stable hidden marker,
updates it on failures, closes it after the latest observed successful schedule,
and reopens that same issue on a later failure. The issue records run/attempt
links, unsuccessful jobs and schedule delay separately. Duplicate deliveries and
out-of-order events are harmless. A workflow concurrency group serializes issue
writes, and each observer reconciles ten recent schedules so replacement of a
pending observer does not drop the previous nightly. The issue retains thirty
attempts in its state journal and displays twenty. Missing/corrupt state and API
errors fail the observer visibly rather than discarding retry history.

An automatic retry requires the newest scheduled run's first attempt to fail
solely because a selected job was cancelled before any steps and its check has
the exact GitHub annotation: “The job was not acquired by Runner of type hosted
even after multiple attempts”. The aggregate and legacy proxy failures are
consequences of that abandoned lane. A missing log, generic cancellation,
execution timeout or mixed product failure cannot qualify. This deliberately
covers the observed runner-acquisition failure; other runner losses need new
positive evidence and tests before eligibility expands.

The observer records its one retry claim in the issue before calling GitHub's
[re-run-failed-jobs API](https://docs.github.com/en/rest/actions/workflow-runs#re-run-failed-jobs-from-a-workflow-run).
That endpoint re-runs failed jobs and their dependent jobs, preserving successful
selected lanes and re-evaluating the aggregate gate. If the API response is
ambiguous or rejected, the claim remains visible and no automatic second request
is made; investigate and retry manually if appropriate. A second Validation
attempt never qualifies for another automatic retry. Alert API failure leaves
the observer red and requires normal Actions monitoring.
