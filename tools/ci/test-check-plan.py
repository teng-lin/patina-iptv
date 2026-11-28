#!/usr/bin/env python3
"""Validate command selection, execution and target budgets without compiling Rust."""
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


def selected_tests(argv, flag="--test"):
    cargo = argv[:argv.index("--")]
    return [cargo[index + 1] for index, value in enumerate(cargo[:-1]) if value == flag]


class Parts(unittest.TestCase):
    """The two parallel CI parts run exactly the whole plan, split in two."""

    def test_parts_partition_every_check_of_the_whole_plan(self):
        for tier in ("pr", "full"):
            whole = [check["name"] for check in checks.plan(tier=tier)]
            app = [check["name"] for check in checks.plan(tier=tier, part="app")]
            core = [check["name"] for check in checks.plan(tier=tier, part="core")]
            with self.subTest(tier=tier):
                self.assertFalse(set(app) & set(core))
                self.assertEqual(sorted(app + core),
                                 sorted([name for name in whole if name != "workspace-tests"] + ["app-tests", "core-tests"]))
                self.assertEqual(app, list(checks.APP_PART_STEPS))
                # The strict timing gates still run alone, after the tests that build their harness
                # and the moved lints, never alongside them.
                self.assertEqual(app[0], "app-tests")
                gates = [app.index(name) for name in ("busy-guide-navigation", "busy-store-reader")]
                self.assertTrue(all(app.index(lint) < min(gates) for lint in checks.APP_STACK_LINTS))
                self.assertEqual(core[-1], "app-build")
                # Every moved lint exists in the Linux plan, so the split cannot silently drop one.
                self.assertTrue(set(checks.APP_STACK_LINTS) <= set(whole))
                self.assertFalse(set(checks.APP_STACK_LINTS) & set(core))

    def test_parts_build_the_same_graph_as_the_whole_test_step(self):
        whole = next(check["argv"] for check in checks.plan(tier="pr") if check["name"] == "workspace-tests")
        parts = {check["name"]: check["argv"] for part in checks.PARTS
                 for check in checks.plan(tier="pr", part=part)}
        prefix = whole[:whole.index("--tests")]
        options = whole[whole.index("--"):]
        for name in ("app-tests", "core-tests"):
            argv = parts[name]
            with self.subTest(step=name):
                self.assertEqual(argv[:len(prefix)], prefix)
                self.assertEqual(argv[argv.index("--"):], options)
                self.assertIn("--timings", argv)
                self.assertNotIn("--tests", argv)
        self.assertEqual(selected_tests(parts["app-tests"]), list(checks.APP_HARNESSES))
        self.assertIn("--lib", parts["core-tests"])
        self.assertNotIn("--lib", parts["app-tests"])
        self.assertEqual(sorted(selected_tests(parts["core-tests"]) + selected_tests(parts["app-tests"])),
                         checks.integration_tests())
        # Tested binaries are named; --bins would also build test = false ones.
        self.assertEqual(selected_tests(parts["core-tests"], "--bin"), checks.tested_binaries())
        self.assertEqual(selected_tests(parts["app-tests"], "--bin"), [])
        self.assertNotIn("--bins", parts["core-tests"])

    def test_integration_tests_follow_cargo_discovery(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[workspace]\nmembers = ["one", "two", "three"]\n')
            for member, extra in (("one", ""), ("two", '[[test]]\nname = "declared"\npath = "check/declared.rs"\n'),
                                  ("three", "")):
                (root / member / "tests").mkdir(parents=True)
                autotests = "autotests = false\n" if member == "three" else ""
                (root / member / "Cargo.toml").write_text(f'[package]\nname = "{member}"\n{autotests}{extra}')
            (root / "one/tests/app").mkdir()
            (root / "one/tests/app/main.rs").write_text("")
            (root / "one/tests/native_player.rs").write_text("")
            (root / "one/tests/app/scenario.rs").write_text("")
            (root / "two/tests/it").mkdir()
            (root / "two/tests/it/main.rs").write_text("")
            (root / "three/tests/ignored.rs").write_text("")
            self.assertEqual(checks.integration_tests(root), ["app", "declared", "it", "native_player"])
            # A renamed target claims its file; a target needing a feature the plan leaves
            # off is skipped, as --tests skips it, while one enabled through default is kept.
            (root / "two/tests/plain.rs").write_text("")
            (root / "two/Cargo.toml").write_text(
                '[package]\nname = "two"\n'
                '[features]\ndefault = ["native"]\nnative = ["deep"]\ndeep = []\noptional = []\n'
                '[[test]]\nname = "renamed"\npath = "tests/plain.rs"\n'
                '[[test]]\nname = "gated"\npath = "check/gated.rs"\nrequired-features = ["optional"]\n'
                '[[test]]\nname = "transitive"\npath = "check/transitive.rs"\nrequired-features = ["deep"]\n')
            self.assertEqual(checks.integration_tests(root), ["app", "it", "native_player", "renamed", "transitive"])
            # Glob members are expanded as Cargo expands them.
            (root / "Cargo.toml").write_text('[workspace]\nmembers = ["one", "t*"]\n')
            self.assertEqual(checks.integration_tests(root), ["app", "it", "native_player", "renamed", "transitive"])
            (root / "one/tests/native_player.rs").unlink()
            with self.assertRaisesRegex(ValueError, "harnesses are missing"):
                checks.integration_tests(root)

    def test_tested_binaries_follow_cargo_discovery(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[workspace]\nmembers = ["auto", "declared", "off"]\n')
            for member in ("auto", "declared", "off"):
                (root / member / "src/bin").mkdir(parents=True)
                (root / member / "src/main.rs").write_text("")
            (root / "auto/Cargo.toml").write_text('[package]\nname = "auto"\n')
            (root / "auto/src/bin/helper.rs").write_text("")
            # An explicit target claims src/main.rs under another name, keeping tests on.
            (root / "declared/Cargo.toml").write_text(
                '[package]\nname = "declared"\n[[bin]]\nname = "tool"\npath = "src/main.rs"\n')
            # test = false and autobins = false keep a package's binaries out.
            (root / "off/Cargo.toml").write_text(
                '[package]\nname = "off"\nautobins = false\n[[bin]]\nname = "off"\npath = "src/main.rs"\ntest = false\n')
            (root / "off/src/bin/extra.rs").write_text("")
            self.assertEqual(checks.tested_binaries(root), ["auto", "helper", "tool"])

    @unittest.skipUnless(shutil.which(os.environ.get("IPTV_REAL_CARGO") or "cargo")
                         and (checks.ROOT.parent / "nsh-patina").is_dir(), "needs Cargo and the NSH checkout")
    def test_parts_select_exactly_the_targets_cargo_tests(self):
        cargo = os.environ.get("IPTV_REAL_CARGO") or "cargo"
        metadata = json.loads(subprocess.check_output(
            [cargo, "metadata", "--locked", "--no-deps", "--format-version", "1"], cwd=checks.ROOT, text=True))
        tested = {(target["kind"][0] if target["kind"] != ["cdylib", "rlib"] else "lib", target["name"])
                  for package in metadata["packages"] for target in package["targets"]
                  if target.get("test") and target["kind"] != ["custom-build"]}
        libraries = {name for kind, name in tested if kind == "lib"}
        # --tests selects libraries, tested binaries and integration tests; the parts
        # select --lib plus each binary and integration test by name, so no other kind may opt in.
        self.assertLessEqual({kind for kind, _ in tested}, {"lib", "bin", "test"})
        self.assertTrue(libraries)
        self.assertEqual(sorted({name for kind, name in tested if kind == "test"}), checks.integration_tests())
        self.assertEqual(sorted({name for kind, name in tested if kind == "bin"}), checks.tested_binaries())

    def test_parts_reject_the_native_tier_and_unknown_parts(self):
        for options in ({"tier": "native", "part": "app"}, {"part": "everything"}):
            with self.subTest(options=options), self.assertRaises(ValueError):
                checks.plan(**options)
        for argv in (["list", "--tier", "native", "--part", "core"],
                     ["run", "--part", "core", "--harness-output", "harnesses.json"]):
            with self.subTest(argv=argv), redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                checks.main(argv)

    def test_app_part_records_the_performance_harnesses(self):
        runner = Mock(return_value=subprocess.CompletedProcess([], 0))
        app_tests = next(check for check in checks.plan(tier="pr", part="app") if check["name"] == "app-tests")
        with patch.object(checks, "record_harnesses", return_value=0) as record, redirect_stdout(io.StringIO()):
            self.assertEqual(checks.run([app_tests], runner=runner, harness_output=Path("harnesses.json")), 0)
        record.assert_called_once()
        runner.assert_not_called()


class Plan(unittest.TestCase):
    def setUp(self):
        self.steps = checks.plan()
        self.by_name = {step["name"]: step["argv"] for step in self.steps}

    def test_rustdoc_warnings_fail_on_linux_and_environment_reaches_child(self):
        rustdoc = next(check for check in self.steps if check["name"] == "workspace-rustdoc")
        self.assertEqual(rustdoc["env"], {"RUSTDOCFLAGS": "-D warnings"})
        self.assertIn("--no-deps", rustdoc["argv"])
        runner = Mock(return_value=subprocess.CompletedProcess([], 0))
        with redirect_stdout(io.StringIO()):
            self.assertEqual(checks.run([rustdoc], runner=runner), 0)
        self.assertEqual(runner.call_args.kwargs["env"]["RUSTDOCFLAGS"], "-D warnings")

    def test_workspace_test_run_records_both_app_executables(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "harnesses.json"
            artifact = {"reason": "compiler-artifact", "manifest_path": str(root / "crates/iptv-app/Cargo.toml")}
            messages = [
                dict(artifact, target={"name": name, "kind": ["test"]}, executable=str(root / name))
                for name in sorted(checks.PERFORMANCE_HARNESSES)
            ]
            stream = "\n".join([*(json.dumps(message) for message in messages), "test synthetic ... ok"])
            script = [sys.executable, "-c", f"print({stream!r})", "--"]
            logged = io.StringIO()
            with redirect_stdout(logged):
                self.assertEqual(checks.record_harnesses(script, {"cwd": root}, output, root), 0)
            self.assertEqual(logged.getvalue(), "test synthetic ... ok\n")
            self.assertEqual(json.loads(output.read_text()), {name: str(root / name) for name in checks.PERFORMANCE_HARNESSES})

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
        for name in ("workspace-tests", "workspace-doctests"):
            self.assertEqual(packages(self.by_name[name]), list(workspace))
        self.assertIn("--tests", self.by_name["workspace-tests"])
        self.assertNotIn("--lib", self.by_name["workspace-tests"])
        self.assertNotIn("--test", self.by_name["workspace-tests"])
        self.assertIn("--doc", self.by_name["workspace-doctests"])
        self.assertIn("--timings", self.by_name["workspace-tests"])
        self.assertIn("--timings", self.by_name["app-build"])

    def test_test_thread_override_only_applies_to_parallel_workspace_tests(self):
        self.assertNotIn("--test-threads", self.by_name["workspace-tests"])
        for tier in ("full", "pr"):
            for threads in (4, 6, 8):
                for check in checks.plan(tier=tier, test_threads=threads):
                    argv = check["argv"]
                    with self.subTest(tier=tier, threads=threads, check=check["name"]):
                        if check["name"] == "workspace-tests":
                            self.assertEqual(argv[argv.index("--test-threads") + 1], str(threads))
                            self.assertGreater(argv.index("--test-threads"), argv.index("--"))
                        else:
                            self.assertNotIn("--test-threads", argv)

    def test_integration_scenarios_share_at_most_two_executables_per_crate(self):
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
            cwd=checks.ROOT, text=True,
        ))
        for package in metadata["packages"]:
            targets = [target["name"] for target in package["targets"] if "test" in target["kind"]]
            with self.subTest(crate=package["name"]):
                self.assertLessEqual(
                    len(targets), 2,
                    f"each integration target costs a link; add scenarios to existing harnesses: {targets}",
                )

    def test_shared_commands_preserve_features_locks_and_named_libtest_output(self):
        self.assertEqual(
            self.by_name["workspace-clippy"],
            ["cargo", "clippy", "--locked", "--workspace", "--all-targets",
             "--features", checks.COVERAGE_FEATURES, "--", "-D", "warnings"],
        )
        for name, argv in self.by_name.items():
            with self.subTest(name=name):
                self.assertIn("--locked", argv)
                self.assertNotIn("--quiet", argv)
                if argv[1] == "test":
                    self.assertIn("--format=pretty", argv[argv.index("--") + 1:])
        for name in ["workspace-tests", "workspace-doctests", "repair-display-independence",
                     "inspection-display-budget", "busy-guide-navigation", "busy-store-reader"]:
            argv = self.by_name[name]
            self.assertEqual(argv[argv.index("--features") + 1], checks.COVERAGE_FEATURES)
            self.assertEqual(packages(argv), list(checks.workspace_crates()))

    def test_isolated_scenarios_have_exact_targets_and_are_not_in_parallel_suite(self):
        expected = {
            "repair-display-independence": (["--lib"], checks.REPAIR),
            "inspection-display-budget": (["--test", "it"], checks.INSPECTION),
            "busy-guide-navigation": (["--test", "app"], checks.LATENCY),
            "busy-store-reader": (["--test", "app"], checks.BUSY_READER),
        }
        for name, (target, test) in expected.items():
            argv = self.by_name[name]
            with self.subTest(name=name):
                self.assertEqual(packages(argv), list(checks.workspace_crates()))
                self.assertIn(target[0], argv[:argv.index("--")])
                if len(target) == 2:
                    self.assertEqual(argv[argv.index(target[0]) + 1], target[1])
                self.assertEqual(argv.count(test), 1)
                self.assertEqual(argv[argv.index("--") + 1:], ["--ignored", "--exact", "--format=pretty"])
        for name in ["workspace-tests", "workspace-doctests"]:
            self.assertNotIn("--ignored", self.by_name[name])

    def test_exact_filters_name_existing_ignored_tests(self):
        sources = {
            checks.REPAIR: "crates/iptv-runtime/src/guide_repair/tests.rs",
            checks.INSPECTION: "crates/iptv-runtime/tests/it/foundation_inspection.rs",
            checks.GTK_LIFECYCLE: "crates/iptv-app/tests/native_player.rs",
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

    def test_the_web_core_builds_for_wasm_outside_the_portable_set(self):
        self.assertNotIn("iptv-web", checks.PORTABLE)
        self.assertEqual(self.by_name["web-core-wasm"],
                         ["cargo", "build", "--locked", "-p", "iptv-web", "--target", "wasm32-unknown-unknown"])

    def test_the_store_builds_and_runs_its_tests_for_wasm_on_linux_only(self):
        self.assertEqual(self.by_name["store-wasm-check"],
                         ["cargo", "check", "--locked", "-p", "iptv-store-sqlite",
                          "--target", "wasm32-unknown-unknown", "--lib", "--tests"])
        self.assertEqual(self.by_name["store-wasm-clippy"],
                         ["cargo", "clippy", "--locked", "-p", "iptv-store-sqlite",
                          "--target", "wasm32-unknown-unknown", "--lib", "--tests", "--", "-D", "warnings"])
        self.assertEqual(self.by_name["store-wasm-tests"],
                         ["node", "tools/wasm-test.mjs", "--cargo", "--locked",
                          "-p", "iptv-store-sqlite", "--lib", "--test", "it"])
        self.assertTrue((checks.ROOT / "tools/wasm-test.mjs").is_file())
        for host in ("macos", "windows"):
            names = {step["name"] for step in checks.plan(host=host)}
            self.assertFalse(names & {"store-wasm-check", "store-wasm-clippy", "store-wasm-tests"})

    def test_the_browser_build_is_linted_and_tested_without_native_services(self):
        wasm = ["--target", "wasm32-unknown-unknown", "--", "-D", "warnings"]
        self.assertEqual(self.by_name["runtime-wasm"],
                         ["cargo", "clippy", "--locked", "-p", "iptv-runtime", "--no-default-features", *wasm])
        self.assertEqual(self.by_name["app-wasm"], ["cargo", "clippy", "--locked", "-p", "iptv-app", *wasm])
        self.assertEqual(self.by_name["web-adapters-wasm"],
                         ["cargo", "clippy", "--locked", "-p", "iptv-web", "--no-default-features", *wasm])
        self.assertEqual(self.by_name["runtime-portable-tests"],
                         ["cargo", "test", "--locked", "-p", "iptv-runtime", "--no-default-features", "--lib",
                          "--", "--format=pretty"])
        self.assertEqual(self.by_name["tizen-nsh-wasm"],
                         ["cargo", "clippy", "--locked", "--manifest-path", "hosts/tizen-nsh/Cargo.toml", *wasm])
        self.assertTrue((checks.ROOT / "hosts/tizen-nsh/Cargo.lock").is_file())
        for host in ("macos", "windows"):
            names = {step["name"] for step in checks.plan(host=host)}
            self.assertFalse({"runtime-wasm", "app-wasm", "web-adapters-wasm", "runtime-portable-tests",
                              "tizen-nsh-wasm"} & names)

    def test_release_only_changes_the_normal_app_build(self):
        release = {step["name"]: step["argv"] for step in checks.plan(release=True)}
        self.assertEqual(self.by_name.keys(), release.keys())
        for name, argv in self.by_name.items():
            self.assertEqual(release[name], argv + (["--release"] if name == "app-build" else []))
        self.assertNotIn("--features", self.by_name["app-build"], "normal build must not enable test-support")
        self.assertEqual(self.steps[-1]["name"], "app-build", "test-support binaries must not replace the final normal app")

    def test_coverage_uses_every_workspace_crate_and_both_test_support_features(self):
        argv = checks.coverage_args()
        self.assertIn("--tests", argv, "coverage must not link unused examples")
        self.assertEqual(packages(argv), list(checks.workspace_crates()))
        self.assertEqual(argv[argv.index("--features") + 1].split(","),
                         ["iptv-runtime/test-support", "iptv-app/test-support"])

    def test_pr_runs_correctness_once_and_defers_allocator_measurements(self):
        pr = {step["name"]: step["argv"] for step in checks.plan(tier="pr")}
        self.assertEqual(set(pr), set(self.by_name))
        command = pr["workspace-tests"]
        self.assertEqual(packages(command), list(checks.workspace_crates()))
        for module in ("catalog_stream_memory::", "epg_memory::", "epg_stream_memory::"):
            self.assertIn(module, command[command.index("--") + 1:])
        self.assertNotIn("--skip", self.by_name["workspace-tests"])

    def test_native_tier_compiles_tests_and_runs_only_platform_suites(self):
        for host in ("macos", "windows"):
            native = {step["name"]: step["argv"] for step in checks.plan(tier="native", host=host)}
            expected = {"native-test-compilation", "native-player-tests", "app-build"}
            if host == "windows":
                expected.add("windows-updater-tests")
                self.assertIn("app_updates::windows::tests", native["windows-updater-tests"])
                expected.add("windows-app-tests")
                app = native["windows-app-tests"]
                self.assertEqual(packages(app), list(checks.workspace_crates()))
                self.assertEqual([app[i + 1] for i, argument in enumerate(app) if argument == "--test"],
                                 list(checks.APP_HARNESSES))
                self.assertNotIn("--skip", app)
            self.assertEqual(set(native), expected)
            self.assertIn("--all-targets", native["native-test-compilation"])
            self.assertNotIn("--features", native["app-build"])
            full = checks.plan(tier="full", host=host)
            self.assertTrue(any(step["name"] == "workspace-tests" for step in full))
            self.assertFalse(any(step["name"].startswith("wasm-") for step in full))
            self.assertFalse(any(step["name"] == "workspace-doctests" for step in full))
            self.assertFalse(any(step["name"] == "native-gtk-lifecycle" for step in full))

    def test_packaged_mac_keeps_the_same_tests_and_reuses_the_app_build(self):
        for tier in ("native", "full"):
            with self.subTest(tier=tier):
                standalone = checks.plan(tier=tier, host="macos")
                self.assertEqual(standalone[-1]["name"], "app-build")
                self.assertEqual(checks.plan(tier=tier, host="macos", skip_app_build=True), standalone[:-1])

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
