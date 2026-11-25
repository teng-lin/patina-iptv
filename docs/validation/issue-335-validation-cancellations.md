# Validation cancellation measurements

The cancellation share fell from **27.88% to 22.78%** across all Validation
events in the refreshed baseline. It remains 7.78 percentage points above
the 15% threshold. Drafts defer the selected heavy lanes. The timing changes
considered here leave the cancellation share above target in replay and
can increase PR wall time. The required lanes and dependencies stay in
place. [Issue #335](https://github.com/teng-lin/patina-iptv/issues/335)

## Observed runner time

The cohort contains completed runs of `Validation` created in UTC from
November 17, 2026 through November 25 at 12:58:57, when #335 was opened.
The recent period starts November 23 at 00:00 UTC. Metadata was extracted
on November 25 from 13:45 to 13:54 UTC. The figures use the terminal run
conclusion at extraction and every listed job attempt, including
successful jobs in runs that ended cancelled.

| UTC period and events | Runs | Cancelled runs | Total job-min | Job-min in cancelled runs | Share |
| --- | ---: | ---: | ---: | ---: | ---: |
| Nov 17–22, all | 376 | 162 | 44,371.6 | 12,371.3 | 27.88% |
| Nov 17–22, PR | 276 | 114 | 26,621.7 | 6,894.1 | 25.90% |
| Nov 23–25 through cutoff, all | 473 | 280 | 33,721.7 | 7,682.7 | 22.78% |
| Nov 23–25 through cutoff, PR | 389 | 246 | 19,986.2 | 7,100.6 | 35.53% |
| Nov 25 through cutoff, all | 172 | 112 | 8,801.1 | 1,554.9 | 17.67% |
| Nov 25 through cutoff, PR | 150 | 106 | 4,973.5 | 1,554.9 | 31.26% |

A job-minute is 60 seconds between the recorded start and finish of a
runner-assigned job. Queued work, skipped jobs, and jobs without an
observed runner interval contribute zero. This is unweighted execution
time and excludes runner queueing, platform billing multipliers and minute
rounding. The fixed cutoff and explicit attempts rule define this refresh
of the issue's approximate 31% and 27% baselines. The comparison
is descriptive. The PR-only share increased from 25.90% to 35.53%. Six
cancelled November 25 pushes had zero observed runner-minutes. In the
recent period, 267.8 cancelled job-minutes ran on earlier attempts of two
runs whose final conclusion was cancelled.

Of the recent cancelled runner-minutes, 1,067.9 were macOS and 1,298.9
were Windows. PRs account for 2,164.9 minutes of that combined total.
Removing every one of those PR macOS and Windows minutes at zero cost
would leave 17.49% cancelled across all Validation runs and 27.70% across
PR runs alone. The issue's target applies to all Validation runs. Holding
the rest of the workload fixed, getting below 15% requires saving more
than 3,087.6 cancelled job-minutes. The equivalent PR-only threshold is
4,826.6 minutes. With cancelled minutes `C`, total minutes `T` and savings
`s`, the share is `(C - s) / (T - s)`.

## Observed draft behavior

The latest attempt of each recent PR run is classified from the
`DRAFT` input in the plan log or the `draft` field in the gate's logged
`PLAN`. Missing telemetry stays unknown. It can result from cancellation
before a plan runs or a workflow revision without the field.

| Result | Verified draft | Verified ready | Unknown |
| --- | ---: | ---: | ---: |
| Cancelled | 68 | 52 | 126 |
| Failed | 15 | 9 | 3 |
| Successful | 0 | 85 | 31 |
| All | 83 | 146 | 160 |
| Job-minutes on latest attempts | 1,729.2 | 9,933.7 | 7,071.0 |

The state is unknown for 41.1% of the recent PR runs and 51.2% of the
cancelled PR runs. Of the 126 cancelled runs with unknown state, 79
recorded positive runner time. Earlier attempts contributed another
1,252.35 PR job-minutes outside the state table.

Of the 83 verified drafts, 80 recorded the gate plan. Seventy-four listed
at least one selected lane in `deferred`, including 71 smoke, 48 coverage,
9 desktop package and 2 Windows package deferrals. No corresponding heavy
leaf job started in any verified draft. The native Windows and macOS lanes
still had 60 and 52 runner-assigned jobs on verified drafts, respectively.

Across 12 same-branch pairs with matching base and head, the successful
ready runs spent a median 37.92 minutes in jobs their matched drafts had
deferred (range 21.50–113.90). One pair planned base `af42cd07` and head
`70b39b06`. Its draft listed all four lanes as deferred, and its ready run
passed all four with 92.1 job-minutes in their leaf jobs. The cancelled
draft's own saving would depend on when those jobs would have started.
[Draft run](https://github.com/teng-lin/patina-iptv/actions/runs/45512955817) ·
[Ready run](https://github.com/teng-lin/patina-iptv/actions/runs/45512986882)

## Timing replay

This replay holds the captured queue intervals, job durations, and selected
lanes and run conclusions fixed. It uses the 79 successful, verified-ready
PR runs with one attempt to calculate wall percentiles. The smoke subset
contains 70 of those runs with both emulators successful. Wall time is
from run creation to the last observed runner completion. The cost replay
changes single-attempt PRs with an observed plan finish, including runs of
unknown draft state. Other runs and their minutes stay in the baseline.

| Replay, minutes | Ready wall p50 / p90 | Smoke-selected PR wall p50 / p90 | Gross cancelled minutes avoided | Extra minutes, cancelled / all | Cancelled share |
| --- | ---: | ---: | ---: | ---: | ---: |
| Observed | 27.45 / 47.96 | 29.47 / 48.51 | — | — | 22.78% |
| Hold macOS/Windows until existing helpers finish | 29.12 / 49.50 | 29.98 / 51.44 | 171.1 | 0.0 / 0.0 | 22.39% |
| Hold macOS/Windows for helpers and a new 2.5-minute Linux check | 29.12 / 49.50 | 30.09 / 51.44 | 318.0 | 347.4 / 652.4 | 22.65% |
| Delay the plan outputs 60 seconds | 28.45 / 48.96 | 30.47 / 49.51 | 319.7 | 168.3 / 302.3 | 22.35% |
| Delay the plan outputs 90 seconds | 28.95 / 49.46 | 30.97 / 50.01 | 538.0 | 247.4 / 448.4 | 21.98% |

The OS-only gate targets the runner jobs labeled macOS or Windows. The
new Linux check starts immediately after `plan`, alongside `helpers`.
An affected job's creation moves to the later of its observed creation
and the gate finish, keeping its observed time from creation to start.
For runs without an observed helpers finish, the replay uses the plan
finish for that timestamp. The aggregate and compatibility checks move
only when the OS delay extends the latest prerequisite finish. The model
gives the extra check zero queue time and charges it only when an affected
lane already reached a runner. Adding the check increases cancelled time
by 29.4 minutes and total runner time by 334.4 minutes, even as the share
falls slightly. It increases wall time on 18 of 79 ready runs, by as much
as 9.48 minutes.

If existing queue time absorbs the gate with the 2.5-minute check up to
the original job start, overall p50 stays at 27.45 minutes and
smoke-selected p50 becomes 29.48 minutes. The model then avoids 235.2
gross cancelled minutes and the cancelled share rises to 22.83%.
Selective OS gating would require changes to the shared host matrix to
keep Linux work independent.

The plan-delay rows apply the wait to every eligible PR as a sensitivity
case. REST records these events under `pull_request` without the
`synchronize` subtype. The rows measure a full-exposure scenario. The
fraction of live runs affected by a synchronize-only policy is unknown.
Hosted wait time counts in the plan job. For cancelled runs the model
uses the last observed runner completion as cutoff for the shifted
intervals, and caps extra check or wait time at that cutoff.
Clip each shifted runner interval at that cutoff. With gross cancelled
savings `s`, extra cancelled minutes `e_c` and extra total minutes `e_t`,
the modeled share is `(C - s + e_c) / (T - s + e_t)`. For example, the
90-second scenario saves a net 290.6 minutes in cancelled runs and 89.6
minutes overall.
Actual cancellation initiation and changes in runner availability are
unobserved.

In this smoke subset, build start is +5.46/+13.42 minutes at p50/p90,
build finish is +13.78/+27.43, and the later emulator finishes at
+22.69/+37.40. By construction, the OS-only replay leaves these offsets
unchanged while total smoke-selected wall time can still grow. A delay in
`plan` shifts the chain from `plan/tools` through build and emulator by
1.0 or 1.5 minutes for each
affected successful run in the replay. Other prerequisites can still
determine the final wall time. This cohort uses independently verified
ready-run state and its own cutoff.
[Original #334 cohort](https://github.com/teng-lin/patina-iptv/issues/334)

## Evidence sources and reproduction

Use `gh api --paginate` on
`repos/teng-lin/patina-iptv/actions/workflows/validation.yml/runs?per_page=100&created=YYYY-MM-DD`
for each UTC day and filter `created_at` at the issue's cutoff. Paginate
`actions/runs/ID/jobs?per_page=100&filter=all` for each run ID and preserve
`run_attempt`, timestamps and `runner_id`. The four-attempt master run
`45125353350` uses paginated `actions/runs/ID/attempts/N/jobs` calls because
the combined endpoint returned HTTP 502. Omitting that successful run
would raise the recent all-event share to 23.52%. Plan and gate job logs
come from `actions/jobs/ID/logs`. For the exact-head example above, the
plan jobs are `138865072811` and `138865324197`, and the gate jobs are
`138865214642` and `138877258061`. Percentiles use inclusive linear
interpolation. Rounding is applied after summing seconds. The reproduction
uses the formulas above with the downloaded metadata.

This is a baseline and replay, with no live comparison for either timing
change. The cancellation target and the no-rise condition remain open.
Any rollout needs before/after runner and wall measurements on comparable
ready PRs, with the smoke critical path measured separately.
