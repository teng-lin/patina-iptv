#!/usr/bin/env python3
"""The tier workflows keep required checks, exact-SHA releases and native coverage."""
import importlib.util
from pathlib import Path
import re
import tempfile
import unittest
from unittest.mock import Mock

ROOT = Path(__file__).resolve().parents[2]


def module(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), ROOT / "tools/ci" / (name + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


class Workflows(unittest.TestCase):
    def test_only_the_orchestrator_subscribes_validation_lanes_to_pr_push_and_schedule(self):
        for name in ("ci", "windows", "android", "coverage", "package-check", "windows-package-check",
                     "android-smoke", "import-stress", "fuzz", "ios", "credentials"):
            text = (ROOT / f".github/workflows/{name}.yml").read_text()
            triggers = text.split("\non:\n", 1)[1].split("\npermissions:", 1)[0]
            self.assertIn("  workflow_call:", triggers, name)
            for event in ("pull_request", "push", "schedule", "merge_group"):
                self.assertNotRegex(triggers, rf"(?m)^  {event}:", name)
        text = (ROOT / ".github/workflows/validation.yml").read_text()
        for event in ("pull_request", "push", "schedule", "merge_group"):
            self.assertRegex(text, rf"(?m)^  {event}:")
        self.assertIn("branches: [master]", text)

    def test_every_ci_helper_test_is_in_the_local_check_wrapper(self):
        wrapper = (ROOT / "tools/check.sh").read_text()
        for path in (ROOT / "tools/ci").glob("test-*.py"):
            self.assertIn(path.relative_to(ROOT).as_posix(), wrapper)

    def test_standalone_lanes_do_not_emit_legacy_required_names(self):
        host = (ROOT / ".github/workflows/ci.yml").read_text()
        android = (ROOT / ".github/workflows/android.yml").read_text()
        self.assertIn("name: host-tests (${{ matrix.os }})", host)
        self.assertIn("name: android-packaging", android)

    def test_native_validation_has_no_signing_environment(self):
        for name in ("ios", "credentials"):
            text = (ROOT / f".github/workflows/{name}.yml").read_text()
            self.assertNotIn("secrets.", text)
            self.assertNotIn("environment:", text)
            self.assertIn("timeout-minutes:", text)

    def test_non_pr_windows_runs_have_independent_concurrency_groups(self):
        text = (ROOT / ".github/workflows/windows.yml").read_text()
        self.assertIn("format('windows-run-{0}', github.run_id)", text)
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'pull_request' }}", text)

    def test_aggregate_waits_for_every_lane_and_current_protection_mirrors_it(self):
        plan = module("validation-plan")
        text = (ROOT / ".github/workflows/validation.yml").read_text()
        aggregate = text.split("  validation:\n", 1)[1].split("  protected-contexts:", 1)[0]
        needs = re.search(r"needs: \[(.*?)\]", aggregate)[1].split(", ")
        self.assertEqual(set(needs), {"plan", "helpers", *plan.LANES})
        self.assertIn("if: always()", aggregate)
        aliases = text.split("  protected-contexts:\n", 1)[1]
        self.assertIn("needs: validation", aliases)
        self.assertIn("if: always()", aliases)
        for context in ("test (ubuntu-24.04)", "test (macos-15)", "android"):
            self.assertIn(context, aliases)
        self.assertIn('test "$RESULT" = success', aliases)

    def test_release_explicitly_requests_candidate_tier_and_publishes_after_artifact_checks(self):
        text = (ROOT / ".github/workflows/release.yml").read_text()
        tests = text.split("  tests:\n", 1)[1].split("  windows-package:\n", 1)[0]
        self.assertIn("uses: ./.github/workflows/validation.yml", tests)
        self.assertIn("tier: release", tests)
        publish = text.split("  publish:\n", 1)[1].split("    runs-on:", 1)[0]
        self.assertIn("verify-windows", publish)
        self.assertIn("tests", publish)
        self.assertIn("verify-public-release.py", text)
        verify = text.split("  verify-windows:\n", 1)[1].split("  package:\n", 1)[0]
        self.assertNotIn("environment:", verify)
        self.assertNotIn("setup-windows", verify)
        self.assertIn("name: packages-windows", verify)

    def test_large_epg_case_is_not_silently_omitted(self):
        script = (ROOT / "tools/ci/import-stress.sh").read_text()
        self.assertIn("--release", script)
        self.assertIn("--test memory", script)
        self.assertIn("--include-ignored", script)
        self.assertNotIn("catalog_stream_memory::", script)

    def test_real_gtk_lifecycle_is_in_ordinary_linux_pr_validation(self):
        checks = module("check-plan")
        steps = {step["name"]: step["argv"] for step in checks.plan(tier="pr")}
        command = steps["native-gtk-lifecycle"]
        self.assertIn("tools/ci/run-xvfb.py", command)
        self.assertIn(checks.GTK_LIFECYCLE, command)
        self.assertIn("--ignored", command)
        self.assertIn("--exact", command)
        self.assertEqual(command[command.index("--features") + 1], checks.COVERAGE_FEATURES)


class FuzzCommand(unittest.TestCase):
    def test_unknown_duplicate_or_shell_target_is_rejected_before_any_build(self):
        fuzz = module("run-fuzz")
        for selection in ('["unknown"]', '["m3u", "m3u"]', '["m3u; true"]', '"m3u"', '[1]'):
            with self.subTest(selection=selection), self.assertRaises(ValueError):
                fuzz.command({"TARGETS": selection, "SECONDS_PER_TARGET": "30"})
        for seconds in ("0", "301", "bad"):
            with self.assertRaises(ValueError):
                fuzz.command({"SECONDS_PER_TARGET": seconds})
        self.assertEqual(fuzz.command({"TARGETS": '["m3u"]', "SECONDS_PER_TARGET": "30"}),
                         ["bash", "tools/ci/fuzz-imports.sh", "30", "m3u"])
        self.assertEqual(len(fuzz.command({"TARGETS": "[]"})), 12)


class PublicArtifacts(unittest.TestCase):
    def test_public_verification_rejects_modified_or_extra_assets_before_attestation(self):
        verifier = module("verify-public-release")
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            artifact = directory / "Synthetic.tar.gz"
            artifact.write_bytes(b"synthetic package bytes")
            verifier.release.write_checksums(directory)
            runner = Mock()
            verifier.verify(directory, "synthetic/repo", "a" * 40, "refs/tags/v1.0.0", runner)
            self.assertEqual(runner.call_count, 2, "both package and checksum file need provenance")
            for call in runner.call_args_list:
                self.assertIn("--source-digest", call.args[0])
                self.assertIn("a" * 40, call.args[0])
                self.assertIn("refs/tags/v1.0.0", call.args[0])
            runner.reset_mock()
            artifact.write_bytes(b"modified")
            with self.assertRaises(SystemExit):
                verifier.verify(directory, "synthetic/repo", "a" * 40, "refs/tags/v1.0.0", runner)
            runner.assert_not_called()
            verifier.release.write_checksums(directory)
            (directory / "extra.txt").write_text("unlisted")
            with self.assertRaises(SystemExit):
                verifier.verify(directory, "synthetic/repo", "a" * 40, "refs/tags/v1.0.0", runner)


if __name__ == "__main__":
    unittest.main()
