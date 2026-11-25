# PR critical-path evidence

Refs #334. No Validation scheduling or Android smoke execution changes are made
by this measurement work. The fresh APK build and its same-run artifact handoff
to **both** tablet and TV emulators remain intact. The ≤18-minute target is not
claimed: there are no post-change scheduling samples.

## Observed baseline

The snapshot contains the latest 100 successful pull-request Validation runs
captured at 2026-11-25 13:45:38 UTC. Of these, 83 were completed first attempts with
a successful APK build and both successful emulator lanes. Eleven lacked that
complete smoke selection; six were retried runs. Eligible creation times run
from 2026-11-23 21:15:46 to 2026-11-25 12:29:41 UTC. This fixed snapshot is a
subset of the issue's broader historical cohort, not a replacement for it.

| Actual interval | n | p50 | p90 |
| --- | ---: | ---: | ---: |
| Whole PR run | 83 | 29.967 min | 51.350 min |
| APK build execution | 83 | 7.183 min | 11.333 min |
| APK build runner queue | 83 | 2.533 min | 8.767 min |
| Plan complete → APK build start | 83 | 2.817 min | 9.883 min |
| Longer emulator job execution | 83 | 7.350 min | 8.233 min |
| Skipped tools complete − plan complete | 68 | 0 s | 1 s |

The latest 20 eligible PRs are a more recent view, with actual whole-run
p50/p90 **27.008 / 41.067 minutes**, build execution **7.033 / 10.950 minutes** and build runner
queue **3.117 / 6.083 minutes**. Eighteen of those have skipped tools, again 0 / 1 s.
All 20 have the separate streams job; the wider cohort of 83 runs includes 13 older
runs that encoded streams within the build. Report the recipe mix rather than
assuming those workflow versions are identical.

Quantiles use the conventional median and nearest-rank p90, `ceil(0.9*n)`.
Whole-run time is run creation through the latest successful job completion,
including the aggregate and protected aliases. Queue time is job creation
through its start. Build execution is start through completion. Dependency
wait is measured separately; it is not inferred by subtracting unrelated
medians. The last selected lane excludes the aggregate/protected aliases.

**Opportunity 1: retain the dependency.** A skipped tools job completed at
essentially the plan's completion; the observed multi-minute build-start delay
is largely runner queueing. These data do not justify adding alternate
Validation branches to bypass a 0–1-second dependency.

## Emulator overlap bound, not an after measurement

Timestamped logs from the latest 20 eligible PRs provide 40 emulator observations:

| Observed bootstrap interval | n | p50 | p90 |
| --- | ---: | ---: | ---: |
| Launcher starts → SDK ready/emulator booted | 40 | 94.755 s | 101.800 s |
| Booted → first install | 40 | 52.683 s | 57.103 s |
| Launcher starts → first install | 40 | 148.167 s | 155.101 s |
| Checkout complete → launcher starts | 40 | 14.5 s | 20 s |

The postboot interval includes the existing 45-second reliability settle and
probe preparation. Removing that protection is not an optimization supported
by these measurements.

For each PR separately, the optimistic model subtracts each lane's observed
preinstall work (capped by APK build duration) from that lane's finish, then takes the maximum with every
other selected lane's unchanged finish. Only then are saving quantiles
computed. This idealized full-bootstrap overlap has whole-critical-path savings
p50/p90 **34.5 / 142.750 s**, n=20, because another lane often becomes critical.
It does not hide the smoke assertions or eliminate the whole seven-minute job.

If both emulator runners instead occupy slots during the APK build, the
corresponding idealized extra idle time is **9.140 / 17.031 runner-minutes** at
p50/p90, n=20. This assumes immediate runner availability and ignores changed
queueing/cancellation/contention; it is a tradeoff bound, not a cost forecast.
Earlier runner occupancy could delay other required lanes. Same-job bootstrap
overlap has only 14.5 / 20 s of pre-launch work to hide, even before KVM ordering
constraints, and would need background ownership/cleanup changes.

**Opportunity 2: retain the default sequence.** The measured bounds do not
justify default eager boot or an owned-background-process redesign here.
A later latency experiment needs an explicit runner-budget decision and actual
post-change evidence. APK input-hash caching is a different, broader opportunity
and is not introduced as a substitute.

## Reproduce the report

Raw job/log snapshots stay outside the repository. This offline analyzer does
not need credentials, execute CI, or modify artifacts. It fails on incomplete
job pagination, mixed run/attempt/head identity and inconsistent first-attempt times.
A rerun is excluded, rather than counting inherited/replayed jobs as a fresh
independent sample. All observed job conclusions must be success or skipped;
allowed-failure runs are explicitly unsupported rather than silently reporting
a shorter whole-run duration. A skipped reusable placeholder may share a display name
with the successful protected Android alias.

Capture a fixed candidate list with authenticated `gh`, then save all job pages
for each first-attempt candidate. These commands do not print tokens or logs:

```sh
PATINA_TIMELINE_DATA=$(mktemp -d)
gh api 'repos/teng-lin/patina-iptv/actions/workflows/validation.yml/runs?event=pull_request&status=success&per_page=100' > "$PATINA_TIMELINE_DATA/runs.json"
python3 - "$PATINA_TIMELINE_DATA" <<'PY'
import json, subprocess, sys
from pathlib import Path
root = Path(sys.argv[1])
for run in json.loads((root / 'runs.json').read_text())['workflow_runs']:
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
  > "$PATINA_TIMELINE_DATA/before.json"
```

Choose windows appropriate to the intervention. For the recent comparison, add
`--latest 20`. To include bootstrap bounds, save each selected run's two smoke
job logs with `gh api --allow-escape-sequences
repos/teng-lin/patina-iptv/actions/jobs/JOB_ID/logs` to
`smoke-RUN_ID-tablet.log` and `smoke-RUN_ID-tv.log`, then add
`--logs-directory "$PATINA_TIMELINE_DATA" --phase-runs 20`. Logs must include
the launcher, boot and first-install markers; missing phase evidence is an
error, not a zero-cost observation. Keep raw escape-bearing logs out of the
terminal and out of commits.

A successful job named `smoke / build` establishes producer success under the
known workflow, not APK byte identity by itself. The report records the API's
PR `head_sha` for cohort provenance. That is not an APK hash or necessarily the default checkout's PR merge SHA; this tool does
not assert artifact identity. A future overlap implementation must still bind
the fresh APK bytes to the checked-out commit, producer run/attempt and digest,
handle failed-job reruns, fail closed on stale/missing inputs, cancel promptly
on producer failure, retain cold-build deadlines, and run both emulator lanes.

## Before/after evidence still required

Collect at least 20 actual successful PR observations in **each** comparable
cohort after a justified implementation. Preserve the lane mix, runner/cache conditions and APK identity recipe;
identify the intervention revision. Evaluate failed and cancelled runs separately; successful-only distributions cannot establish
reliability. Do not generate empty PRs or rerun one revision to inflate the
sample. The observational baseline and optimistic model are not post-change
observations.

```sh
python3 tools/ci/validation-timeline.py compare \
  --before before.json --after after.json --comparable
```

The comparison remains pending unless both actual cohorts have at least 20 distinct
runs, do not overlap, are temporally ordered, and retain the same selection
contract. `--comparable` is the owner's explicit assessment, not something the
tool infers from timestamps. Only then does it report whether the observed
after median is at most 18 minutes. That descriptive threshold is not proof that a
particular scheduling change caused the difference. #334 remains open for a
justified fast path, later measurements and its other separately scoped
opportunities.
