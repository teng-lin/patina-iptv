# Master candidate wall time and exact-pin reuse

November 25 candidates spent time waiting for earlier master runs and
waiting for runners after jobs were created. In a successful first-attempt
sample, median trigger-to-finish wall time rose
from 30.3 to 50.8 minutes between November 21 and November 25. Median wall
time from `plan` start rose from 29.7 to 33.1 minutes, and median summed
job-minutes rose from 223.8 to 229.4. The November 25 median wait before
`plan` could enter a runner queue was 16.7 minutes. [Issue #336](https://github.com/teng-lin/patina-iptv/issues/336)

## Two sealed candidates with comparable lanes

Times are minutes from the workflow's creation unless the row specifies a
duration. Both runs used NSH `fd847c95` and parallel ARM candidate
builds, with the immutable Linux NSH CLI available. The older November 21
30.3-minute run used the monolithic Android package job and did not produce
a successful seal, so the table uses a sealed 30.8-minute run from
November 24 for the like-for-like example.
[November 21 run](https://github.com/teng-lin/patina-iptv/actions/runs/44871354212)

| Timing | [30.8-minute candidate](https://github.com/teng-lin/patina-iptv/actions/runs/45399915160) | [52.0-minute candidate](https://github.com/teng-lin/patina-iptv/actions/runs/45518242886) |
| --- | ---: | ---: |
| `plan` created | 0.02 | 6.08 |
| `plan` runner queue, duration | 0.03 | 5.38 |
| `plan` finished | 0.43 | 11.80 |
| Windows host checks finished | 28.98 | 34.63 |
| macOS host checks finished | 22.80 | 46.73 |
| Linux candidate package, runner queue duration | 0.03 | 15.83 |
| Linux candidate package, execution duration | 20.88 | 22.67 |
| Linux candidate package finished | 21.35 | 50.30 |
| ARM candidate package finished | 23.40 | 48.95 |
| `validation` finished | 29.13 | 50.72 |
| `seal-candidate` finished | 30.75 | 52.00 |
| `plan` start to seal, duration | 30.70 | 40.53 |
| Total execution, unweighted job-minutes | 217.97 | 222.45 |

Windows host checks were the final prerequisite in the fast example. In
the slow example, Linux packaging waited 15.8 minutes for its runner and
became the final prerequisite. That job took 1.8 additional execution
minutes. Windows host checks used fewer runner-minutes in the slower run
and finished 15.7 minutes before Linux packaging. ARM candidate assembly finished
1.35 minutes before Linux packaging, so making Linux packaging faster than
that would expose another dependency in this run.

The slow example had 6.1 minutes of workflow-level wait before `plan` was
created and a further 5.4 minutes of runner queue for `plan`. The previous
master run sealed at 00:33:41 UTC, when this run's plan was created, after
the 00:27:36 trigger. The two task-1 runs also spent 28.3 and 36.3 minutes
respectively before plan creation. In the latter, the plan was created at
04:27:37 UTC, the preceding run's seal completion time. Its `plan` runner
queue lasted two seconds, and its first plan start to seal took only 27.4
minutes. `run_started_at` equalled the trigger timestamp in that run.
[Predecessor](https://github.com/teng-lin/patina-iptv/actions/runs/45510506946) · [Task-1 run A](https://github.com/teng-lin/patina-iptv/actions/runs/45539347352) · [Task-1 run B](https://github.com/teng-lin/patina-iptv/actions/runs/45542817982)

Master concurrency lets the running candidate finish and keeps the newest
pending push. The
pre-plan interval measures elapsed time before a job exists. The aligned
predecessor finishes show why it is useful to report separately from
`started_at − created_at` for runnable jobs. The first-attempt sample
cannot assign every pre-plan second to an individual scheduler decision.
[Master concurrency change](https://github.com/teng-lin/patina-iptv/commit/001a8d4ae97df8178f99c3a98b01363780a39e5f)

| Trigger date (UTC) | Runs | Trigger to finish p50 / p90 | Trigger to plan creation p50 / p90 | Plan start to finish p50 / p90 | Job-minutes p50 / p90 | Job runner queue p50 / p90 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Nov 21 | 3 | 30.3 / 40.8 | 0.6 / 0.7 | 29.7 / 40.1 | 223.8 / 248.5 | 0.03 / 0.85 |
| Nov 22 | 5 | 33.0 / 50.6 | 0.0 / 2.4 | 33.0 / 50.5 | 220.0 / 255.0 | 0.03 / 0.05 |
| Nov 23 | 3 | 55.9 / 61.9 | 19.8 / 27.7 | 32.1 / 36.1 | 207.3 / 283.6 | 0.62 / 5.05 |
| Nov 24 | 12 | 41.2 / 60.4 | 6.9 / 25.8 | 31.2 / 40.8 | 221.2 / 235.6 | 0.05 / 3.57 |
| Nov 25 through issue opening | 10 | 50.8 / 63.6 | 16.7 / 30.9 | 33.1 / 41.7 | 229.4 / 241.8 | 0.05 / 5.92 |

The issue's 53-minute value is the reported baseline. The independently
bounded cohort above excludes retried and incomplete runs and uses UTC
trigger dates, which gives a smaller sample. In the ten November 25
first-attempt runs, Linux packaging was the final work job four times and
macOS host tests twice. Windows host tests, macOS packaging, ARM assembly,
and tablet smoke each finished last once. The bottleneck can therefore
move between legs even after workflow-level wait is removed.

## NSH Android runtime and Patina app builds

The `nsh.rev` at the measured master head is
`413cfb65bf87a7bd9937f223fdc56896d8a718c8`.
The three verified runs below all checked it out and restored immutable
bundles under the exact key
`2b84c50ecad9e1baf856aabb9e695a8cf581286b8bf7b5574a4c7fc6bef60be3`.
The slow 52-minute candidate also restored all three ABI bundles under
the key for its earlier `fd847c95` pin. Its case therefore also includes
Patina builds with warm NSH runtime files.

| Master run | NSH fetch seconds, ARM64 / ARMv7 / x86_64 | Patina ARM app build and staging, ARM64 / ARMv7 | x86_64 development APK step |
| --- | ---: | ---: | ---: |
| [45539347352](https://github.com/teng-lin/patina-iptv/actions/runs/45539347352) | 8.3 / 6.6 / 8.4 | 12m27s / 13m36s | 4m19s |
| [45542817982](https://github.com/teng-lin/patina-iptv/actions/runs/45542817982) | 5.5 / 5.2 / 5.8 | 12m20s / 13m08s | 4m31s |
| [45619533821](https://github.com/teng-lin/patina-iptv/actions/runs/45619533821) | 6.5 / 4.4 / 5.7 | 11m39s / 12m33s | 8m29s |

Fetch seconds are from the timestamp of the `nsh-android-bundle.py fetch`
step to the `Restored CI tool from immutable release` line. The Android
composite setup itself took 26–41 seconds across the nine jobs and skipped
the `Build pinned Android runtime` fallback. The release tags for the key
are public, published and immutable. The fetch checks the bundle digest
and attestation, and unpacks the expected runtime files for the requested
ABI. The key covers the exact checkout, compiler, SDK, NDK, Java, build
recipe and selected build environment. The two ARM jobs then build and
stage Patina with `sync-host.sh --release`. The x86_64 job prepares the
development APK for smoke and includes app-native compilation and Gradle
work. Its runtime restore provides no evidence about either shipping ARM
compile. [Runtime identity](../../tools/ci/nsh-tool-key.py) · [Restore action](../../.github/actions/android/action.yml)

The current example finished in 28.50 minutes with 224.85 summed
job-minutes. Its Linux package finished at +27.28, followed by validation
and sealing. Windows host checks finished at +24.92, macOS host checks at
+25.00, and Android candidate assembly at +19.22. The two ARM jobs still
cost 29.82 job-minutes combined. The current bottleneck in this example
is Linux packaging, while making app-native Android work cheaper would
save runner time outside this critical path. Holding other timings fixed,
Linux packaging could improve the wall time by at most 2.28 minutes
before the macOS host checks finished last.
[Current exact-head run](https://github.com/teng-lin/patina-iptv/actions/runs/45619533821)

## Desktop artifact reuse

Release already reuses the correct class of artifact. The 0.1.9 release
verified candidate run `45619533821` and the identical SHA
`b61b524d9178b4349cad14827e0982759b42f87e`. It skipped its redundant
tests and unsigned package builds, then signed and verified publication.
The release workflow selected the sealed candidate rather than an
ordinary host-test artifact. [Release plan job](https://github.com/teng-lin/patina-iptv/actions/runs/45624404947/job/139240671280) · [Candidate verification](../../tools/ci/release-candidate.py)

The host and Windows test lanes build or check workspace artifacts,
including test features and development profiles. Windows explicitly
checks `target/debug/iptv-app.exe`, whereas its installer runs
`build-windows.ps1 -Release`. Linux and macOS shipping jobs use the NSH
generated runner with the shipping lock and link against the pinned,
verified libmpv runtime. The host jobs do not publish a binary with the
required release inputs. Windows's `-SkipBuild` only consumes an existing
`target/release/iptv-app.exe` without proving that executable's identity.
[Windows tests](../../.github/workflows/windows.yml) · [Windows packaging](../../tools/package-windows.ps1) · [Linux packaging](../../tools/ci/package-linux.sh) · [macOS packaging](../../tools/package-macos.sh)

These package jobs run in parallel with host checks. In the current
example, a Linux package handoff from the last Linux host part at +13.47
would leave 13.81 minutes for transfer, provenance verification, packaging
and smoke before missing the observed Linux finish at +27.28. Waiting for
Windows tests at +24.92 already exceeds the entire independent Windows
installer finish at +13.72. Even a valid executable handoff would need to
eliminate enough runner work to offset its dependency, transfer and queue
costs before improving wall time. The packaging step timings include
assembly and audits, so treating their full duration as reusable compile
time exaggerates the opportunity.

A separate reuse pilot needs two consumers of the same
candidate-grade binary. A producer would record the actual checked-out
commit, generated runner lock, NSH pin, compiler and target, profile,
features, version flags, pinned native-runtime digests and runner image.
A same-run consumer would verify that identity and the artifact digest
before packaging and running the bundle audit, notices, SBOM and smoke
checks. The
successful Validation gate, candidate sealing, and release trust checks
would still bind the checks and unsigned bytes to the exact source SHA
and workflow run and attempt. That pilot should measure wall time and
runner usage with the artifact checks enabled. The independent package
lanes preserve the current parallelism while that measurement is pending.

## Reproducing the timings

Query successful master push runs for `validation.yml` with UTC creation
dates November 21–25 and keep first attempts with `updated_at` before
`2026-11-25T12:58:59Z`, when the issue opened. Read the jobs of that exact
attempt. The cohort predates the current example, which is shown separately.

```sh
gh api --method GET repos/teng-lin/patina-iptv/actions/workflows/validation.yml/runs \
  -f branch=master -f event=push -f status=success \
  -f created=2026-11-21..2026-11-25 -F per_page=100 --paginate --slurp
gh api --method GET repos/teng-lin/patina-iptv/actions/runs/45518242886/attempts/1/jobs \
  -F per_page=100 --paginate --slurp
gh api repos/teng-lin/patina-iptv/actions/jobs/139223854310/logs
```

Use the maximum executed job's `completed_at` as the run finish. Subtract
the run's `created_at` for trigger-to-finish time, the plan's `started_at`
for execution wall time, and the plan's `created_at` from its `started_at`
for its runner queue. The interval before the plan is `plan.created_at −
run.created_at`. Sum `completed_at − started_at` across executed jobs
for unweighted runner-minutes, excluding skipped jobs. Day-level queue
percentiles pool those jobs, while other percentiles use one value per
run. The p50 is the median and p90 is the nearest rank. Read completion
offsets along workflow `needs` edges to identify the controlling job,
then validate its timings with the prerequisite and gate jobs.
