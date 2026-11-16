#!/usr/bin/env python3
"""Validate command selection and execution without launching Cargo."""
from contextlib import redirect_stderr, redirect_stdout
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

SPEC = importlib.util.spec_from_file_location("check_plan", Path(__file__).with_name("check-plan.py"))
checks = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(checks)


def packages(argv):
    return [argv[index + 1] for index, value in enumerate(argv[:-1]) if value == "-p"]


class Plan(unittest.TestCase):
    def setUp(self):
        self.steps = checks.plan()
        self.by_name = {step["name"]: step["argv"] for step in self.steps}

    def test_workspace_inventory_reads_package_names_not_directory_names(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["generated-directory"]\n')
            (root / "generated-directory").mkdir()
            (root / "generated-directory/Cargo.toml").write_text('[package]\nname="generated-package"\n')
            self.assertEqual(checks.workspace_crates(root), ("generated-package",))

    def test_inventory_refuses_identifiers_that_would_split_coverage_arguments(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["generated"]\n')
            (root / "generated").mkdir()
            for name in ["generated --features unexpected", "generated\tflag", "-p", ""]:
                with self.subTest(name=name):
                    (root / "generated/Cargo.toml").write_text('[package]\nname=' + json.dumps(name) + '\n')
                    with self.assertRaisesRegex(ValueError, "plain Cargo identifiers"):
                        checks.workspace_crates(root)

    def test_every_workspace_crate_is_formatted_and_tested_once(self):
        workspace = checks.workspace_crates()
        self.assertEqual(len(workspace), len(set(workspace)))
        self.assertEqual(packages(checks.format_plan()[0]["argv"]), list(workspace))
        libraries = packages(self.by_name["library-tests"])
        self.assertEqual(libraries, [crate for crate in workspace if crate != "iptv-app"])
        self.assertEqual(packages(self.by_name["app-tests"]), ["iptv-app"])
        self.assertNotIn("--lib", self.by_name["app-tests"])
        self.assertNotIn("--test", self.by_name["app-tests"], "default tests include all ordinary app targets/doc tests")

    def test_shared_commands_preserve_features_locks_and_named_libtest_output(self):
        self.assertEqual(
            self.by_name["workspace-clippy"],
            ["cargo", "clippy", "--locked", "--workspace", "--all-targets",
             "--features", "iptv-app/test-support", "--", "-D", "warnings"],
        )
        library = self.by_name["library-tests"]
        self.assertEqual(library[library.index("--features") + 1], "iptv-runtime/test-support")
        for name, argv in self.by_name.items():
            with self.subTest(name=name):
                self.assertIn("--locked", argv)
                self.assertNotIn("--quiet", argv)
                if argv[1] == "test":
                    self.assertIn("--format=pretty", argv[argv.index("--") + 1:])
        for name in ["app-tests", "guide-input-budget", "busy-store-reader"]:
            argv = self.by_name[name]
            self.assertEqual(argv[argv.index("--features") + 1], "test-support")

    def test_isolated_scenarios_have_exact_targets_and_are_not_in_parallel_suite(self):
        expected = {
            "repair-display-independence": ("iptv-runtime", ["--lib"], checks.REPAIR),
            "guide-input-budget": ("iptv-app", ["--test", "app"], checks.LATENCY),
            "busy-store-reader": ("iptv-app", ["--test", "app"], checks.BUSY_READER),
        }
        for name, (package, target, test) in expected.items():
            argv = self.by_name[name]
            with self.subTest(name=name):
                self.assertEqual(packages(argv), [package])
                self.assertIn(target[0], argv[:argv.index("--")])
                if len(target) == 2:
                    self.assertEqual(argv[argv.index(target[0]) + 1], target[1])
                self.assertEqual(argv.count(test), 1)
                self.assertEqual(argv[argv.index("--") + 1:], ["--ignored", "--exact", "--format=pretty"])
        for name in ["library-tests", "app-tests"]:
            self.assertNotIn("--ignored", self.by_name[name])

    def test_exact_filters_name_existing_ignored_tests(self):
        sources = {
            checks.REPAIR: "crates/iptv-runtime/src/guide_repair/tests.rs",
            checks.LATENCY: "crates/iptv-app/tests/app/store_latency.rs",
            checks.BUSY_READER: "crates/iptv-app/tests/app/store_latency.rs",
        }
        for selector, path in sources.items():
            name = selector.split("::")[-1]
            text = (checks.ROOT / path).read_text(encoding="utf-8")
            with self.subTest(selector=selector):
                found = re.search(r"(?m)^fn " + re.escape(name) + r"\(", text)
                self.assertIsNotNone(found, "an exact libtest filter must not silently select zero tests")
                attributes = text[:found.start()].rsplit("\n}", 1)[-1]
                self.assertIn("#[test]", attributes)
                self.assertIn("#[ignore", attributes)

    def test_portable_builds_have_the_exact_target_and_exclude_native_crates(self):
        portable = [step for step in self.steps if step["name"].startswith("wasm-")]
        self.assertEqual(len(portable), len(checks.PORTABLE))
        for crate, step in zip(checks.PORTABLE, portable, strict=True):
            self.assertEqual(step["argv"], ["cargo", "build", "--locked", "-p", crate,
                                            "--target", "wasm32-unknown-unknown"])
        self.assertLess(set(checks.PORTABLE), set(checks.workspace_crates()) - {"iptv-app"})

    def test_release_only_changes_the_normal_app_build(self):
        release = {step["name"]: step["argv"] for step in checks.plan(release=True)}
        self.assertEqual(self.by_name.keys(), release.keys())
        for name, argv in self.by_name.items():
            self.assertEqual(release[name], argv + (["--release"] if name == "app-build" else []))
        self.assertNotIn("--features", self.by_name["app-build"], "normal build must not enable test-support")
        self.assertEqual(self.steps[-1]["name"], "app-build", "test-support binaries must not replace the final normal app")

    def test_coverage_uses_every_workspace_crate_and_both_test_support_features(self):
        argv = checks.coverage_args()
        self.assertEqual(packages(argv), list(checks.workspace_crates()))
        self.assertEqual(argv[argv.index("--features") + 1].split(","),
                         ["iptv-runtime/test-support", "iptv-app/test-support"])

    def test_runner_streams_argv_in_root_and_stops_at_first_failure(self):
        runner = Mock(side_effect=[subprocess.CompletedProcess([], 0), subprocess.CompletedProcess([], 7)])
        steps = [checks.step("first", "generated-tool", "argument with spaces"),
                 checks.step("failed", "generated-tool", "second"),
                 checks.step("never", "generated-tool", "third")]
        output, errors = io.StringIO(), io.StringIO()
        with patch.dict(os.environ, {}, clear=True), redirect_stdout(output), redirect_stderr(errors):
            self.assertEqual(checks.run(steps, root=Path("/generated/root"), runner=runner), 7)
        self.assertEqual(runner.call_count, 2)
        for call in runner.call_args_list:
            self.assertIsInstance(call.args[0], list)
            self.assertEqual(call.kwargs, {"cwd": Path("/generated/root"), "check": False})
        self.assertIn("==> first", output.getvalue())
        self.assertNotIn("==> never", output.getvalue())
        self.assertIn("failed (exit 7)", errors.getvalue())

    def test_mac_child_restores_selected_runtime_without_mutating_parent_environment(self):
        environment = {"IPTV_RS_MPV_LIB_DIR": "/generated/runtime", "PATH": "/generated/bin"}
        for inherited in (None, "/generated/other-runtime"):
            original = dict(environment)
            if inherited is not None:
                original["DYLD_LIBRARY_PATH"] = inherited
            runner = Mock(return_value=subprocess.CompletedProcess([], 0))
            with patch.dict(os.environ, original, clear=True), patch.object(sys, "platform", "darwin"):
                with redirect_stdout(io.StringIO()):
                    self.assertEqual(checks.run([checks.step("synthetic", "generated-tool")], runner=runner), 0)
                self.assertEqual(dict(os.environ), original)
            self.assertEqual(runner.call_args.kwargs["env"],
                             dict(original, DYLD_LIBRARY_PATH="/generated/runtime"))

    def test_other_hosts_keep_the_inherited_environment(self):
        original = {"IPTV_RS_MPV_LIB_DIR": "/generated/runtime", "DYLD_LIBRARY_PATH": "/generated/other"}
        runner = Mock(return_value=subprocess.CompletedProcess([], 0))
        with patch.dict(os.environ, original, clear=True), patch.object(sys, "platform", "linux"):
            with redirect_stdout(io.StringIO()):
                self.assertEqual(checks.run([checks.step("synthetic", "generated-tool")], runner=runner), 0)
            self.assertEqual(dict(os.environ), original)
        self.assertNotIn("env", runner.call_args.kwargs)

    def test_missing_tool_fails_without_starting_following_checks(self):
        runner = Mock(side_effect=FileNotFoundError("generated executable is absent"))
        with patch.dict(os.environ, {}, clear=True), redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            self.assertEqual(checks.run(self.steps, runner=runner), 127)
        self.assertEqual(runner.call_count, 1)

    def test_plan_listing_never_needs_cargo_or_the_current_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run(
                [sys.executable, str(Path(checks.__file__)), "list", "--json"],
                cwd=directory, env=dict(os.environ, PATH=""), capture_output=True, text=True, check=True,
            )
        self.assertEqual(json.loads(result.stdout), self.steps)
        self.assertEqual(result.stderr, "")


@unittest.skipUnless(os.name != "nt" and shutil.which("bash"), "coverage.sh is a Unix wrapper")
class CustomCoverage(unittest.TestCase):
    def test_custom_arguments_reach_fake_cargo_without_extra_test_filters_or_features(self):
        cases = [
            ["-p", "generated-crate"],
            ["-p", "generated-crate", "--", "--nocapture"],
            ["-p", "generated-crate", "--", "--test-threads=1", "--format=terse"],
        ]
        for arguments in cases:
            with self.subTest(arguments=arguments), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                script = root / "tools/ci/coverage.sh"
                script.parent.mkdir(parents=True)
                shutil.copyfile(checks.ROOT / "tools/ci/coverage.sh", script)
                binary = root / "bin"
                binary.mkdir()
                cargo = binary / "cargo"
                cargo.write_text(f"#!{sys.executable}\n" + '''import json, os, sys
args = sys.argv[1:]
with open(os.environ["COVERAGE_ARGV_LOG"], "a") as stream:
    stream.write(json.dumps(args) + "\\n")
if "--" in args and "--" in args[args.index("--") + 1:]:
    raise SystemExit("extra separator would turn output options into a zero-match test filter")
print("synthetic coverage output")
''', encoding="utf-8")
                cargo.chmod(0o755)
                log = root / "argv.jsonl"
                result = subprocess.run(
                    [shutil.which("bash"), str(script), *arguments], capture_output=True, text=True,
                    env={"PATH": str(binary) + os.pathsep + os.defpath, "COVERAGE_ARGV_LOG": str(log)},
                    check=False,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                calls = [json.loads(line) for line in log.read_text().splitlines()]
                measured = [argv for argv in calls if "--no-report" in argv]
                self.assertEqual(measured, [["llvm-cov", "--locked", "--no-report", *arguments]])


if __name__ == "__main__":
    unittest.main()
