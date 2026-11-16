#!/usr/bin/env python3
"""Fabricated evidence only: regressions must fail without timing the test runner."""
import copy
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

import journeys as j


class Evidence(unittest.TestCase):
    def setUp(self):
        self.spec = j.load(j.HERE / "fixtures.json")
        self.baseline = j.load(j.HERE / "baseline.json")
        self.samples = [
            {"journey": name, "build_commit": "a" * 8, "build_mode": "debug-test-support", "events": [
                {"kind": "start", "at_us": 10, "selection": 1, "token": 7},
                {"kind": self.spec[name]["terminal"], "at_us": 20, "selection": 1, "token": 7}],
             "counters": {"store_reads": 0, "player_opens": 0}}
            for name in j.CASES
        ]

    def result(self):
        return j.summary(self.samples, self.spec, self.baseline, 1, 100)

    def test_complete_evidence_has_medians_and_tails(self):
        result = self.result()
        self.assertTrue(result["valid"], result)
        for summary in result["journeys"].values():
            self.assertEqual((summary["sample_count"], summary["median_us"], summary["p95_us"], summary["max_us"]), (1, 10, 10, 10))

    def test_missing_duplicate_stale_and_timeout_are_invalid(self):
        terminal = self.spec["selection"]["terminal"]
        self.assertEqual(j.evidence(self.samples[-1], terminal, 100), 10)
        messages = {"missing": "missing or duplicate", "duplicate": "missing or duplicate",
                    "stale": "stale selection", "timeout": "journey timeout", "clock": "non-monotonic",
                    "terminal-before-start": "terminal occurred before start", "no-token": "missing selected source token",
                    "wrong-token": "stale source token", "failure": "scenario failed"}
        for defect, message in messages.items():
            with self.subTest(defect=defect):
                sample = copy.deepcopy(self.samples[-1])
                if defect == "missing":
                    sample["events"].pop()
                elif defect == "duplicate":
                    sample["events"].append(sample["events"][-1])
                elif defect == "stale":
                    sample["events"][-1]["selection"] = 2
                elif defect == "timeout":
                    sample["events"][-1]["at_us"] = 200
                elif defect == "terminal-before-start":
                    sample["events"].reverse()
                    sample["events"][0]["at_us"] = 10
                    sample["events"][1]["at_us"] = 20
                elif defect == "clock":
                    sample["events"][-1]["at_us"] = 5
                elif defect == "wrong-token":
                    sample["events"][-1]["token"] = 8
                elif defect == "no-token":
                    sample["events"][-1].pop("token")
                else:
                    sample["failure"] = "scenario failed"
                with self.assertRaisesRegex(ValueError, message):
                    j.evidence(sample, terminal, 100)

    def test_a_proxy_cannot_supply_first_render_evidence(self):
        for proxy in ("acceptance-proxy", "ready-proxy", "size-proxy"):
            sample = copy.deepcopy(self.samples[-1])
            sample["events"][-1]["kind"] = proxy
            with self.assertRaisesRegex(ValueError, "missing first-render"):
                j.evidence(sample, "first-render", 100)
        sample["events"][-1]["kind"] = "first-render"
        self.assertEqual(j.evidence(sample, "first-render", 100), 10)

    def test_zero_missing_duplicate_and_unknown_samples_are_invalid(self):
        self.assertFalse(j.summary([], self.spec, self.baseline, 0, 100)["valid"])
        self.samples.pop()
        self.assertFalse(self.result()["valid"])
        self.samples.append(copy.deepcopy(self.samples[0]))
        self.assertFalse(self.result()["valid"])
        self.samples[-1]["journey"] = "unknown"
        self.assertFalse(self.result()["valid"])

    def test_extra_store_read_or_player_reopen_identifies_regressed_journey(self):
        for name, counter in (("busy-guide", "store_reads"), ("selection", "player_opens")):
            with self.subTest(counter=counter):
                samples = copy.deepcopy(self.samples)
                sample = next(s for s in samples if s["journey"] == name)
                sample["counters"][counter] = self.baseline["journeys"][name][counter] + 1
                result = j.summary(samples, self.spec, self.baseline, 1, 100)
                self.assertFalse(result["valid"])
                self.assertEqual(result["journeys"][name]["valid_evidence_count"], 0)
                self.assertIsNone(result["journeys"][name]["median_us"])
                self.assertTrue(any(name in error and counter in error for error in result["errors"]))

    def test_persisted_report_revalidates_metadata_and_samples(self):
        report = {"schema": 1, "dirty": False, "commit": "a" * 40, "platform": "linux", "backend": "fabricated",
                  "hardware_class": "host", "build_mode": "debug-test-support", "fixture": self.spec,
                  "fixture_sha256": j.digest((j.HERE / "fixtures.json").read_bytes()),
                  "repetitions": 1, "timeout_us": 100, "samples": self.samples, "valid": True}
        self.assertTrue(j.check_report(report, self.baseline)["valid"])
        stale = copy.deepcopy(report)
        stale["samples"][0]["build_commit"] = "b" * 8
        self.assertFalse(j.check_report(stale, self.baseline)["valid"])
        report["samples"] = []
        self.assertFalse(j.check_report(report, self.baseline)["valid"], "stored valid cannot hide lost samples")
        for field, value in (("schema", 2), ("dirty", True), ("commit", ""), ("fixture_sha256", "b" * 64), ("backend", "")):
            bad = {**report, field: value}
            with self.subTest(field=field), self.assertRaises(ValueError):
                j.check_report(bad, self.baseline)

    def test_missing_negative_bool_and_extra_counters_are_invalid(self):
        for value in ({}, {"store_reads": -1, "player_opens": 0}, {"store_reads": True, "player_opens": 0},
                      {"store_reads": 0, "player_opens": 0, "other": 0}):
            sample = copy.deepcopy(self.samples[0])
            sample["counters"] = value
            with self.assertRaises(ValueError):
                j.evidence(sample, "guide-mounted", 100)



class Runner(unittest.TestCase):
    @unittest.skipUnless(shutil.which("true"), "requires a no-op executable")
    def test_missing_evidence_writes_every_failed_sample_and_a_failure_report(self):
        with tempfile.TemporaryDirectory(prefix="patina-fabricated-run-") as directory:
            output = Path(directory) / "report.json"
            result = subprocess.run([sys.executable, str(j.HERE / "journeys.py"), "run",
                                     "--harness", shutil.which("true"), "--native-harness", shutil.which("true"),
                                     "--repetitions", "1", "--output", str(output)],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            report = j.load(output)
            self.assertFalse(report["valid"])
            self.assertEqual(len(report["samples"]), 3)
            self.assertEqual(set(report["journeys"]), set(j.CASES))
            self.assertTrue(all(report["journeys"][name]["median_us"] is None for name in j.CASES))


class MalformedRunner(unittest.TestCase):
    def test_malformed_or_nonobject_sample_files_do_not_drop_other_samples(self):
        from unittest.mock import patch
        import contextlib
        import io
        import argparse
        import os
        for payload in ("{", "[]"):
            with self.subTest(payload=payload), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / "report.json"
                args = argparse.Namespace(output=str(output), baseline=str(j.HERE / "baseline.json"),
                                          repetitions=1, timeout=10, harness=sys.executable, native_harness=sys.executable)
                def scenario(command, env, timeout):
                    Path(env["IPTV_PERF_SAMPLE"]).write_text(payload)
                    return subprocess.CompletedProcess(command, 0, "")
                with patch.object(j, "scenario", side_effect=scenario), patch.dict(os.environ, {}), contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(j.run(args), 1)
                report = j.load(output)
                self.assertEqual(len(report["samples"]), 3)
                self.assertTrue(all("unreadable evidence" in s["failure"] for s in report["samples"]))


class Android(unittest.TestCase):
    def test_android_proxy_output_is_invalid_and_classifies_hardware(self):
        spec = importlib.util.spec_from_file_location("android_envelope", j.HERE / "android-envelope.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        text = "run 1: activity launch 12 ms, guide mount proxy 30 ms\nzap 1: reported size proxy 15 ms"
        result = module.envelope("emulator-5556", text, "a" * 40)
        self.assertFalse(result["valid"])
        self.assertEqual(result["hardware_class"], "emulator")
        self.assertEqual(result["sample_count"], {"cold": 1, "selection": 1})
        self.assertEqual(module.envelope("synthetic-device", "", "b" * 40)["hardware_class"], "physical-tv-unverified")


class Policy(unittest.TestCase):
    def setUp(self):
        self.base = j.load(j.HERE / "baseline.json")
        self.candidate = copy.deepcopy(self.base)

    def test_same_or_stricter_baselines_pass(self):
        self.assertEqual(j.policy(self.base, self.candidate, []), [])
        self.candidate["journeys"]["cached-guide"]["store_reads"] -= 1
        self.assertEqual(j.policy(self.base, self.candidate, []), [])

    def test_increase_counter_removal_and_journey_removal_require_rationale(self):
        for defect in ("increase", "counter", "journey"):
            self.candidate = copy.deepcopy(self.base)
            if defect == "increase":
                self.candidate["journeys"]["selection"]["player_opens"] += 1
            elif defect == "counter":
                del self.candidate["journeys"]["selection"]["player_opens"]
            else:
                del self.candidate["journeys"]["selection"]
            with self.subTest(defect=defect), self.assertRaisesRegex(ValueError, "baseline weakening"):
                j.policy(self.base, self.candidate, [])

    def test_endpoint_or_fixture_change_requires_its_own_review_record(self):
        spec = j.load(j.HERE / "fixtures.json")
        changed = copy.deepcopy(spec)
        changed["selection"]["terminal"] = "acceptance-proxy"
        with self.assertRaisesRegex(ValueError, "fixture specification or endpoint"):
            j.policy(self.base, self.candidate, [], spec, changed)

    def test_a_reused_or_unrelated_rationale_cannot_authorize_weakening(self):
        self.candidate["journeys"]["selection"]["player_opens"] += 1
        record = {"base_sha256": j.digest(json.dumps(self.base, sort_keys=True).encode()),
                  "candidate_sha256": j.digest(json.dumps(self.candidate, sort_keys=True).encode()),
                  "rationale": "Fabricated test explanation for a newly reviewed fixture workload."}
        self.assertTrue(j.policy(self.base, self.candidate, [record]))
        self.candidate["journeys"]["selection"]["player_opens"] += 1
        with self.assertRaises(ValueError):
            j.policy(self.base, self.candidate, [record])


if __name__ == "__main__":
    unittest.main()
