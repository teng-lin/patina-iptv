# PR critical-path evidence

Refs #334. This measurement work makes no Validation scheduling or Android
smoke execution change. The fresh APK build and its same-run artifact handoff
to **both** tablet and TV emulators remain intact. No speedup or ≤18-minute
target is claimed; there are no post-change scheduling observations.

## Fixed candidate sample

At 2026-11-25 13:45:38 UTC, the first API page contained the latest **100 of 244**
successful pull-request Validation runs. This was a bounded candidate sample,
not a complete Nov 23–25 population. Of those 100 candidates, 83 were completed
first attempts with a successful APK build and both successful emulator lanes.
Eleven lacked that smoke selection; six were retried runs. Eligible creation
times span 2026-11-23 21:15:46 through 2026-11-25 12:29:41 UTC.

| Actual interval | PR runs | p50 | p90 |
| --- | ---: | ---: | ---: |
| Whole run | 83 | 29.967 min | 51.350 min |
| APK build execution | 83 | 7.183 min | 11.333 min |
| APK build runner queue | 83 | 2.533 min | 8.767 min |
| Plan complete → APK build created | 83 | 1 s | 1 s |
| Tools complete → APK build created | 72 | 0 s | 1 s |
| Plan complete → APK build started | 83 | 2.817 min | 9.883 min |
| Longer emulator job execution | 83 | 7.350 min | 8.233 min |
| Skipped tools complete − plan complete | 68 | 0 s | 1 s |

Tools were skipped in 68 runs, executed successfully as `tools / linux` in four,
and genuinely absent from 11 older workflow recipes. In the four executed-tools
runs, plan-complete → build-created has p50/p90 **516.5/1375 s**, n=4; this small
subgroup is descriptive, not a target sample. The planner's fallback builder
must remain available when the immutable CLI is missing.

The latest 20 eligible **PR runs** have actual whole-run p50/p90
**27.008/41.067 min**, build execution **7.033/10.950 min**, and build runner
queue **3.117/6.083 min**. Eighteen skip tools; two execute it. All 20 have the
separate streams job. The wider sample includes 13 older runs that encoded
streams within the build; these recipes are disclosed rather than assumed equal.

These are run observations, not 83 or 20 statistically independent PRs. The 83
runs have 83 distinct API head SHAs but only 63 head branches. The recent 20 have
20 distinct API heads and 16 branches. API PR-number associations are missing
for 82/83 and 19/20 runs, respectively, so exact distinct-PR counts cannot be
inferred. The report exposes known PR associations, branches and repeated API
heads instead of discarding legitimate new runs of the same commit or PR.

Quantiles use the conventional median and nearest-rank p90, `ceil(0.9*n)`.
Whole time is run creation through the latest successful job completion,
including the aggregate and protected aliases. Queue time is build-job creation
through start; dependency intervals use each run's actual plan/tools completion
and build creation. The last selected lane excludes the aggregate/protected
aliases. No interval is inferred by subtracting unrelated medians.

**Opportunity 1:** the skipped dependency itself adds 0–1 s in these samples.
The separate per-run build-creation and runner-queue intervals support retaining
the current dependency rather than adding alternate Validation branches merely
to bypass a skipped tools job. This does not erase real executed-tools work.

## Conditional eager scenario, not an implementation bound

Timestamped logs from the latest 20 eligible runs provide 40 lane observations:

| Observed bootstrap interval | Lanes | p50 | p90 |
| --- | ---: | ---: | ---: |
| Launcher starts → SDK ready/emulator booted | 40 | 94.755 s | 101.800 s |
| Booted → first install | 40 | 52.683 s | 57.103 s |
| Launcher starts → first install | 40 | 148.167 s | 155.101 s |
| Checkout complete → launcher starts | 40 | 14.5 s | 20 s |

The postboot interval includes the existing 45-second reliability settle and
probe preparation. These measurements do not justify removing that protection.

For each run and form, let `B` be the APK build start, `R` the later APK/streams
finish, `P` the observed launcher-to-first-install duration, `I` the exact
first-install log timestamp, and `E` that emulator job's observed finish.
The conditional scenario uses one consistent eager schedule:

```text
ideal_install = max(R, B + P)
ideal_finish  = ideal_install + (E - I)
idle_seconds  = max(0, R - (B + P))
```

This assumes both runners start at `B`, zero new runner queue, zero prelaunch
and artifact-handoff cost, and that all observed preinstall work can be reordered
before inputs arrive. It preserves actual input availability, both observed
post-install tails (including assertions), and every other selected lane's
finish. The latest selected-lane finish is recomputed **within each run** before
saving quantiles are taken. API step times locate the correct log interval;
exact log timestamps avoid second-rounding errors in the post-install tail.

Under those conditions only, selected-lane critical savings are
**34.5/205.318 s** at p50/p90, n=20. Summed additional idle occupancy across the
two runners is **9.140/17.031 runner-minutes**, n=20. This scenario removes the
observed post-input queue/prelaunch delay rather than silently retaining it
while assuming an earlier runner start. The previous hide-only replay is not
used as an eager-boot bound. Neither result is an observed after measurement,
a realizable upper bound, or a prediction of whole-PR wall time. The idle metric
counts waiting after bootstrap for producer inputs; it is not net billed runner
time under the scenario's zero-overhead assumptions.

**Opportunity 2:** the default sequence remains unchanged as an engineering
choice for this PR. The conditional gain/cost illustrates the tradeoff but does
not prove overlap worthless or establish a real speedup. Earlier runner
occupancy can change queueing and contention for other required lanes; a real
implementation must also preserve cold-build, cancellation and input-identity
contracts. Same-job overlap has only 14.5/20 s of observed prelaunch work to
hide, before KVM ordering constraints, and would add owned-background cleanup
complexity. A separately scoped pilot and actual after-runs remain in #334;
no scheduling architecture or APK input-hash cache is introduced here.

## Reproduce and retain the fixed evidence

The durable private bundle on SkyWalker is
`~/.codex/worktree-archives/patina-334-baseline-20261125T134538Z/`.
Its `capture-manifest.json` records all 100 candidate IDs, the selected 83 and
recent 20 IDs, and SHA-256 hashes of all 141 raw run/job/log files. The original
run list is `pr-runs-page1.json`, raw SHA-256:

```text
c2acb568b2a0faf1603646b3b3800ef3b9eb36c2cd6455ea5a04805c681fe1e3
```

The analyzer also emits candidate/selected IDs, API total versus retrieved
counts, sampling strategy, a canonical snapshot digest, job-file digests and
per-form log digests. CLI reports additionally record the raw source-file digest.
Reports bind the analyzer source digest too; comparisons require the same
analyzer version, so re-analyze both captures after a methodology change.
These distinguish the retained snapshot from a later moving latest-100 page.
For this original bounded sample, explicitly select `fixed-candidates`:

```sh
PATINA_TIMELINE_DATA="$HOME/.codex/worktree-archives/patina-334-baseline-20261125T134538Z"
python3 tools/ci/validation-timeline.py report \
  --runs "$PATINA_TIMELINE_DATA/pr-runs-page1.json" \
  --jobs-directory "$PATINA_TIMELINE_DATA" --logs-directory "$PATINA_TIMELINE_DATA" \
  --sampling fixed-candidates --phase-runs 20 \
  --since 2026-11-23T00:00:00Z --until 2026-11-26T00:00:00Z \
  > "$PATINA_TIMELINE_DATA/before.json"
```

Add `--latest 20` for the recent-run view. Raw snapshots remain outside the
repository. The offline analyzer needs no credentials, executes no CI, and
modifies no artifacts. Phase logs must lie inside the matching launcher step
and declare that form's `AVD_TARGET` and `AVD_PROFILE`; swapped, missing or stale
logs are rejected rather than treated as zero-cost work. Mixed run/attempt/head
identity, incomplete job pages and inconsistent times fail closed. Retried runs
are excluded; allowed-failure job outcomes are explicitly unsupported rather
than silently undercounting a successful workflow's elapsed time.

## Capture a new cohort

The default `complete-capture` strategy requires `total_count` to equal the
retrieved run count. Capture every API page for an explicitly chosen query;
use a `created=` filter and narrower windows if API limits prevent completion.
Do not label a first page as a complete date window. For example:

```sh
PATINA_TIMELINE_DATA=$(mktemp -d)
gh api --paginate --slurp \
  'repos/teng-lin/patina-iptv/actions/workflows/validation.yml/runs?event=pull_request&status=success&created=2026-11-23..2026-11-25&per_page=100' \
  > "$PATINA_TIMELINE_DATA/run-pages.json"
python3 - "$PATINA_TIMELINE_DATA" <<'PY'
import json, subprocess, sys
from pathlib import Path
root = Path(sys.argv[1])
pages = json.loads((root / 'run-pages.json').read_text())
runs = [run for page in pages for run in page['workflow_runs']]
assert all(page['total_count'] == pages[0]['total_count'] for page in pages)
assert len(runs) == len({run['id'] for run in runs}) == pages[0]['total_count']
(root / 'runs.json').write_text(json.dumps({'total_count': pages[0]['total_count'], 'workflow_runs': runs}))
for run in runs:
    if run['run_attempt'] != 1:
        continue
    path = f"repos/teng-lin/patina-iptv/actions/runs/{run['id']}/attempts/1/jobs?per_page=100"
    pages = json.loads(subprocess.check_output(['gh', 'api', '--paginate', '--slurp', path]))
    jobs = [job for page in pages for job in page['jobs']]
    (root / f"jobs-{run['id']}-attempt1.json").write_text(json.dumps({
        'total_count': pages[0]['total_count'], 'jobs': jobs}))
PY
python3 tools/ci/validation-timeline.py report \
  --runs "$PATINA_TIMELINE_DATA/runs.json" --jobs-directory "$PATINA_TIMELINE_DATA" \
  --since 2026-11-23T00:00:00Z --until 2026-11-26T00:00:00Z \
  > "$PATINA_TIMELINE_DATA/report.json"
```

For phase evidence, save each selected run's two smoke-job logs with
`gh api --allow-escape-sequences repos/teng-lin/patina-iptv/actions/jobs/JOB_ID/logs`
to `smoke-RUN_ID-tablet.log` and `smoke-RUN_ID-tv.log`, then add
`--logs-directory "$PATINA_TIMELINE_DATA" --phase-runs 20`. Keep escape-bearing
raw logs out of terminals and commits. The installed GitHub CLI used for this
capture was 2.102.0; its `api --help` documents `--allow-escape-sequences`.

A successful `smoke / build` job establishes producer success under the known
workflow, not APK byte identity. The API's PR `head_sha` is not an APK digest
or necessarily the checkout's merge SHA. A future fast path must still bind
fresh APK bytes to the checked-out commit, producer run/attempt and digest,
handle failed-job reruns, fail closed on stale inputs, cancel promptly when the
producer fails, retain cold-build deadlines, and exercise both emulator lanes.

## Actual before/after evidence still required

Collect at least 20 actual successful PR runs in each comparable cohort after a
justified implementation. Distinct run IDs count as run observations; repeated
API heads or PRs are disclosed, not called independent samples. Do not generate
empty PRs, close/reopen loops or reruns to manufacture volume. An explicit
comparability assessment must consider that clustering as well as lane/recipe
mix, runner/cache conditions, APK identity and the intervention revision.
Evaluate failed and cancelled runs separately for reliability.

```sh
python3 tools/ci/validation-timeline.py compare \
  --before before.json --after after.json --comparable
```

Comparison requires explicit consistent sampling strategies, disjoint and
chronologically ordered actual run cohorts, and the known first-attempt,
successful PR/tablet/TV selection contract. Fixed candidate samples must have
the same capture width; their observed medians are sample descriptions, not
complete-window population claims. Models never count as after observations.
Only with at least 20 runs per cohort and the explicit comparability assessment
does the tool report whether the observed after median is at most 18 minutes.
That is descriptive evidence, not proof of causation. #334 remains open for
actual fast-path work, subsequent measurements and its other PR opportunities.
