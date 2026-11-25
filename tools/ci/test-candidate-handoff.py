#!/usr/bin/env python3
"""Synthetic provenance, hostile archive, fallback and measurement contracts."""

import copy
from contextlib import chdir
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SPEC = importlib.util.spec_from_file_location("handoff", Path(__file__).with_name("candidate-handoff.py"))
handoff = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(handoff)


class Contract(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.destination = self.root / "release"
        self.archive = self.root / "candidate.zip"
        self.expected = {
            "schema": 1,
            "commit": "a" * 40,
            "source_sha256": "1" * 64,
            "locks": {"Cargo.lock": "2" * 64},
            "profile": "release",
            "features": "default; no test-support",
            "native": {"image_version": "synthetic-image", "link": "3" * 64, "sdk": "4" * 64},
            "runtime": {"libmpv-2.dll": "5" * 64},
        }
        self.exe = b"MZ synthetic executable; never executed"
        self.proof = {
            "run_id": "101",
            "attempt": "1",
            "job_id": "201",
            "artifact_id": "301",
            "inputs_sha256": hashlib.sha256(handoff.encoded(self.expected)).hexdigest(),
            "file_sha256": hashlib.sha256(self.exe).hexdigest(),
        }
        self.manifest = {
            "schema": 1,
            "inputs": copy.deepcopy(self.expected),
            "producer": {"run_id": 101, "attempt": 1, "job_id": 201, "repository": handoff.REPOSITORY, "workflow": handoff.WORKFLOW, "commit": self.expected["commit"]},
            "file": {"name": "iptv-app.exe", "sha256": self.proof["file_sha256"], "size": len(self.exe)},
        }
        self.build_archive()
        self.run = {
            "head_sha": self.expected["commit"],
            "head_branch": handoff.BRANCH,
            "path": handoff.WORKFLOW,
            "event": "push",
            "repository": {"full_name": handoff.REPOSITORY},
            "head_repository": {"full_name": handoff.REPOSITORY},
            "run_attempt": 1,
            "status": "in_progress",
        }
        self.job = {"id": 201, "run_id": 101, "run_attempt": 1, "name": handoff.PRODUCER, "head_sha": self.expected["commit"], "status": "completed", "conclusion": "success"}
        self.artifact = {
            "id": 301,
            "name": "pilot336-exe-101-1",
            "digest": "sha256:" + self.proof["artifact_digest"],
            "expired": False,
            "workflow_run": {"id": 101, "head_sha": self.expected["commit"]},
        }
        self.requests, self.downloads = [], []

    def build_archive(self, members=None):
        raw = handoff.encoded(self.manifest)
        self.proof["manifest_sha256"] = hashlib.sha256(raw).hexdigest()
        with zipfile.ZipFile(self.archive, "w") as archive:
            for name, data in members if members is not None else [("manifest.json", raw), ("iptv-app.exe", self.exe)]:
                archive.writestr(name, data)
        self.proof["artifact_digest"] = handoff.digest(self.archive)

    def api(self, endpoint):
        self.requests.append(endpoint)
        if endpoint.endswith("/runs/101"):
            return copy.deepcopy(self.run)
        if endpoint.endswith("/jobs/201"):
            return copy.deepcopy(self.job)
        if "/artifacts?" in endpoint:
            return {"artifacts": copy.deepcopy(self.artifacts if hasattr(self, "artifacts") else [self.artifact])}
        self.fail("Unexpected synthetic API request: " + endpoint)

    def download(self, identifier, destination):
        self.downloads.append(identifier)
        destination.write_bytes(self.archive.read_bytes())

    def acquire(self, **kwargs):
        return handoff.acquire(self.expected, self.proof, self.destination, self.api, self.download, **kwargs)

    def assert_closed(self):
        self.assertFalse(self.destination.exists())
        self.assertEqual(self.downloads, [])

    def rust_fixture(self):
        sysroot = self.root / "synthetic-rust-toolchain"
        (sysroot / "bin").mkdir(parents=True)
        (sysroot / "lib/rustlib/x86_64-pc-windows-msvc/lib").mkdir(parents=True)
        for name in ("rustc.exe", "cargo.exe", "rustc_driver-synthetic.dll"):
            (sysroot / "bin" / name).write_bytes(("synthetic " + name).encode())
        (sysroot / "lib/rustlib/x86_64-pc-windows-msvc/lib/std.rlib").write_bytes(b"synthetic standard library")
        records = [{"pkg": name, "target": "x86_64-pc-windows-msvc", "is_extension": False} for name in ("cargo", "rustc", "rust-std")]
        self.write_receipts(sysroot, records)
        return sysroot, records

    def write_receipts(self, sysroot, records, version="1"):
        directory = sysroot / "lib/rustlib"
        (directory / "components").write_bytes("".join(item["pkg"] + "-" + item["target"] + "\n" for item in records).encode())
        text = f"config_version = {json.dumps(version)}\n"
        for item in records:
            text += "\n[[components]]\n" + "".join(f"{name} = {json.dumps(value)}\n" for name, value in item.items())
        (directory / "multirust-config.toml").write_text(text)

    def test_verified_success_stages_only_exact_executable(self):
        self.assertEqual(self.acquire(), "reuse")
        self.assertEqual([path.name for path in self.destination.iterdir()], ["iptv-app.exe"])
        self.assertEqual((self.destination / "iptv-app.exe").read_bytes(), self.exe)
        self.assertEqual(self.downloads, [301])

    def test_missing_requires_successful_complete_list(self):
        self.artifacts = []
        self.assertEqual(self.acquire(), "absent")
        self.assert_closed()
        self.assertTrue(any("/artifacts?" in request for request in self.requests))

    def test_actual_expired_metadata_allows_cold_fallback(self):
        self.artifact["expired"] = True
        self.assertEqual(self.acquire(), "expired")
        self.assert_closed()

    def test_explicit_cold_never_reads_api_or_downloads(self):
        self.assertEqual(self.acquire(cold=True), "explicit-cold")
        self.assertEqual(self.requests, [])
        self.assert_closed()

    def test_api_errors_never_become_absence(self):
        for marker in ("403", "404", "429", "500", "timeout", "invalid JSON"):
            with self.subTest(marker=marker), patch.object(self, "api", side_effect=RuntimeError(marker)):
                with self.assertRaises(RuntimeError):
                    self.acquire()
                self.assert_closed()

    def test_download_failure_is_visible_without_fallback(self):
        with patch.object(self, "download", side_effect=RuntimeError("synthetic 404")):
            with self.assertRaises(RuntimeError):
                self.acquire()
        self.assertFalse(self.destination.exists())

    def test_each_independent_input_axis_must_match(self):
        for key in self.expected:
            expected = copy.deepcopy(self.expected)
            expected[key] = "mismatched synthetic input"
            with self.subTest(key=key), self.assertRaises(ValueError):
                handoff.acquire(expected, self.proof, self.destination, self.api, self.download)
            self.assert_closed()

    def test_build_overrides_rejected_without_logging_secrets(self):
        for name in (
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_TARGET_DIR",
            "CARGO_PROFILE_RELEASE_LTO",
            "CARGO_BUILD_RUSTC",
            "CARGO_INCREMENTAL",
            "CC_x86_64_pc_windows_msvc",
            "OPENSSL_DIR",
            "CMAKE_GENERATOR",
            "SOURCE_DATE_EPOCH",
        ):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "^Unsupported build environment override$"):
                handoff.validate_environment({name: "synthetic-private-value"})
        handoff.validate_environment({"GH_TOKEN": "synthetic-do-not-record", "CARGO_HOME": "synthetic", "CARGO_TERM_COLOR": "always"})
        handoff.validate_environment({"CARGO_INCREMENTAL": "0"})
        for value in ("1", "false", "00"):
            with self.subTest(incremental=value), self.assertRaises(ValueError):
                handoff.validate_environment({"CARGO_INCREMENTAL": value})

    def test_failed_or_running_or_wrong_attempt_producer_cannot_fallback(self):
        for field, value in [("conclusion", "failure"), ("status", "in_progress"), ("run_attempt", 2), ("run_id", 102), ("name", "Other job"), ("head_sha", "b" * 40)]:
            old = self.job[field]
            self.job[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.acquire()
            self.job[field] = old
            self.assert_closed()

    def test_locked_native_tool_selectors_and_flags_are_refused(self):
        # cc 1.11.1 documents target suffixes and HOST_/TARGET_ variants;
        # find-msvc-tools 0.1.15 also selects alternate installations/toolsets.
        for name in (
            "TARGET_CFLAGS",
            "HOST_CC",
            "CFLAGS_x86_64_pc_windows_msvc",
            "CFLAGS_x86_64-pc-windows-msvc",
            "CRATE_CC_NO_DEFAULTS",
            "TARGET_AR",
            "HOST_ARFLAGS",
            "ARFLAGS_x86_64_pc_windows_msvc",
            "HOST_CXXFLAGS",
            "TARGET_CXXSTDLIB_STATIC",
            "CC_MASM_ASM_x86_64_pc_windows_msvc",
            "TARGET_RANLIBFLAGS",
            "VCToolsVersion",
            "VisualStudioVersion",
            "VSINSTALLDIR",
            "VSCMD_ARG_TGT_ARCH",
            "VSCMD_ARG_VCVARS_SPECTRE",
            "VSTEL_MSBuildProjectFullPath",
        ):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "^Unsupported build environment override$"):
                handoff.validate_environment({name: "synthetic-override"})

    def test_native_tool_discovery_requires_one_effective_file_each(self):
        tool = self.root / "synthetic-tool.exe"
        tool.write_bytes(b"synthetic-native-tool")
        tools = {name: str(tool) for name in ("cl", "link", "perl", "nasm", "cmake", "ninja")}
        expected = {"name": tool.name, "sha256": handoff.digest(tool)}
        self.assertEqual(handoff.fingerprint_tools(tools)["perl"], expected)
        for value in ([str(tool), str(tool)], None, ""):
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, "exactly one effective file"):
                handoff.fingerprint_tools(dict(tools, perl=value))
        del tools["link"]
        with self.assertRaises(ValueError):
            handoff.fingerprint_tools(tools)

    def test_original_successful_producer_is_retained_on_consumer_retry(self):
        self.run["run_attempt"] = 2
        self.assertEqual(self.acquire(), "reuse")
        self.assertIn(f"repos/{handoff.REPOSITORY}/actions/jobs/201", self.requests)

    def test_untrusted_workflow_run_rejected(self):
        for field, value in [
            ("head_sha", "b" * 40),
            ("head_branch", "master"),
            ("path", "untrusted.yml"),
            ("event", "pull_request"),
            ("head_repository", {"full_name": "synthetic/fork"}),
            ("repository", {"full_name": "synthetic/fork"}),
        ]:
            old = self.run[field]
            self.run[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.acquire()
            self.run[field] = old
            self.assert_closed()

    def test_artifact_metadata_mismatch_or_ambiguity_rejected(self):
        for field, value in [("id", 302), ("name", "pilot336-other"), ("digest", "sha256:" + "0" * 64), ("expired", "false"), ("workflow_run", {"id": 102, "head_sha": "b" * 40})]:
            old = self.artifact[field]
            self.artifact[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.acquire()
            self.artifact[field] = old
            self.assert_closed()
        self.artifacts = [self.artifact, dict(self.artifact, id=302)]
        with self.assertRaises(ValueError):
            self.acquire()
        self.assert_closed()

    def test_raw_archive_digest_checked_before_unpack(self):
        self.archive.write_bytes(self.archive.read_bytes() + b"tampering")
        with self.assertRaisesRegex(ValueError, "archive checksum"):
            self.acquire()
        self.assertFalse(self.destination.exists())

    def test_zip_member_allowlist_traversal_duplicate_and_extra(self):
        raw = handoff.encoded(self.manifest)
        for name in ("../iptv-app.exe", "/iptv-app.exe", "C:/iptv-app.exe", "sub/iptv-app.exe", "iptv-app.exe\\evil"):
            self.build_archive([("manifest.json", raw), (name, self.exe)])
            with self.subTest(name=name), self.assertRaises(ValueError):
                handoff.unpack(self.archive, self.expected, self.proof, self.destination)
            self.assertFalse(self.destination.exists())
        self.build_archive([("manifest.json", raw), ("iptv-app.exe", self.exe), ("script.ps1", b"synthetic")])
        with self.assertRaises(ValueError):
            handoff.unpack(self.archive, self.expected, self.proof, self.destination)
        with self.assertWarns(UserWarning):
            self.build_archive([("manifest.json", raw), ("iptv-app.exe", self.exe), ("iptv-app.exe", self.exe)])
        with self.assertRaises(ValueError):
            handoff.unpack(self.archive, self.expected, self.proof, self.destination)

    def test_symlink_and_oversized_members_rejected(self):
        symlink = zipfile.ZipInfo("iptv-app.exe")
        symlink.create_system = 3
        symlink.external_attr = (stat.S_IFLNK | 0o777) << 16
        self.build_archive([("manifest.json", handoff.encoded(self.manifest)), (symlink, b"../outside")])
        with self.assertRaisesRegex(ValueError, "Unsafe"):
            handoff.unpack(self.archive, self.expected, self.proof, self.destination)
        self.build_archive()
        with patch.object(handoff, "MAX_EXE", 2), self.assertRaisesRegex(ValueError, "Unsafe"):
            handoff.unpack(self.archive, self.expected, self.proof, self.destination)

    def test_manifest_and_file_proofs_are_separate_from_archive_proof(self):
        self.build_archive([("manifest.json", b"{}"), ("iptv-app.exe", self.exe)])
        with self.assertRaisesRegex(ValueError, "Manifest checksum"):
            handoff.unpack(self.archive, self.expected, self.proof, self.destination)
        self.build_archive([("manifest.json", handoff.encoded(self.manifest)), ("iptv-app.exe", b"tampered exe")])
        with self.assertRaisesRegex(ValueError, "Executable"):
            handoff.unpack(self.archive, self.expected, self.proof, self.destination)
        self.assertFalse(self.destination.exists())

    def test_manifest_identity_cannot_self_authenticate(self):
        self.manifest["producer"]["attempt"] = 2
        self.build_archive()
        with self.assertRaisesRegex(ValueError, "provenance"):
            handoff.unpack(self.archive, self.expected, self.proof, self.destination)
        self.assertFalse(self.destination.exists())

    def test_fresh_target_required_even_for_explicit_cold(self):
        self.destination.mkdir()
        for cold in (True, False):
            with self.subTest(cold=cold), self.assertRaises(ValueError):
                self.acquire(cold=cold)
        self.assertEqual(self.requests, [])

    def test_artifact_list_is_paginated_before_accepting_absence(self):
        requests = []

        def api(endpoint):
            requests.append(endpoint)
            return {"artifacts": list(range(100)) if "&page=1" in endpoint else []}

        self.assertEqual(len(handoff.pages("synthetic?filter=all", "artifacts", api)), 100)
        self.assertEqual(len(requests), 2)

    def test_measurement_statistics_include_failures_and_small_cohorts(self):
        runs = [{"conclusion": "success", "seconds": value} for value in range(1, 21)] + [{"conclusion": "failure", "seconds": 99}]
        self.assertEqual(handoff.measurements(runs), {"n": 21, "successful_n": 20, "measured_successful_n": 20, "unavailable_n": 0, "failures": 1, "p50_seconds": 10.5, "p90_seconds": 18})
        self.assertIsNone(handoff.measurements([])["p90_seconds"])

    def test_actual_parallel_endpoints_and_failure_counts(self):
        jobs = []
        for identifier, name, endpoint in ((201, handoff.PRODUCER, 100), (202, "Pilot consumer", 160), (203, "Cold control", 130)):
            jobs.append({"id": identifier, "name": name, "attempt": 1, "conclusion": "success", "trigger_to_completion_seconds": endpoint, "queue_seconds": 10, "execution_seconds": 90})
        timeline = {"run_id": 101, "commit": "a" * 40, "intended_mode": "reuse", "decision": "reuse", "jobs": jobs}
        summary = handoff.summarize([timeline])
        self.assertEqual(summary["paths"]["producer_consumer_path"]["p50_seconds"], 160)
        self.assertEqual(summary["paths"]["whole_pilot"]["p50_seconds"], 160)
        self.assertEqual(summary["paths"]["cold_control_path"]["p50_seconds"], 130)
        failed = copy.deepcopy(timeline)
        failed["run_id"] = 102
        failed["jobs"][1]["conclusion"] = "cancelled"
        summary = handoff.summarize([timeline, failed])
        self.assertEqual(summary["paths"]["whole_pilot"]["n"], 2)
        self.assertEqual(summary["paths"]["whole_pilot"]["failures"], 1)
        self.assertEqual(summary["paths"]["cold_control_path"]["successful_n"], 2)

    def test_measurement_cohorts_do_not_mix_heads_decisions_or_duplicate_runs(self):
        first = {"run_id": 101, "commit": "a" * 40, "intended_mode": "reuse", "decision": "reuse", "jobs": []}
        for field, value in (("commit", "b" * 40), ("decision", "expired"), ("intended_mode", "cold"), ("run_id", 101)):
            other = dict(first, run_id=102)
            other[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                handoff.summarize([first, other])

    def test_retry_preserves_failed_attempt_and_runner_cost(self):
        jobs = []
        for identifier, name, attempt, conclusion, endpoint, cost in (
            (201, handoff.PRODUCER, 1, "success", 100, 80),
            (202, "Pilot consumer", 1, "failure", 150, 40),
            (203, "Cold control", 1, "success", 130, 90),
            (204, "Pilot consumer", 2, "success", 250, 60),
        ):
            jobs.append({"id": identifier, "name": name, "attempt": attempt, "conclusion": conclusion, "trigger_to_completion_seconds": endpoint, "queue_seconds": 10, "execution_seconds": cost})
        timeline = {"run_id": 101, "attempt": 2, "commit": "a" * 40, "intended_mode": "reuse", "decision": "reuse", "jobs": jobs}
        summary = handoff.summarize([timeline])
        self.assertEqual(summary["cohort"], "retried")
        self.assertEqual(summary["paths"]["whole_pilot"]["p50_seconds"], 250)
        self.assertEqual(summary["attempts_by_job"]["Pilot consumer"]["failed"], 1)
        self.assertEqual(summary["attempts_by_job"]["Pilot consumer"]["total_execution_seconds"], 100)
        self.assertEqual(summary["segments"]["Pilot consumer/execution_seconds"]["n"], 2)
        self.assertEqual(summary["segments"]["Pilot consumer/execution_seconds"]["failures"], 1)
        self.assertEqual(summary["runs"][0]["attempts"]["total_execution_seconds"], 270)
        clean = dict(timeline, run_id=102, attempt=1, jobs=jobs[:3])
        with self.assertRaisesRegex(ValueError, "clean and retried"):
            handoff.summarize([timeline, clean])
        jobs[1]["conclusion"] = "cancelled"
        canceled = handoff.summarize([timeline])["runs"][0]["attempts"]
        self.assertEqual(canceled["failed"], 0)
        self.assertEqual(canceled["canceled"], 1)
        self.assertEqual(canceled["total_execution_seconds"], 270)

    def test_queue_only_cancellation_still_has_timeline_evidence(self):
        run = {"head_sha": "a" * 40, "run_attempt": 1, "created_at": "2026-01-01T00:00:00Z"}
        job = {"id": 201, "name": handoff.PRODUCER, "run_attempt": 1, "conclusion": "cancelled", "created_at": "2026-01-01T00:00:02Z", "started_at": None, "completed_at": "2026-01-01T00:00:12Z"}

        def api(endpoint):
            return {"jobs": [job]} if "/jobs?" in endpoint else run

        with patch.dict(os.environ, {"PILOT_INTENDED_MODE": "reuse"}):
            timeline = handoff.timeline(101, api)
        self.assertEqual(timeline["jobs"][0]["queue_seconds"], 10)
        self.assertEqual(timeline["jobs"][0]["execution_seconds"], 0)
        self.assertEqual(timeline["jobs"][0]["trigger_to_completion_seconds"], 12)

    def test_missing_failed_consumer_outputs_are_retained_in_intended_cohort(self):
        job = {"id": 201, "name": "Pilot consumer", "attempt": 1, "conclusion": "success", "trigger_to_completion_seconds": 160, "queue_seconds": 10, "execution_seconds": 90}
        jobs = [dict(job, name=handoff.PRODUCER, id=200), job, dict(job, name="Cold control", id=202)]
        passing = {"run_id": 101, "commit": "a" * 40, "intended_mode": "reuse", "decision": "reuse", "jobs": jobs}
        for conclusion in ("failure", "cancelled", "skipped"):
            for decision in ("", "unavailable"):
                failed = dict(passing, run_id=102, decision=decision, jobs=[jobs[0], dict(job, id=203, conclusion=conclusion), jobs[2]])
                with self.subTest(conclusion=conclusion, decision=decision):
                    summary = handoff.summarize([passing, failed])
                    self.assertEqual(summary["paths"]["whole_pilot"]["n"], 2)
                    self.assertEqual(summary["paths"]["whole_pilot"]["failures"], 1)
                    self.assertEqual(summary["attempts_by_job"]["Pilot consumer"]["n"], 2)
                    self.assertEqual(summary["runs"][1]["decision"], "unavailable")
                    self.assertEqual(summary["segments"]["Pilot consumer/execution_seconds"]["failures"], 1)
        with self.assertRaisesRegex(ValueError, "Successful consumer"):
            handoff.summarize([dict(passing, decision="")])

    def test_additional_effective_native_overrides_are_refused(self):
        for name in ("SQLCIPHER_LIB_DIR", "SQLCIPHER_STATIC", "PERL", "NASM", "RUSTC_BOOTSTRAP", "RUSTC_LINKER"):
            with self.subTest(name=name), self.assertRaises(ValueError):
                handoff.validate_environment({name: "synthetic-native-override"})

    def test_inherited_and_invalid_queue_intervals_are_unavailable(self):
        observed = {"head_sha": "a" * 40, "run_attempt": 2, "created_at": "2026-01-01T00:00:00Z"}
        base = {"name": handoff.PRODUCER, "conclusion": "success", "created_at": "2026-01-01T00:02:00Z", "started_at": "2026-01-01T00:00:10Z", "completed_at": "2026-01-01T00:01:00Z"}
        jobs = [dict(base, id=201, run_attempt=1), dict(base, name="Pilot consumer", id=202, run_attempt=2)]

        def api(endpoint):
            return {"jobs": jobs} if "/jobs?" in endpoint else observed

        with patch.dict(os.environ, {"PILOT_INTENDED_MODE": "reuse", "PILOT_DECISION": "reuse"}):
            timeline = handoff.timeline(101, api)
        self.assertEqual([job["queue_seconds"] for job in timeline["jobs"]], [None, None])
        self.assertEqual([job["execution_seconds"] for job in timeline["jobs"]], [50, None])
        self.assertTrue(timeline["jobs"][0]["inherited_from_previous_attempt"])
        summary = handoff.summarize([timeline])
        attempts = summary["runs"][0]["attempts"]
        self.assertEqual(attempts["unknown_queue_attempts"], 2)
        self.assertEqual(attempts["known_queue_seconds_sum"], 0)
        self.assertIsNone(attempts["total_execution_seconds"])
        self.assertEqual(attempts["known_execution_seconds_sum"], 50)
        self.assertEqual(attempts["unknown_execution_attempts"], 1)
        self.assertEqual(summary["segments"]["Pilot consumer/queue_seconds"]["unavailable_n"], 1)

    def copied_execution_fixture(self):
        observed = {"head_sha": "a" * 40, "run_attempt": 2, "created_at": "2026-01-01T00:00:00Z"}
        jobs = []
        for identifier, name, runner, end in ((201, handoff.PRODUCER, 7, "00:01:00"), (202, "Pilot consumer", 8, "00:01:00"), (203, "Cold control", 9, "00:01:10")):
            jobs.append({"id": identifier, "name": name, "head_sha": "a" * 40, "runner_id": runner, "runner_group_id": 1, "run_attempt": 1, "conclusion": "success", "created_at": "2026-01-01T00:00:00Z", "started_at": "2026-01-01T00:00:10Z", "completed_at": f"2026-01-01T{end}Z", "steps": [{"name": "Synthetic build or package", "status": "completed", "conclusion": "success", "started_at": "2026-01-01T00:00:10Z", "completed_at": f"2026-01-01T{end}Z"}]})
        # GitHub gives retained successes new IDs/attempts/created_at and even a
        # different runner group, while the execution itself remains identical.
        jobs += [dict(copy.deepcopy(jobs[index]), id=identifier, run_attempt=2, runner_group_id=2, created_at="2026-01-01T00:03:00Z") for index, identifier in ((0, 204), (2, 205))]
        jobs.append(dict(copy.deepcopy(jobs[1]), id=206, run_attempt=2, runner_id=10, created_at="2026-01-01T00:03:00Z", started_at="2026-01-01T00:03:10Z", completed_at="2026-01-01T00:04:00Z", steps=[{"name": "Synthetic actual retry", "conclusion": "success"}]))
        return observed, jobs

    def collect_synthetic_timeline(self, observed, jobs):
        with patch.dict(os.environ, {"PILOT_INTENDED_MODE": "reuse", "PILOT_DECISION": "reuse"}):
            return handoff.timeline(101, lambda endpoint: {"jobs": jobs} if "/jobs?" in endpoint else observed)

    def test_copied_successes_keep_raw_records_without_duplicate_execution_cost(self):
        observed, jobs = self.copied_execution_fixture()
        timeline = self.collect_synthetic_timeline(observed, jobs)
        self.assertEqual(len(timeline["jobs"]), 6)
        self.assertEqual({row["id"]: row["alias_of"] for row in timeline["jobs"] if row["alias_of"]}, {204: 201, 205: 203})
        self.assertEqual(timeline["jobs"][3]["attempt"], 2)
        self.assertEqual(timeline["jobs"][3]["steps"], jobs[3]["steps"])
        summary = handoff.summarize([timeline])
        self.assertEqual(summary["runs"][0]["attempts"]["n"], 4)
        self.assertEqual(summary["runs"][0]["attempts"]["copied_aliases"], 2)
        self.assertEqual(summary["runs"][0]["attempts"]["total_execution_seconds"], 210)
        self.assertEqual(summary["attempts_by_job"][handoff.PRODUCER]["n"], 1)
        self.assertEqual(summary["segments"][handoff.PRODUCER + "/execution_seconds"]["n"], 1)
        self.assertEqual(summary["segments"]["Pilot consumer/execution_seconds"]["n"], 2)
        self.assertEqual(summary["paths"]["whole_pilot"]["p50_seconds"], 240)

    def test_genuine_failed_and_canceled_retry_executions_are_retained(self):
        for conclusion in ("failure", "cancelled"):
            observed, jobs = self.copied_execution_fixture()
            jobs[1]["conclusion"] = conclusion
            timeline = self.collect_synthetic_timeline(observed, jobs)
            self.assertIsNone(timeline["jobs"][1]["alias_of"])
            summary = handoff.summarize([timeline])
            attempts = summary["runs"][0]["attempts"]
            with self.subTest(conclusion=conclusion):
                self.assertEqual(attempts["n"], 4)
                self.assertEqual(attempts["total_execution_seconds"], 210)
                self.assertEqual(attempts["failed"], int(conclusion == "failure"))
                self.assertEqual(attempts["canceled"], int(conclusion == "cancelled"))
                self.assertEqual(summary["segments"]["Pilot consumer/execution_seconds"]["failures"], 1)

    def test_unproven_execution_copies_are_unknown_not_assumed_aliases(self):
        for field, value in (("head_sha", None), ("head_sha", "b" * 40), ("runner_id", 11), ("steps", []), ("steps", [{"name": "Different synthetic step"}])):
            observed, jobs = self.copied_execution_fixture()
            jobs[3][field] = value
            timeline = self.collect_synthetic_timeline(observed, jobs)
            with self.subTest(field=field, value=value):
                self.assertIsNone(timeline["jobs"][3]["alias_of"])
                self.assertTrue(timeline["jobs"][3]["execution_identity_unknown"])
                self.assertIsNone(timeline["jobs"][3]["execution_seconds"])
                summary = handoff.summarize([timeline])
                self.assertEqual(summary["runs"][0]["attempts"]["n"], 5)
                self.assertIsNone(summary["runs"][0]["attempts"]["total_execution_seconds"])
        observed, jobs = self.copied_execution_fixture()
        jobs.append(dict(copy.deepcopy(jobs[0]), id=207))
        self.assertNotIn(204, handoff.execution_aliases(jobs))
        # An invalid creation timestamp before completion is insufficient proof.
        jobs = jobs[:-1]
        jobs[3]["created_at"] = "2026-01-01T00:00:20Z"
        self.assertNotIn(204, handoff.execution_aliases(jobs))
        self.assertTrue(self.collect_synthetic_timeline(observed, jobs)["jobs"][3]["execution_identity_unknown"])
        jobs[3]["created_at"] = jobs[3]["completed_at"]
        self.assertNotIn(204, handoff.execution_aliases(jobs))

    def test_seal_binds_original_job_identity_and_rejects_ambiguity(self):
        executable = self.root / "target/release/iptv-app.exe"
        executable.parent.mkdir(parents=True)
        executable.write_bytes(self.exe)
        output = self.root / "sealed"
        proof = handoff.seal(self.expected, output, 101, 1, lambda _: {"jobs": [self.job]}, self.root)
        self.assertEqual(proof["job_id"], "201")
        self.assertEqual(proof["manifest_sha256"], handoff.digest(output / "manifest.json"))
        self.assertEqual(json.loads((output / "manifest.json").read_text())["producer"]["attempt"], 1)
        for jobs in ([], [self.job, self.job], [dict(self.job, run_attempt=2)]):
            with self.subTest(jobs=jobs), self.assertRaisesRegex(ValueError, "ambiguous"):
                handoff.seal(self.expected, self.root / "refused", 101, 1, lambda _: {"jobs": jobs}, self.root)
            self.assertFalse((self.root / "refused").exists())

    def test_native_tree_identity_refuses_empty_and_linked_inputs(self):
        directory = self.root / "native-inputs"
        directory.mkdir()
        with self.assertRaisesRegex(ValueError, "Empty"):
            handoff.tree_digest(directory)
        (directory / "header.h").write_bytes(b"synthetic native header")
        before = handoff.tree_digest(directory)
        (directory / "header.h").write_bytes(b"changed synthetic header")
        self.assertNotEqual(handoff.tree_digest(directory)["sha256"], before["sha256"])
        (directory / "linked.h").symlink_to(directory / "header.h")
        with self.assertRaisesRegex(ValueError, "links"):
            handoff.tree_digest(directory)

    def test_rust_diagnostics_preserve_hashes_and_only_bounded_known_receipt_text(self):
        sysroot = self.root / "synthetic-rust"
        receipts = sysroot / "lib/rustlib"
        receipts.mkdir(parents=True)
        (sysroot / "bin").mkdir()
        (sysroot / "bin/rustc.exe").write_bytes(b"synthetic compiler bytes")
        (receipts / "components").write_bytes(b"rustc\nrust-std\n")
        (receipts / "unrelated.txt").write_text("synthetic text must not be captured")
        (receipts / "multirust-config.toml").write_bytes(b"\xff")
        (receipts / "multirust-channel-manifest.toml").write_bytes(b"x" * (2 * 1024 * 1024 + 1))
        identity = handoff.tree_digest(sysroot / "lib")
        report = handoff.rust_diagnostics(sysroot)
        self.assertEqual(handoff.tree_digest(sysroot / "lib"), identity)
        self.assertEqual(report["files"]["bin/rustc.exe"]["sha256"], handoff.digest(sysroot / "bin/rustc.exe"))
        self.assertEqual(report["known_installer_receipts"]["components"]["utf8"], "rustc\nrust-std\n")
        self.assertNotIn("unrelated.txt", report["known_installer_receipts"])
        self.assertEqual(report["known_installer_receipts"]["multirust-config.toml"]["omitted"], "not UTF-8")
        self.assertEqual(report["known_installer_receipts"]["multirust-channel-manifest.toml"]["omitted"], "exceeds bounded receipt size")

    def test_receipt_order_is_compatible_but_raw_diagnostics_preserve_it(self):
        sysroot, records = self.rust_fixture()
        original = handoff.rust_toolchain(sysroot)
        diagnostic = handoff.rust_diagnostics(sysroot)
        self.expected["rust_toolchain"] = original
        self.proof["inputs_sha256"] = hashlib.sha256(handoff.encoded(self.expected)).hexdigest()
        components = sysroot / "lib/rustlib/components"
        components.write_bytes(b"\n".join(reversed(components.read_bytes()[:-1].split(b"\n"))) + b"\n")
        current = handoff.rust_toolchain(sysroot)
        self.assertEqual(current, original)
        self.assertNotEqual(handoff.rust_diagnostics(sysroot)["files"], diagnostic["files"])
        self.assertEqual(handoff.validate_producer(self.proof, dict(self.expected, rust_toolchain=current), self.api), 101)

    def test_receipt_semantic_changes_are_not_compatible(self):
        sysroot, records = self.rust_fixture()
        self.expected["rust_toolchain"] = handoff.rust_toolchain(sysroot)
        self.proof["inputs_sha256"] = hashlib.sha256(handoff.encoded(self.expected)).hexdigest()
        for field, value in (("pkg", "rustc-next"), ("target", "aarch64-pc-windows-msvc"), ("is_extension", True)):
            changed = copy.deepcopy(records)
            changed[1][field] = value
            self.write_receipts(sysroot, changed)
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "Independent consumer inputs"):
                handoff.validate_producer(self.proof, dict(self.expected, rust_toolchain=handoff.rust_toolchain(sysroot)), self.api)
            self.assert_closed()

    def test_component_membership_alone_is_bound_before_any_api_or_staging(self):
        sysroot, _ = self.rust_fixture()
        self.expected["rust_toolchain"] = handoff.rust_toolchain(sysroot)
        self.proof["inputs_sha256"] = hashlib.sha256(handoff.encoded(self.expected)).hexdigest()
        components = sysroot / "lib/rustlib/components"
        original = components.read_bytes()[:-1].split(b"\n")
        config = sysroot / "lib/rustlib/multirust-config.toml"
        config_before = config.read_bytes()
        for members in (original + [b"synthetic-tools-x86_64-pc-windows-msvc"], original[:-1], [b"rustc-next-x86_64-pc-windows-msvc", *original[1:]]):
            components.write_bytes(b"\n".join(members) + b"\n")
            self.assertEqual(config.read_bytes(), config_before)
            with self.subTest(members=members), self.assertRaisesRegex(ValueError, "Independent consumer inputs"):
                handoff.validate_producer(self.proof, dict(self.expected, rust_toolchain=handoff.rust_toolchain(sysroot)), self.api)
            self.assertEqual(self.requests, [])
            self.assert_closed()

    def test_non_component_receipts_remain_raw_including_schema_fields_and_order(self):
        sysroot, records = self.rust_fixture()
        self.expected["rust_toolchain"] = handoff.rust_toolchain(sysroot)
        self.proof["inputs_sha256"] = hashlib.sha256(handoff.encoded(self.expected)).hexdigest()
        for rows in (list(reversed(records)), [dict(records[0], is_extension="false"), *records[1:]], [dict(records[0], unknown_future_field="synthetic"), *records[1:]]):
            self.write_receipts(sysroot, rows)
            with self.subTest(rows=rows), self.assertRaisesRegex(ValueError, "Independent consumer inputs"):
                handoff.validate_producer(self.proof, dict(self.expected, rust_toolchain=handoff.rust_toolchain(sysroot)), self.api)
        self.write_receipts(sysroot, records, version="2")
        with self.assertRaisesRegex(ValueError, "Independent consumer inputs"):
            handoff.validate_producer(self.proof, dict(self.expected, rust_toolchain=handoff.rust_toolchain(sysroot)), self.api)

    def test_component_receipt_duplicates_invalid_data_and_missing_membership_fail_closed(self):
        for raw in (b"", b"rustc\nrustc\n", b" rustc\n", b"\xff", b"x" * (64 * 1024 + 1)):
            with self.subTest(raw_length=len(raw)), self.assertRaises(ValueError):
                handoff.rustup_components(raw)
        sysroot, _ = self.rust_fixture()
        (sysroot / "lib/rustlib/components").unlink()
        with self.assertRaisesRegex(ValueError, "receipt is missing"):
            handoff.rust_toolchain(sysroot)

    def test_component_receipt_accepts_only_lf_terminated_lines(self):
        expected = handoff.rustup_components(b"cargo\nrustc\n")
        self.assertEqual(handoff.rustup_components(b"rustc\ncargo\n"), expected)
        for separator in ("\r", "\r\n", "\v", "\f", "\u0085", "\u2028", "\u2029"):
            raw = ("cargo" + separator + "rustc\n").encode()
            with self.subTest(separator=repr(separator)), self.assertRaises(ValueError):
                handoff.rustup_components(raw)
        for raw in (b"cargo\nrustc", b"cargo\n\nrustc\n"):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                handoff.rustup_components(raw)

    def test_compiler_driver_and_standard_library_bytes_remain_required_inputs(self):
        sysroot, _ = self.rust_fixture()
        self.expected["rust_toolchain"] = handoff.rust_toolchain(sysroot)
        self.proof["inputs_sha256"] = hashlib.sha256(handoff.encoded(self.expected)).hexdigest()
        for name in ("bin/rustc_driver-synthetic.dll", "lib/rustlib/x86_64-pc-windows-msvc/lib/std.rlib"):
            path = sysroot / name
            original = path.read_bytes()
            path.write_bytes(original + b" changed")
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "Independent consumer inputs"):
                handoff.validate_producer(self.proof, dict(self.expected, rust_toolchain=handoff.rust_toolchain(sysroot)), self.api)
            self.assert_closed()
            path.write_bytes(original)

    def test_cli_refuses_wrong_repository_branch_head_and_run_before_acquisition(self):
        (self.root / "target").mkdir()
        handoff.write_json(self.root / "target/pilot-inputs.json", self.expected)
        environment = {
            "GITHUB_REPOSITORY": handoff.REPOSITORY,
            "GITHUB_REF": "refs/heads/" + handoff.BRANCH,
            "GITHUB_SHA": self.expected["commit"],
            "GITHUB_RUN_ID": "101",
            "PRODUCER_PROOF": json.dumps(self.proof),
        }
        for key, value in (("GITHUB_REPOSITORY", "synthetic/fork"), ("GITHUB_REF", "refs/heads/master"), ("GITHUB_SHA", "b" * 40), ("GITHUB_RUN_ID", "102")):
            with self.subTest(key=key), chdir(self.root), patch.dict(os.environ, dict(environment, **{key: value}), clear=True):
                with patch("sys.argv", ["candidate-handoff.py", "acquire", "target/release"]), patch.object(handoff, "acquire") as acquire:
                    with self.assertRaises(ValueError):
                        handoff.main()
                    acquire.assert_not_called()

    def test_workflow_is_isolated_and_retains_full_package_checks(self):
        root = Path(__file__).resolve().parents[2]
        workflow = (root / handoff.WORKFLOW).read_text()
        package = (root / "tools/ci/candidate-handoff.ps1").read_text()
        self.assertNotIn("pull_request", workflow)
        self.assertNotIn("actions: write", workflow)
        self.assertEqual(workflow.count("cache: 'false'"), 5)
        self.assertEqual(workflow.count("ref: ${{ github.sha }}"), 4)
        for forbidden in ("signing-input-", "unsigned-packages-", "release-candidate"):
            self.assertNotIn(forbidden, workflow)
        for check in ("package-windows.ps1 -SkipBuild", "check-windows-package.ps1", "sbom.py", "build-inventory.py"):
            self.assertIn(check, package)

    def test_inputs_only_probe_cannot_build_consume_or_measure_a_candidate(self):
        workflow = (Path(__file__).resolve().parents[2] / handoff.WORKFLOW).read_text()
        producer, other = workflow.split("  consumer:\n", 1)
        consumer, other = other.split("  control:\n", 1)
        control, measurement = other.split("  measurements:\n", 1)
        for name in ("Set package metadata", "Cold candidate-grade release build"):
            for block in (producer, control):
                self.assertIn(f"- name: {name}\n        if: ${{{{ !inputs.inputs_only }}}}", block)
        self.assertIn("id: seal\n        if: ${{ !inputs.inputs_only }}", producer)
        self.assertIn("id: upload\n        if: ${{ !inputs.inputs_only }}", producer)
        self.assertIn("if: ${{ !inputs.inputs_only &&", consumer)
        self.assertIn("if: always() && !inputs.inputs_only", measurement)
        self.assertIn("- uses: ./.github/actions/cargo-tool\n        if: ${{ !inputs.inputs_only }}", control)
        self.assertIn("- name: Installer, locked notices, SBOM, inventory and smoke checks\n        if: ${{ !inputs.inputs_only }}", control)
        self.assertIn("target/pilot-rust-files.json", producer)
        self.assertIn("target/pilot-rust-files.json", control)


if __name__ == "__main__":
    unittest.main()
