#!/usr/bin/env python3
"""Generated job snapshots exercise cohort identity, timestamps and paired models."""
import copy
from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("validation-timeline.py")
SPEC = importlib.util.spec_from_file_location("validation_timeline", SCRIPT)
TIMELINE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TIMELINE)


def run_capture(runs, total=None):
    return {"total_count": len(runs) if total is None else total, "workflow_runs": runs}


def form_log(text, form):
    if form == "tv":
        return text.replace("AVD_TARGET: google_apis", "AVD_TARGET: android-tv").replace("AVD_PROFILE: pixel_tablet", "AVD_PROFILE: tv_1080p")
    return text


def generated(identity=1, day=1):
    start = datetime(2026, 1, day, tzinfo=timezone.utc)

    def date(seconds):
        return (start + timedelta(seconds=seconds)).isoformat().replace("+00:00", "Z")

    run = {"id": identity, "event": "pull_request", "status": "completed", "conclusion": "success",
           "run_attempt": 1, "created_at": date(0), "head_sha": f"{identity:040x}",
           "html_url": f"https://example.test/generated-runs/{identity}", "head_branch": f"generated-{identity}",
           "pull_requests": [{"number": identity}]}
    jobs = []

    def job(name, created, began, ended, conclusion="success", steps=()):
        jobs.append({"id": identity * 100 + len(jobs), "run_id": identity, "run_attempt": 1,
                     "head_sha": run["head_sha"], "name": name, "created_at": date(created), "started_at": date(began),
                     "completed_at": date(ended), "conclusion": conclusion, "steps": list(steps)})

    job("plan", 0, 0, 10)
    job("tools", 10, 10, 10, "skipped")
    job("smoke / build", 10, 70, 200)
    job("smoke / streams", 10, 10, 30)
    job("coverage / coverage", 10, 10, 590)
    for form, end in (("tablet", 600), ("tv", 580)):
        job(f"smoke / smoke ({form}, generated, generated)", 200, 220, end, steps=[
            {"name": "Run actions/checkout@generated", "completed_at": date(222)},
            {"name": f"Smoke test on the {form} emulator", "started_at": date(235), "completed_at": date(end - 5)}])
    job("validation", 600, 600, 610)
    job("android", 610, 610, 615)
    # Legitimate skipped reusable placeholder duplicates the protected alias name.
    job("android", 10, 10, 10, "skipped")
    log = (f"{date(225)} Emulator booted in 1s\n"  # earlier helper test output is not boot evidence
           f"{date(235)} ##[group]Run python3 tools/ci/android/ci-emulator.py generated\n"
           f"{date(235)}   AVD_TARGET: google_apis\n"
           f"{date(235)}   AVD_PROFILE: pixel_tablet\n"
           f"{date(280)} Emulator booted in 45s\n"
           f"{date(335)} == install and first start\n")
    return run, {"total_count": len(jobs), "jobs": jobs}, log


class Timelines(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.run, self.jobs, self.log = generated()
        self.write()

    def write(self):
        identity = self.run["id"]
        (self.root / f"jobs-{identity}-attempt1.json").write_text(json.dumps(self.jobs))
        for form in ("tablet", "tv"):
            (self.root / f"smoke-{identity}-{form}.log").write_text(form_log(self.log, form))

    def report(self, **options):
        return TIMELINE.report(run_capture([self.run]), self.root, **options)

    def test_actual_queue_and_dependency_intervals_and_paired_critical_model(self):
        value = self.report(logs_directory=self.root)
        self.assertEqual(value["summary"]["n"], 1)
        row = value["runs"][0]
        self.assertEqual(row["actual"]["whole_minutes"], 615 / 60)
        self.assertEqual(row["actual"]["build_runner_queue_minutes"], 1)
        self.assertEqual(row["actual"]["plan_to_build_start_minutes"], 1)
        self.assertEqual(row["actual"]["skipped_tools_vs_plan_seconds"], 0)
        self.assertEqual(row["last_selected_lane"], "smoke / smoke (tablet, generated, generated)")
        # Both lanes could hide100s, but coverage ends only10s behind tablet.
        model = value["conditional_eager_model"]
        self.assertEqual(model["critical_saving_seconds"]["p50"], 10)
        self.assertEqual(model["added_idle_runner_minutes"]["p50"], 1)
        self.assertEqual(model["phases"]["preinstall_seconds"]["p50"], 100)
        self.assertEqual(model["same_job_setup_overlap_upper_bound_seconds"]["p50"], 13)

    def test_early_producer_still_waits_for_bootstrap_in_one_consistent_eager_schedule(self):
        for job in self.jobs["jobs"]:
            if job["name"] == "smoke / build":
                job["completed_at"] = "2026-01-01T00:01:40Z"
            elif job["name"] == "coverage / coverage":
                job["completed_at"] = "2026-01-01T00:06:40Z"
        self.write()
        value = self.report(logs_directory=self.root)
        row = value["runs"][0]["conditional_eager_model"]
        install = TIMELINE.timestamp("2026-01-01T00:02:50Z")
        self.assertEqual(row["ideal_install_timestamps"], {"tablet": install, "tv": install})
        # Zero new queue/prelaunch removes135s, and30s of bootstrap overlaps production.
        self.assertEqual(value["conditional_eager_model"]["critical_saving_seconds"]["p50"], 165)
        self.assertEqual(value["conditional_eager_model"]["added_idle_runner_minutes"]["p50"], 0)

    def test_skipped_tools_negative_interval_cannot_bypass_final_actual_validation(self):
        next(j for j in self.jobs["jobs"] if j["name"] == "tools")["completed_at"] = "2026-01-01T00:00:09Z"
        self.write()
        with self.assertRaisesRegex(ValueError, "negative dependency"):
            self.report()

    def test_late_streams_preserve_input_availability_and_use_the_same_idle_schedule(self):
        for streams_end in (120, 210):
            with self.subTest(streams_end=streams_end):
                self.jobs = generated()[1]
                origin = datetime(2026, 1, 1, tzinfo=timezone.utc)
                for job in self.jobs["jobs"]:
                    if job["name"] == "smoke / build":
                        job["completed_at"] = "2026-01-01T00:01:40Z"
                    elif job["name"] == "smoke / streams":
                        job["completed_at"] = (origin + timedelta(seconds=streams_end)).isoformat()
                    elif job["name"] == "coverage / coverage":
                        job["completed_at"] = "2026-01-01T00:06:40Z"
                self.write()
                value = self.report(logs_directory=self.root)
                model = value["runs"][0]["conditional_eager_model"]
                window = streams_end - 70  # APK build starts 70 s after run creation.
                install_seconds = max(streams_end, 70 + 100)
                install = origin.timestamp() + install_seconds
                self.assertEqual(model["ideal_install_timestamps"], {"tablet": install, "tv": install})
                self.assertEqual(model["assumed_input_window_seconds"], window)
                self.assertEqual(model["critical_saving_seconds"], 335 - install_seconds)
                self.assertEqual(model["added_idle_runner_minutes"], 2 * max(0, window - 100) / 60)
                self.assertEqual(value["runs"][0]["actual"]["smoke_start_after_inputs_minutes"], (220 - streams_end) / 60)

    def test_truncated_run_pages_require_an_explicit_fixed_candidate_sample(self):
        source = run_capture([self.run], total=100)
        with self.assertRaisesRegex(ValueError, "truncated"):
            TIMELINE.report(source, self.root)
        value = TIMELINE.report(source, self.root, sampling="fixed-candidates")
        self.assertEqual(value["sampling"]["api_total_count"], 100)
        self.assertEqual(value["sampling"]["retrieved_count"], 1)
        self.assertFalse(value["sampling"]["capture_complete"])
        self.assertEqual(value["sampling"]["candidate_run_ids"], [1])
        self.assertEqual(value["sampling"]["selected_run_ids"], [1])
        self.assertEqual(len(value["sampling"]["canonical_snapshot_sha256"]), 64)
        source["total_count"] = 101
        second = TIMELINE.report(source, self.root, sampling="fixed-candidates")
        self.assertNotEqual(second["sampling"]["canonical_snapshot_sha256"], value["sampling"]["canonical_snapshot_sha256"])
        with self.assertRaises(ValueError):
            TIMELINE.capture(run_capture([self.run], total=0), "fixed-candidates")

    def test_executed_nested_tools_and_per_run_dependency_intervals_are_measured(self):
        tools = next(j for j in self.jobs["jobs"] if j["name"] == "tools")
        tools.update(name="tools / linux", conclusion="success", completed_at="2026-01-01T00:00:30Z")
        build = next(j for j in self.jobs["jobs"] if j["name"] == "smoke / build")
        build["created_at"] = "2026-01-01T00:00:31Z"
        self.write()
        row = self.report()["runs"][0]
        self.assertEqual(row["tools_result"], "success")
        self.assertEqual(row["actual"]["plan_to_build_created_seconds"], 21)
        self.assertEqual(row["actual"]["tools_to_build_created_seconds"], 1)
        self.assertNotIn("skipped_tools_vs_plan_seconds", row["actual"])
        self.jobs["jobs"].remove(tools)
        self.jobs["total_count"] -= 1
        self.write()
        self.assertEqual(self.report()["summary"]["tools_results"], {"absent": 1})

    def test_phase_logs_are_bound_to_the_matching_avd_form(self):
        tablet = self.root / "smoke-1-tablet.log"
        tv = self.root / "smoke-1-tv.log"
        tablet_text, tv_text = tablet.read_text(), tv.read_text()
        tablet.write_text(tv_text)
        tv.write_text(tablet_text)
        with self.assertRaisesRegex(ValueError, "AVD identity"):
            self.report(logs_directory=self.root)
        self.write()
        tablet.write_text(tablet_text.replace("AVD_TARGET: google_apis", "unrelated generated output"))
        with self.assertRaisesRegex(ValueError, "AVD identity"):
            self.report(logs_directory=self.root)

    def test_repeated_revision_runs_are_disclosed_without_being_called_independent_prs(self):
        second, jobs, _ = generated(2, day=2)
        second.update(head_sha=self.run["head_sha"], head_branch=self.run["head_branch"], pull_requests=self.run["pull_requests"])
        for job in jobs["jobs"]:
            job["head_sha"] = second["head_sha"]
        (self.root / "jobs-2-attempt1.json").write_text(json.dumps(jobs))
        value = TIMELINE.report(run_capture([self.run, second]), self.root)
        self.assertEqual(value["summary"]["n"], 2)
        self.assertEqual(value["summary"]["distinct_api_head_shas"], 1)
        self.assertEqual(value["summary"]["repeated_api_head_runs"], 1)
        self.assertEqual(value["summary"]["distinct_known_pr_numbers"], 1)
        self.assertEqual(value["summary"]["repeated_known_pr_occurrences"], 1)
        self.assertEqual(value["summary"]["repeated_head_branch_runs"], 1)

    def test_conditional_tail_uses_exact_log_install_time_not_rounded_api_step_start(self):
        for old, new in (("00:03:55Z", "00:03:55.500000Z"), ("00:04:40Z", "00:04:40.500000Z"),
                         ("00:05:35Z", "00:05:35.500000Z")):
            self.log = self.log.replace(old, new)
        next(job for job in self.jobs["jobs"] if job["name"] == "coverage / coverage")["completed_at"] = "2026-01-01T00:06:40Z"
        self.write()
        value = self.report(logs_directory=self.root)
        model = value["runs"][0]["conditional_eager_model"]
        expected = TIMELINE.timestamp("2026-01-01T00:07:44.500000Z")
        self.assertEqual(model["ideal_finish_timestamps"]["tablet"], expected)
        self.assertEqual(model["critical_saving_seconds"], 135.5)

    def test_quantiles_use_median_and_nearest_rank_not_interpolation(self):
        self.assertEqual(TIMELINE.distribution([4, 1, 3, 2]), {"n": 4, "p50": 2.5, "p90": 4})
        self.assertEqual(TIMELINE.distribution([]), {"n": 0})

    def test_filters_date_explicitly_even_when_older_jobs_are_cached(self):
        self.assertEqual(self.report(since="2026-01-02T00:00:00Z")["summary"]["n"], 0)
        self.assertEqual(self.report(until="2026-01-01T00:00:00Z")["excluded"], {"outside_window": 1})
        with self.assertRaises(ValueError):
            self.report(since="2026-01-02T00:00:00Z", until="2026-01-01T00:00:00Z")
        with self.assertRaises(ValueError):
            self.report(since="2026-01-01")

    def test_retry_failure_and_other_events_are_not_independent_successful_pr_observations(self):
        for key, value, reason in (("run_attempt", 2, "retried_run"),
                                   ("event", "push", "not_successful_completed_pr"),
                                   ("conclusion", "failure", "not_successful_completed_pr"),
                                   ("status", "in_progress", "not_successful_completed_pr")):
            with self.subTest(key=key):
                run = dict(self.run, **{key: value})
                result = TIMELINE.report(run_capture([run]), self.root)
                self.assertEqual(result["summary"]["n"], 0)
                self.assertEqual(result["excluded"], {reason: 1})

    def test_both_emulator_lanes_and_successful_fresh_build_are_required(self):
        for name in ("smoke / build", "smoke / smoke (tv, generated, generated)"):
            with self.subTest(name=name):
                original = copy.deepcopy(self.jobs)
                next(j for j in self.jobs["jobs"] if j["name"] == name)["conclusion"] = "skipped"
                self.write()
                self.assertEqual(self.report()["summary"]["n"], 0)
                self.jobs = original

    def test_mixed_attempt_ids_truncated_pages_and_invalid_timestamps_fail_closed(self):
        changes = [lambda value: value.update(total_count=100),
                   lambda value: value["jobs"][0].update(run_id=99),
                   lambda value: value["jobs"][0].update(run_attempt=2),
                   lambda value: value["jobs"][0].update(head_sha="generated wrong revision"),
                   lambda value: value["jobs"][0].update(created_at="2026-01-01T00:01:00Z"),
                   lambda value: value["jobs"][1].update(id=value["jobs"][0]["id"])]
        for change in changes:
            with self.subTest(change=change):
                self.jobs = generated()[1]
                change(self.jobs)
                self.write()
                with self.assertRaises(ValueError):
                    self.report()
        with self.assertRaises(ValueError):
            TIMELINE.report(run_capture([self.run, self.run]), self.root)

    def test_missing_or_reversed_phase_evidence_does_not_become_a_zero_cost_observation(self):
        for text in ("generated truncated log", self.log.replace("00:04:40Z", "00:03:00Z")):
            with self.subTest(text=text), self.assertRaises(ValueError):
                TIMELINE.phases(text)
        self.assertNotIn("conditional_eager_model", self.report())
        with self.assertRaises(ValueError):
            self.report(phase_runs=-1)

    def test_phase_logs_only_add_models_and_never_change_the_multi_run_cohort(self):
        second, jobs, log = generated(2, day=2)
        outside, _, _ = generated(3, day=3)
        (self.root / "jobs-2-attempt1.json").write_text(json.dumps(jobs))
        for form in ("tablet", "tv"):
            (self.root / f"smoke-2-{form}.log").write_text(form_log(log, form))
        options = {"since": "2026-01-01T00:00:00Z", "until": "2026-01-03T00:00:00Z"}
        runs = [self.run, second, outside]
        before = TIMELINE.report(run_capture(runs), self.root, **options)
        after = TIMELINE.report(run_capture(runs), self.root, logs_directory=self.root, **options)
        self.assertEqual(before["summary"]["n"], 2)
        self.assertEqual(before["summary"], after["summary"])
        self.assertEqual(before["excluded"], {"outside_window": 1})
        self.assertEqual(before["excluded"], after["excluded"])
        self.assertEqual([row["actual"] for row in before["runs"]], [row["actual"] for row in after["runs"]])
        self.assertEqual(after["conditional_eager_model"]["n_pr_runs"], 2)
        # A genuine sequence saved under the wrong run filename must also fail.
        (self.root / "smoke-2-tablet.log").write_text(self.log)
        with self.assertRaises(ValueError):
            TIMELINE.report(run_capture(runs), self.root, logs_directory=self.root, **options)

    def test_phase_timestamps_must_belong_to_the_identified_job(self):
        began = TIMELINE.timestamp("2026-01-02T00:00:00Z")
        with self.assertRaises(ValueError):
            TIMELINE.phases(self.log, began=began)
        with self.assertRaises(ValueError):
            TIMELINE.phases(self.log, ended=began - 86400)

    def test_phase_markers_inside_job_but_outside_driver_step_are_rejected(self):
        self.log = self.log.replace("00:03:55Z", "00:03:50Z")
        self.write()
        with self.assertRaises(ValueError):
            self.report(logs_directory=self.root)

    def test_invalid_step_chronology_and_allowed_failures_do_not_understate_elapsed_time(self):
        for started, ended in ((221, 590), (235, 230), (235, 601)):
            with self.subTest(started=started, ended=ended):
                self.jobs = generated()[1]
                job = next(j for j in self.jobs["jobs"] if j["name"].startswith("smoke / smoke (tablet,"))
                origin = datetime(2026, 1, 1, tzinfo=timezone.utc)
                job["steps"][1].update(started_at=(origin + timedelta(seconds=started)).isoformat(),
                                       completed_at=(origin + timedelta(seconds=ended)).isoformat())
                self.write()
                with self.assertRaises(ValueError):
                    self.report(logs_directory=self.root)
        self.jobs = generated()[1]
        next(j for j in self.jobs["jobs"] if j["name"] == "coverage / coverage")["conclusion"] = "failure"
        self.write()
        with self.assertRaisesRegex(ValueError, "allowed-failure"):
            self.report()

    def test_latest_limit_and_legacy_inline_stream_recipe_are_explicit(self):
        older, jobs, _ = generated(2, day=2)
        (self.root / "jobs-2-attempt1.json").write_text(json.dumps(jobs))
        value = TIMELINE.report(run_capture([self.run, older]), self.root, latest=1)
        self.assertEqual([r["run_id"] for r in value["runs"]], [2])
        self.jobs["jobs"] = [j for j in self.jobs["jobs"] if j["name"] != "smoke / streams"]
        self.jobs["total_count"] -= 1
        self.write()
        self.assertEqual(self.report()["summary"]["separate_streams_job"], {"absent": 1})
        with self.assertRaises(ValueError):
            self.report(latest=0)

    def test_cli_reports_from_saved_json_without_network_or_credentials(self):
        runs = self.root / "runs.json"
        runs.write_text(json.dumps(run_capture([self.run])))
        result = subprocess.run([sys.executable, SCRIPT, "report", "--runs", runs,
                                 "--jobs-directory", self.root], check=True, capture_output=True, text=True)
        self.assertEqual(json.loads(result.stdout)["summary"]["n"], 1)

    def comparison(self, count=20):
        base = self.report()
        before, after = copy.deepcopy(base), copy.deepcopy(base)
        row = base["runs"][0]
        before["runs"] = [dict(copy.deepcopy(row), run_id=i, created_at="2026-01-01T00:00:00Z") for i in range(count)]
        after["runs"] = [dict(copy.deepcopy(row), run_id=100 + i, created_at="2026-02-01T00:00:00Z") for i in range(count)]
        for item in (before, after):
            ids = [r["run_id"] for r in item["runs"]]
            item["sampling"].update(api_total_count=count, retrieved_count=count,
                                    candidate_run_ids=ids, selected_run_ids=ids.copy(), capture_complete=True)
        return before, after

    def test_target_evidence_requires_twenty_actual_runs_in_each_comparable_distinct_cohort(self):
        before, after = self.comparison(19)
        self.assertEqual(TIMELINE.compare(before, after, comparable=True)["status"], "pending")
        before, after = self.comparison()
        self.assertEqual(TIMELINE.compare(before, after)["status"], "pending")
        result = TIMELINE.compare(before, after, comparable=True)
        self.assertEqual(result["status"], "measured")
        self.assertTrue(result["after_median_at_most_18_minutes"])
        after["runs"][0]["run_id"] = before["runs"][0]["run_id"]
        for key in ("candidate_run_ids", "selected_run_ids"):
            after["sampling"][key][0] = after["runs"][0]["run_id"]
        self.assertEqual(TIMELINE.compare(before, after, comparable=True)["status"], "pending")

    def test_reversed_or_different_contract_cohorts_remain_pending_and_threshold_uses_actual_values(self):
        before, after = self.comparison()
        after["selector"]["both_emulators_required"] = False
        self.assertEqual(TIMELINE.compare(before, after, True)["status"], "pending")
        before, after = self.comparison()
        after["runs"][0]["created_at"] = "2025-01-01T00:00:00Z"
        self.assertEqual(TIMELINE.compare(before, after, True)["status"], "pending")
        before, after = self.comparison()
        for row in after["runs"]:
            row["actual"]["whole_minutes"] = 20
        self.assertFalse(TIMELINE.compare(before, after, True)["after_median_at_most_18_minutes"])

    def test_equal_but_wrong_contracts_never_satisfy_the_target_gate(self):
        for key, value in (("attempt", 2), ("event", "push"), ("conclusion", "failure"),
                           ("both_emulators_required", False), ("all_jobs_success_or_skipped", False), ("attempt", True)):
            with self.subTest(key=key):
                before, after = self.comparison()
                before["selector"][key] = after["selector"][key] = value
                result = TIMELINE.compare(before, after, True)
                self.assertEqual(result["status"], "pending")
                self.assertIsNone(result["after_median_at_most_18_minutes"])

    def test_comparison_requires_consistent_explicit_sampling_and_candidate_ids(self):
        before, after = self.comparison()
        after["sampling"]["strategy"] = "fixed-candidates"
        self.assertEqual(TIMELINE.compare(before, after, True)["status"], "pending")
        before, after = self.comparison()
        after["analyzer_sha256"] = "0" * 64
        self.assertEqual(TIMELINE.compare(before, after, True)["status"], "pending")
        before, after = self.comparison()
        after["sampling"]["api_total_count"] = 100
        with self.assertRaisesRegex(ValueError, "truncated"):
            TIMELINE.compare(before, after, True)
        before, after = self.comparison()
        after["sampling"]["selected_run_ids"] = []
        with self.assertRaisesRegex(ValueError, "manifest"):
            TIMELINE.compare(before, after, True)

    def test_model_samples_summary_counts_or_duplicate_ids_cannot_satisfy_target_gate(self):
        before, after = self.comparison(19)
        after["summary"]["n"] = 1000
        after["conditional_eager_model"] = {"n_pr_runs": 1000, "critical_saving_seconds": {"p50": 600}}
        self.assertIsNone(TIMELINE.compare(before, after, True)["after_median_at_most_18_minutes"])
        after["kind"] = "conditional_eager_model"
        with self.assertRaises(ValueError):
            TIMELINE.compare(before, after, True)
        before, after = self.comparison()
        after["runs"][1]["run_id"] = after["runs"][0]["run_id"]
        with self.assertRaises(ValueError):
            TIMELINE.compare(before, after, True)


if __name__ == "__main__":
    unittest.main()
