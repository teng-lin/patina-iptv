#!/usr/bin/env python3
"""Report PR Validation timelines from immutable run/job snapshots, without running CI."""
import argparse
from collections import Counter
from datetime import datetime
import hashlib
import json
import math
from pathlib import Path
import re
import statistics

SCHEMA = 2
GATES = {"validation", "test (ubuntu-24.04)", "test (macos-15)", "android"}
FORMS = ("tablet", "tv")
AVDS = {"tablet": ("google_apis", "pixel_tablet"), "tv": ("android-tv", "tv_1080p")}
SAMPLING = ("complete-capture", "fixed-candidates")


def timestamp(value):
    result = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if result.utcoffset() is None:
        raise ValueError("timestamps must include a timezone")
    return result.timestamp()


def distribution(values):
    values = sorted(values)
    if not values:
        return {"n": 0}
    return {"n": len(values), "p50": statistics.median(values),
            "p90": values[math.ceil(0.9 * len(values)) - 1]}


def load(path):
    return json.loads(Path(path).read_text())


def capture(source, strategy):
    runs, total = source["workflow_runs"], source["total_count"]
    if type(total) is not int or total < len(runs):
        raise ValueError("run capture total_count is inconsistent")
    if strategy not in SAMPLING:
        raise ValueError("unknown sampling strategy")
    if strategy == "complete-capture" and total != len(runs):
        raise ValueError("run capture is truncated: paginate all runs or explicitly select fixed-candidates sampling")
    ids = [run["id"] for run in runs]
    if len(ids) != len(set(ids)):
        raise ValueError("duplicate run IDs in candidate capture")
    digest = hashlib.sha256(json.dumps(source, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return runs, {"strategy": strategy, "api_total_count": total, "retrieved_count": len(runs),
                  "capture_complete": total == len(runs), "candidate_run_ids": ids,
                  "canonical_snapshot_sha256": digest,
                  "digest_encoding": "UTF-8 compact JSON with sorted keys"}


def phases(text, began=None, ended=None, form=None):
    """Use the first actual launcher invocation, not earlier helper-test output."""
    markers = ("##[group]Run python3 tools/ci/android/ci-emulator.py", "Emulator booted in ",
               "== install and first start")
    found = []
    for line in text.splitlines():
        if len(found) == len(markers):
            break
        date, separator, message = line.partition(" ")
        if separator and markers[len(found)] in message:
            value = timestamp(date)
            if found and value < found[-1]:
                raise ValueError("smoke phase timestamps are not monotonic")
            found.append(value)
    if len(found) != len(markers):
        raise ValueError("smoke log lacks a complete launcher/boot/first-install sequence")
    if (began is not None and found[0] < began) or (ended is not None and found[-1] > ended):
        raise ValueError("smoke phase evidence is outside the identified job")
    if form is not None:
        observed = {"AVD_TARGET": set(), "AVD_PROFILE": set()}
        for line in text.splitlines():
            date, separator, message = line.partition(" ")
            match = re.fullmatch(r"\s*(AVD_TARGET|AVD_PROFILE): (\S+)\s*", message)
            if separator and match and (began is None or timestamp(date) >= began) and (ended is None or timestamp(date) <= ended):
                observed[match[1]].add(match[2])
        target, profile = AVDS[form]
        if observed != {"AVD_TARGET": {target}, "AVD_PROFILE": {profile}}:
            raise ValueError(f"smoke log AVD identity does not match {form}")
    return {"launcher_timestamp": found[0], "first_install_timestamp": found[2],
            "prepare_boot_seconds": found[1] - found[0],
            "postboot_to_install_seconds": found[2] - found[1],
            "preinstall_seconds": found[2] - found[0]}


def summarize(rows):
    heads = Counter(row["head_sha"] for row in rows)
    branches = Counter(row["head_branch"] for row in rows if row["head_branch"] is not None)
    prs = Counter(number for row in rows for number in row["pull_request_numbers"])
    keys = sorted({key for row in rows for key in row["actual"]})
    return {"n": len(rows), "distinct_api_head_shas": len(heads),
            "repeated_api_head_runs": sum(n - 1 for n in heads.values()),
            "distinct_known_pr_numbers": len(prs), "repeated_known_pr_occurrences": sum(n - 1 for n in prs.values()),
            "runs_without_pr_number": sum(not row["pull_request_numbers"] for row in rows),
            "distinct_head_branches": len(branches), "repeated_head_branch_runs": sum(n - 1 for n in branches.values()),
            "actual": {key: distribution([row["actual"][key] for row in rows if key in row["actual"]])
                       for key in keys},
            "last_selected_lane": dict(Counter(row["last_selected_lane"] for row in rows)),
            "tools_results": dict(Counter(row["tools_result"] for row in rows)),
            "separate_streams_job": dict(Counter(row["separate_streams_job"] for row in rows))}


def report(source, jobs_directory, logs_directory=None, since=None, until=None, phase_runs=20, latest=None,
           sampling="complete-capture"):
    runs, sampling_info = capture(source, sampling)
    if phase_runs < 0:
        raise ValueError("phase-runs must not be negative")
    if latest is not None and latest < 1:
        raise ValueError("latest must be positive")
    rows, excluded, seen = [], Counter(), set()
    lower = timestamp(since) if since else -math.inf
    upper = timestamp(until) if until else math.inf
    if upper <= lower:
        raise ValueError("until must be after since")
    for run in sorted(runs, key=lambda item: (timestamp(item["created_at"]), item["id"]), reverse=True):
        identity = run["id"]
        if identity in seen:
            raise ValueError(f"duplicate run ID {identity}")
        seen.add(identity)
        start = timestamp(run["created_at"])
        if not lower <= start < upper:
            excluded["outside_window"] += 1
            continue
        if run["event"] != "pull_request" or run["conclusion"] != "success" or run["status"] != "completed":
            excluded["not_successful_completed_pr"] += 1
            continue
        # A rerun can contain inherited jobs with rewritten created_at values.
        # Do not treat it as an independent observation or derive queue time from it.
        if run["run_attempt"] != 1:
            excluded["retried_run"] += 1
            continue
        job_path = Path(jobs_directory) / f"jobs-{identity}-attempt1.json"
        job_bytes = job_path.read_bytes()
        snapshot = json.loads(job_bytes)
        jobs = snapshot["jobs"]
        if snapshot["total_count"] != len(jobs):
            raise ValueError(f"run {identity}: incomplete job pagination")
        if any(job["run_id"] != identity or job["run_attempt"] != 1 or job["head_sha"] != run["head_sha"]
               for job in jobs):
            raise ValueError(f"run {identity}: mixed run or attempt identity")
        if len({job["id"] for job in jobs}) != len(jobs):
            raise ValueError(f"run {identity}: duplicate job IDs")
        named = {}
        for job in jobs:
            previous = named.get(job["name"])
            if previous and previous["conclusion"] != "skipped" and job["conclusion"] != "skipped":
                raise ValueError(f"run {identity}: ambiguous job name {job['name']}")
            # A skipped reusable-job placeholder and the protected Android alias
            # can legitimately have the same display name in one attempt.
            if not previous or previous["conclusion"] == "skipped":
                named[job["name"]] = job
        build = named.get("smoke / build")
        lanes = {form: [job for job in jobs if job["name"].startswith(f"smoke / smoke ({form},")]
                 for form in FORMS}
        if not build or build["conclusion"] != "success" or any(
                len(found) != 1 or found[0]["conclusion"] != "success" for found in lanes.values()):
            excluded["without_successful_build_and_both_emulators"] += 1
            continue
        if "plan" not in named or named["plan"]["conclusion"] != "success":
            raise ValueError(f"run {identity}: missing successful plan")
        if any(job["conclusion"] not in {"success", "skipped"} for job in jobs):
            raise ValueError(f"run {identity}: unsupported non-success/non-skipped job outcome; "
                             "cannot undercount an allowed-failure job")
        successful = [job for job in jobs if job["conclusion"] == "success"]
        for job in successful:
            created, began, ended = (timestamp(job[key]) for key in ("created_at", "started_at", "completed_at"))
            if not start <= created <= began <= ended:
                raise ValueError(f"run {identity}: invalid first-attempt timestamps for {job['name']}")
        plan_end = timestamp(named["plan"]["completed_at"])
        build_start, build_end = (timestamp(build[key]) for key in ("started_at", "completed_at"))
        streams = named.get("smoke / streams")
        if streams and streams["conclusion"] != "success":
            raise ValueError(f"run {identity}: successful smoke has an unsuccessful streams producer")
        ready = max(build_end, timestamp(streams["completed_at"])) if streams else build_end
        smoke = {form: found[0] for form, found in lanes.items()}
        smoke_ends = {form: timestamp(job["completed_at"]) for form, job in smoke.items()}
        selected = [job for job in successful if job["name"] not in GATES]
        last = max(selected, key=lambda job: timestamp(job["completed_at"]))
        tool_jobs = [job for job in jobs if job["name"] == "tools" or job["name"].startswith("tools / ")]
        executed_tools = [job for job in tool_jobs if job["conclusion"] != "skipped"]
        tool_result = "success" if executed_tools else ("skipped" if tool_jobs else "absent")
        tool_end = max(timestamp(job["completed_at"]) for job in (executed_tools or tool_jobs)) if tool_jobs else None
        actual = {
            "whole_minutes": (max(timestamp(job["completed_at"]) for job in successful) - start) / 60,
            "plan_end_minutes": (plan_end - start) / 60,
            "build_start_minutes": (build_start - start) / 60,
            "plan_to_build_created_seconds": timestamp(build["created_at"]) - plan_end,
            "plan_to_build_start_minutes": (build_start - plan_end) / 60,
            "build_runner_queue_minutes": (build_start - timestamp(build["created_at"])) / 60,
            "build_minutes": (build_end - build_start) / 60,
            "smoke_start_after_inputs_minutes": (min(timestamp(job["started_at"]) for job in smoke.values()) - ready) / 60,
            "smoke_max_minutes": max((timestamp(job["completed_at"]) - timestamp(job["started_at"])) / 60
                                     for job in smoke.values()),
            "smoke_end_minutes": (max(smoke_ends.values()) - start) / 60,
        }
        if tool_end is not None:
            actual["tools_to_build_created_seconds"] = timestamp(build["created_at"]) - tool_end
            if tool_result == "skipped":
                actual["skipped_tools_vs_plan_seconds"] = tool_end - plan_end
        if min(actual.values()) < 0:
            raise ValueError(f"run {identity}: negative dependency or queue interval")
        row = {"run_id": identity, "head_sha": run["head_sha"], "created_at": run["created_at"],
               "url": run["html_url"], "head_branch": run.get("head_branch"),
               "pull_request_numbers": sorted({pr["number"] for pr in run.get("pull_requests", [])}),
               "jobs_snapshot_sha256": hashlib.sha256(job_bytes).hexdigest(),
               "actual": actual, "last_selected_lane": last["name"],
               "tools_result": tool_result, "tools_job_names": [job["name"] for job in tool_jobs],
               "separate_streams_job": "present" if streams else "absent"}
        if logs_directory and len(rows) < phase_runs:
            pair, gaps, log_digests = {}, [], {}
            for form, job in smoke.items():
                checkout = next(step for step in job["steps"] if step["name"].startswith("Run actions/checkout@"))
                driver = next(step for step in job["steps"] if step["name"].startswith("Smoke test on the "))
                driver_lower, driver_upper = timestamp(driver["started_at"]), timestamp(driver["completed_at"])
                checkout_end = timestamp(checkout["completed_at"])
                if not timestamp(job["started_at"]) <= checkout_end <= driver_lower <= driver_upper <= timestamp(job["completed_at"]):
                    raise ValueError(f"run {identity}: checkout/launcher chronology is inconsistent for {form}")
                log_bytes = (Path(logs_directory) / f"smoke-{identity}-{form}.log").read_bytes()
                log_digests[form] = hashlib.sha256(log_bytes).hexdigest()
                pair[form] = phases(log_bytes.decode(), driver_lower, driver_upper, form)
                gaps.append(driver_lower - checkout_end)
            others = [timestamp(job["completed_at"]) for job in selected
                      if not job["name"].startswith("smoke / smoke (")]
            old_end = max(*smoke_ends.values(), *others)
            ideal_installs = {form: max(ready, build_start + pair[form]["preinstall_seconds"]) for form in FORMS}
            ideal_finishes = {form: ideal_installs[form] + smoke_ends[form] - pair[form]["first_install_timestamp"]
                              for form in FORMS}
            ideal_end = max(*ideal_finishes.values(), *others)
            row["conditional_eager_model"] = {
                "phases": pair, "log_sha256": log_digests,
                "assumed_input_window_seconds": ready - build_start,
                "ideal_install_timestamps": ideal_installs, "ideal_finish_timestamps": ideal_finishes,
                # Both savings and idle use the SAME conditional eager schedule.
                "critical_saving_seconds": old_end - ideal_end,
                "added_idle_runner_minutes": sum(max(0, ready - build_start - pair[form]["preinstall_seconds"])
                                                 for form in FORMS) / 60,
                "same_job_setup_overlap_upper_bound_seconds": gaps,
            }
        rows.append(row)
        if latest is not None and len(rows) == latest:
            break
    result = {"schema_version": SCHEMA, "kind": "actual_pr_observations",
        "analyzer_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), "selector": {
        "since_inclusive": since, "until_exclusive": until, "latest_eligible": latest, "attempt": 1,
        "event": "pull_request", "conclusion": "success", "both_emulators_required": True,
        "all_jobs_success_or_skipped": True}, "sampling": sampling_info,
        "quantiles": "p50 conventional median; p90 nearest rank ceil(0.9*n)",
        "summary": summarize(rows), "excluded": dict(excluded), "runs": rows}
    result["sampling"]["selected_run_ids"] = [row["run_id"] for row in rows]
    models = [row["conditional_eager_model"] for row in rows if "conditional_eager_model" in row]
    if models:
        result["conditional_eager_model"] = {
            "n_pr_runs": len(models), "n_emulator_lanes": len(models) * 2,
            "assumptions": "Conditional scenario, not an observed gain or implementation bound: both emulator runners "
                           "start at APK build execution with zero new queue/prelaunch/handoff cost; all observed preinstall "
                           "work is assumed reorderable; install cannot precede actual APK/streams readiness; "
                           "observed post-install tails and other lane finishes stay fixed. "
                           "Runner contention, cancellation and implementation overhead are not modeled.",
            "critical_saving_seconds": distribution([model["critical_saving_seconds"] for model in models]),
            "added_idle_runner_minutes": distribution([model["added_idle_runner_minutes"] for model in models]),
            "same_job_setup_overlap_upper_bound_seconds": distribution([
                gap for model in models for gap in model["same_job_setup_overlap_upper_bound_seconds"]]),
            "phases": {key: distribution([pair[key] for model in models for pair in model["phases"].values()])
                       for key in ("prepare_boot_seconds", "postboot_to_install_seconds", "preinstall_seconds")}}
    return result


def compare(before, after, comparable=False):
    for item in (before, after):
        if item.get("schema_version") != SCHEMA or item.get("kind") != "actual_pr_observations":
            raise ValueError("comparison requires actual timeline reports, never model output")
    left, right = before["runs"], after["runs"]
    for rows in (left, right):
        if len({row["run_id"] for row in rows}) != len(rows):
            raise ValueError("comparison requires distinct run IDs in each cohort")
        if any(not math.isfinite(row["actual"]["whole_minutes"]) or row["actual"]["whole_minutes"] < 0
               for row in rows):
            raise ValueError("comparison requires actual nonnegative finite durations")
    for item in (before, after):
        sample = item["sampling"]
        candidates = sample["candidate_run_ids"]
        if (sample["strategy"] not in SAMPLING or sample["retrieved_count"] != len(candidates)
                or sample["api_total_count"] < sample["retrieved_count"] or len(set(candidates)) != len(candidates)):
            raise ValueError("comparison requires an explicit consistent sampling contract")
        if sample["selected_run_ids"] != [row["run_id"] for row in item["runs"]] or not set(sample["selected_run_ids"]) <= set(candidates):
            raise ValueError("comparison rows do not match the captured candidate/selection manifest")
        if sample["strategy"] == "complete-capture" and sample["api_total_count"] != sample["retrieved_count"]:
            raise ValueError("complete-capture comparison cannot use truncated run lists")
    required = {"attempt": 1, "event": "pull_request", "conclusion": "success",
                "both_emulators_required": True, "all_jobs_success_or_skipped": True}
    reason = None
    if any(type(item["selector"][key]) is not type(value) or item["selector"][key] != value
           for item in (before, after) for key, value in required.items()):
        reason = "Both cohorts must use successful first-attempt PR runs with both emulator lanes"
    elif before["analyzer_sha256"] != after["analyzer_sha256"]:
        reason = "Analyzer versions differ; re-analyze both cohorts with the same tool"
    elif before["sampling"]["strategy"] != after["sampling"]["strategy"]:
        reason = "Sampling strategies differ"
    elif before["sampling"]["strategy"] == "fixed-candidates" and before["sampling"]["retrieved_count"] != after["sampling"]["retrieved_count"]:
        reason = "Fixed candidate capture widths differ"
    elif not comparable:
        reason = "Owner has not established cohort/intervention comparability"
    elif len(left) < 20 or len(right) < 20:
        reason = "At least 20 actual PR runs are required in EACH cohort"
    elif {row["run_id"] for row in left} & {row["run_id"] for row in right}:
        reason = "Before and after cohorts overlap"
    elif max(timestamp(row["created_at"]) for row in left) >= min(timestamp(row["created_at"]) for row in right):
        reason = "After observations must follow the before cohort"
    elif any(before["selector"][key] != after["selector"][key]
             for key in ("attempt", "event", "conclusion", "both_emulators_required")):
        reason = "Cohort selection contracts differ"
    observed = {"before": summarize(left)["actual"].get("whole_minutes", {"n": 0}),
                "after": summarize(right)["actual"].get("whole_minutes", {"n": 0})}
    return {"status": "pending" if reason else "measured", "reason": reason, "actual_whole_minutes": observed,
            "after_median_at_most_18_minutes": None if reason else observed["after"]["p50"] <= 18,
            "cohort_repetition": {"before": {key: summarize(left)[key] for key in ("distinct_api_head_shas", "repeated_api_head_runs", "distinct_known_pr_numbers", "runs_without_pr_number")},
                                  "after": {key: summarize(right)[key] for key in ("distinct_api_head_shas", "repeated_api_head_runs", "distinct_known_pr_numbers", "runs_without_pr_number")}},
            "note": "Descriptive PR-run evidence, not statistically independent PRs. Owner comparability assessment must consider "
                    "repeat/clustered revisions, lane mix, runner/cache conditions and intervention; do not manufacture runs. "
                    "Conditional models never count as post-change samples."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    analyze = commands.add_parser("report")
    analyze.add_argument("--runs", type=Path, required=True)
    analyze.add_argument("--jobs-directory", type=Path, required=True)
    analyze.add_argument("--logs-directory", type=Path)
    analyze.add_argument("--since")
    analyze.add_argument("--until")
    analyze.add_argument("--phase-runs", type=int, default=20)
    analyze.add_argument("--latest", type=int, help="Use only the latest N eligible observations")
    analyze.add_argument("--sampling", choices=SAMPLING, default="complete-capture",
                         help="Default requires all run pages; fixed-candidates explicitly declares a bounded sample")
    comparison = commands.add_parser("compare")
    comparison.add_argument("--before", type=Path, required=True)
    comparison.add_argument("--after", type=Path, required=True)
    comparison.add_argument("--comparable", action="store_true",
                            help="Owner has independently established cohort/intervention comparability")
    args = parser.parse_args()
    try:
        if args.command == "report":
            source_bytes = args.runs.read_bytes()
            source = json.loads(source_bytes)
            result = report(source, args.jobs_directory, args.logs_directory,
                            args.since, args.until, args.phase_runs, args.latest, args.sampling)
            result["sampling"].update(source_file=str(args.runs.resolve()),
                                      source_file_sha256=hashlib.sha256(source_bytes).hexdigest())
        else:
            result = compare(load(args.before), load(args.after), args.comparable)
    except (ValueError, KeyError, OSError, StopIteration) as error:
        parser.exit(2, f"Cannot report incomplete or inconsistent evidence: {error}\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
