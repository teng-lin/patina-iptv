#!/usr/bin/env python3
"""Every platform's checks and the contributor map agree on workspace ownership.

Linux/macOS and Windows delegate shared Rust checks to one command plan.
Assert features, exact isolated scenarios, test output and delegation as well
as workspace/portable inventories; coverage uses the same inventory.
"""
from pathlib import Path
import importlib.util
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = re.compile(r"-p\s+([a-z0-9-]+)")
SPEC = importlib.util.spec_from_file_location("check_plan", ROOT / "tools/ci/check-plan.py")
plan = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(plan)


def lines_with(path, word):
    return [line for line in (ROOT / path).read_text(encoding="utf-8").splitlines() if word in line and not line.lstrip().startswith("#")]


def packages(lines):
    return {name for line in lines for name in PACKAGE.findall(line)}


def local_targets(markdown):
    """File/directory links only; external links and section anchors are excluded."""
    return {
        target.split("#", 1)[0]
        for target in re.findall(r"\]\(([^\s)]+)\)", markdown)
        if not re.match(r"[a-zA-Z][a-zA-Z0-9+.-]*:", target)
        and not target.startswith("#")
    }


class Parity(unittest.TestCase):
    def setUp(self):
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        self.members = {Path(member).name for member in manifest["workspace"]["members"]}
        self.libraries = self.members - {"iptv-app"}
        self.wasm = set(plan.PORTABLE)

    def test_reference_script_tests_and_formats_every_crate(self):
        self.assertEqual(set(plan.workspace_crates()), self.members)
        format_argv = plan.format_plan()[0]["argv"]
        self.assertEqual({format_argv[i + 1] for i, arg in enumerate(format_argv[:-1]) if arg == "-p"}, self.members)
        script = (ROOT / "tools/check.sh").read_text(encoding="utf-8")
        self.assertEqual(script.count("python3 tools/ci/check-plan.py run\n"), 1)
        self.assertEqual(script.count("python3 tools/ci/check-plan.py format\n"), 1)
        self.assertEqual(lines_with("tools/check.sh", "cargo test"), [])
        self.assertLess(self.wasm, self.libraries)

    def test_windows_check_delegates_the_same_features_scenarios_and_output(self):
        script = (ROOT / "tools/build-windows.ps1").read_text(encoding="utf-8")
        self.assertEqual(script.count('$checkArgs = @("$PSScriptRoot/ci/check-plan.py", \'run\')'), 1)
        self.assertEqual(script.count("& python @checkArgs"), 1)
        self.assertIn("if ($Release) { $checkArgs += '--release' }", script)
        self.assertIn("if (!$Check)", script, "-Check's normal app build must not run twice")
        native = lines_with("tools/build-windows.ps1", "cargo test")
        self.assertEqual(len(native), 1)
        self.assertIn("nsh-platform-windows --lib chrome::tests -- --format=pretty", native[0])
        self.assertIn("pins.py\" check-nsh", script)
        steps = {step["name"]: step["argv"] for step in plan.plan()}
        tests = steps["workspace-tests"]
        self.assertEqual(tests[tests.index("--features") + 1], plan.COVERAGE_FEATURES)
        self.assertIn("--tests", tests)
        self.assertIn("--doc", steps["workspace-doctests"])
        for name in ("repair-display-independence", "inspection-display-budget",
                     "guide-input-budget", "busy-store-reader"):
            argv = steps[name]
            self.assertEqual(argv[argv.index("--") + 1:], ["--ignored", "--exact", "--format=pretty"])

    def test_coverage_measures_every_workspace_crate(self):
        args = plan.coverage_args()
        self.assertEqual({args[i + 1] for i, arg in enumerate(args[:-1]) if arg == "-p"}, self.members)
        script = (ROOT / "tools/ci/coverage.sh").read_text(encoding="utf-8")
        self.assertIn("arguments=$(python3 tools/ci/check-plan.py coverage-args)", script)
        self.assertIn('read -r -a checks <<< "$arguments"', script)
        self.assertIn('"${checks[@]}" -- --format=pretty', script)
        self.assertIn(plan.INSPECTION + " \\\n    -- --ignored --exact --format=pretty", script)

    def test_coverage_enables_native_runtime_and_headless_application_scenarios(self):
        args = plan.coverage_args()
        self.assertEqual(args[args.index("--features") + 1].split(","),
                         ["iptv-runtime/test-support", "iptv-app/test-support"])

    def test_coverage_runs_the_real_native_player_on_an_isolated_display(self):
        script = (ROOT / "tools/ci/coverage.sh").read_text(encoding="utf-8")
        self.assertIn("python3 tools/ci/run-xvfb.py cargo llvm-cov test", script)
        self.assertIn("--test native_player", script)
        self.assertIn("--ignored --exact native_gtk_player_shares_pip_frames_and_recreates_only_after_context_loss", script)

    def test_contributor_map_lists_the_portable_builds(self):
        text = (ROOT / "docs/contributing.md").read_text(encoding="utf-8")
        block = re.search(
            r"<!-- portable-crates:start -->(.*?)<!-- portable-crates:end -->",
            text,
            re.S,
        )
        self.assertIsNotNone(block, "contributor map needs the portable crate inventory")
        self.assertEqual(set(re.findall(r"`(iptv-[a-z-]+)`", block[1])), self.wasm)

    def test_contributor_map_local_links_exist(self):
        page = ROOT / "docs/contributing.md"
        targets = local_targets(page.read_text(encoding="utf-8"))
        self.assertTrue(targets, "the ownership map must link to implementation/tests")
        for target in sorted(targets):
            with self.subTest(target=target):
                self.assertTrue((page.parent / target).exists(), f"broken map link: {target}")

    def test_documented_packages_and_scenario_modules_exist(self):
        text = (ROOT / "docs/contributing.md").read_text(encoding="utf-8")
        self.assertLessEqual(set(PACKAGE.findall(text)), self.members)
        modules = re.findall(r"--test app ([a-z_]+)", text)
        self.assertTrue(modules)
        for module in modules:
            self.assertTrue((ROOT / f"crates/iptv-app/tests/app/{module}.rs").is_file())

    def test_local_target_parser_ignores_external_links_and_sections(self):
        self.assertEqual(
            local_targets("[a](../tools/check.sh#example) [b](architecture.md) "
                          "[c](#section) [d](https://example.test/path) "
                          "[e](mailto:generated@example.test)"),
            {"../tools/check.sh", "architecture.md"},
        )


if __name__ == "__main__":
    unittest.main()
