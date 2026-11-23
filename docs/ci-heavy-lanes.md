# Proposal for scheduling expensive pull request checks

Issue #241 measured 104 PR runs between November 22 and 23, 2026 UTC. A newer push canceled 61 runs. Canceled runs used 3,862 runner-minutes and successful runs used 3,888. Successful PR runs had a 24.9-minute median and a 37.8-minute 90th percentile.

| Option | Effect on PR turnaround | Effect on runner use |
|---|---|---|
| Run packaging, installer, coverage, and emulator smoke in the merge queue and on nightly runs | The PR gets host, Windows native, and Android compile feedback sooner. The full heavy suite runs before the merge queue admits the commit. | Removes heavy jobs from intermediate pushes. Savings depend on the number of merge-queue retries. |
| Put a short preflight before those lanes | Adds the preflight duration on PRs whose heavy jobs would otherwise start immediately. Pilot target is at most three minutes at p90. | Avoids starting heavy jobs on pushes that fail formatting, compile checks, or core tests before the gate. |
| Put full host validation before those lanes | The 14–20 minute Linux host job becomes serial for PRs needing a package or smoke run. Expect roughly 15 additional minutes in the warm case. | Avoids heavy jobs on pushes that fail host validation or are superseded while host validation runs. |

Start by confirming branch protection requires the aggregate validation result for merge groups. The merge-queue option should run all checks selected for the same changed files and require their results through the aggregate gate. Keep nightly and standalone dispatch available. Adopt the short preflight if merge-queue coverage is unavailable or reviewers need full package results before queueing.

The 3,862 canceled runner-minutes are an upper bound on savings in a sample of this size, about half the combined runner time of canceled and successful PRs. Each option still starts lightweight checks, and the merge queue spends runner time on its own builds. The available totals exclude a heavy-lane breakdown, so the pilot should also record runner-minutes by lane before using the sample to forecast savings.

Run the selected policy for a comparable sample. Report canceled runner-minutes, the share of all PR runner-minutes consumed by canceled runs, median and p90 time to first actionable result, and merge-queue wait. Use an absolute head SHA and the final attempt for elapsed time. Include all attempts for billed runner-minutes and distinguish scheduler cancellations from superseded pushes.
