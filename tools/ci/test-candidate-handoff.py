#!/usr/bin/env python3
"""Synthetic provenance, hostile archive, fallback and measurement contracts."""

import copy
import hashlib
import importlib.util
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
        self.assertEqual(handoff.measurements(runs), {"n": 21, "successful_n": 20, "failures": 1, "p50_seconds": 10.5, "p90_seconds": 18})
        self.assertIsNone(handoff.measurements([])["p90_seconds"])

    def test_actual_parallel_endpoints_and_failure_counts(self):
        jobs = []
        for identifier, name, endpoint in ((201, handoff.PRODUCER, 100), (202, "Pilot consumer", 160), (203, "Cold control", 130)):
            jobs.append({"id": identifier, "name": name, "attempt": 1, "conclusion": "success", "trigger_to_completion_seconds": endpoint, "queue_seconds": 10, "execution_seconds": 90})
        timeline = {"run_id": 101, "commit": "a" * 40, "decision": "reuse", "jobs": jobs}
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
        first = {"run_id": 101, "commit": "a" * 40, "decision": "reuse", "jobs": []}
        for field, value in (("commit", "b" * 40), ("decision", "expired"), ("run_id", 101)):
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
        timeline = {"run_id": 101, "attempt": 2, "commit": "a" * 40, "decision": "reuse", "jobs": jobs}
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

        timeline = handoff.timeline(101, api)
        self.assertEqual(timeline["jobs"][0]["queue_seconds"], 10)
        self.assertEqual(timeline["jobs"][0]["execution_seconds"], 0)
        self.assertEqual(timeline["jobs"][0]["trigger_to_completion_seconds"], 12)

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


if __name__ == "__main__":
    unittest.main()
