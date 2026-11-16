#!/usr/bin/env python3
"""Check finished-tool cache identities without installing or downloading tools."""
from contextlib import redirect_stdout
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
import textwrap
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
ACTION = Path(".github/actions/cargo-tool/action.yml")
RECIPE = (ROOT / ACTION).read_text(encoding="utf-8")
IDENTITY = textwrap.dedent(RECIPE.split('python3 - <<\'PY\' >> "$GITHUB_OUTPUT"\n', 1)[1].split("        PY\n", 1)[0])


class ToolCache(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        previous = Path.cwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, previous)
        ACTION.parent.mkdir(parents=True)
        ACTION.write_text(RECIPE, encoding="utf-8")
        self.environment = {
            "RUNNER_TEMP": str(self.root / "runner"),
            "CARGO_HOME": str(self.root / "cargo"),
            "TOOL_NAME": "cargo-about",
            "TOOL_VERSION": "0.9.3",
            "TOOL_FEATURES": "cli",
        }

    def identity(self, compiler="generated compiler identity", **environment):
        output = io.StringIO()
        with patch.dict(os.environ, self.environment | environment, clear=True), redirect_stdout(output), \
                patch("subprocess.check_output", return_value=compiler):
            exec(compile(IDENTITY, str(ACTION), "exec"), {})
        return dict(line.split("=", 1) for line in output.getvalue().splitlines())

    def test_application_dependencies_and_sources_do_not_invalidate_finished_tools(self):
        before = self.identity()
        Path("Cargo.lock").write_text("generated dependency change\n")
        Path("app.rs").write_text("generated application change\n")
        self.assertEqual(self.identity(), before)
        self.assertEqual(Path(before["path"]), Path(before["root"]) / "bin/cargo-about")

    def test_explicit_cargo_home_never_requires_a_system_home_directory(self):
        with patch.object(Path, "home", side_effect=RuntimeError("no system home")):
            self.assertEqual(Path(self.identity()["root"]),
                             Path(self.environment["RUNNER_TEMP"]) / "patina-cargo-tools/cargo-about")

    def test_tool_version_features_and_compiler_flags_are_distinct(self):
        before = self.identity()["key"]
        self.assertNotEqual(before, self.identity(compiler="different generated compiler")["key"])
        for environment in (
            {"TOOL_NAME": "cargo-deny"}, {"TOOL_VERSION": "0.9.4"},
            {"TOOL_FEATURES": ""}, {"RUSTFLAGS": "-C target-cpu=generated"},
        ):
            with self.subTest(environment=environment):
                self.assertNotEqual(before, self.identity(**environment)["key"])

    def test_recipe_and_global_config_invalidate_the_tool(self):
        before = self.identity()["key"]
        ACTION.write_text(RECIPE + "\n# generated recipe change\n", encoding="utf-8")
        changed = self.identity()["key"]
        self.assertNotEqual(before, changed)
        config = Path(self.environment["CARGO_HOME"]) / "config.toml"
        config.parent.mkdir()
        config.write_text('[build]\nrustflags = ["--cfg=generated"]\n')
        configured = self.identity()["key"]
        self.assertNotEqual(changed, configured)
        Path('.cargo').mkdir()
        Path('.cargo/config.toml').write_text('[build]\nrustflags = ["--cfg=generated_workspace"]\n')
        self.assertNotEqual(configured, self.identity()["key"])

    def test_invalid_executable_names_are_rejected(self):
        with self.assertRaisesRegex(SystemExit, "invalid Cargo tool name"):
            self.identity(TOOL_NAME="../generated")

    def test_release_setup_disables_tool_restore_and_save(self):
        setup = (ROOT / ".github/actions/setup/action.yml").read_text(encoding="utf-8")
        self.assertIn("cache: ${{ inputs.cache }}", setup)
        self.assertIn("if: inputs.cache == 'true'", RECIPE)
        self.assertIn("github.ref == 'refs/heads/master'", RECIPE)
        self.assertIn("steps.tool.outputs.cache-hit != 'true'", RECIPE)

    def test_packaging_cache_has_a_master_producer_on_dependency_changes(self):
        workflow = (ROOT / ".github/workflows/validation.yml").read_text(encoding="utf-8")
        self.assertIn("  push:\n    branches: [master]", workflow)
        self.assertIn("uses: ./.github/workflows/package-check.yml", workflow)
        spec = importlib.util.spec_from_file_location("validation", ROOT / "tools/ci/validation-plan.py")
        validation = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(validation)
        for path in ("Cargo.lock", "nsh.rev", ".github/actions/setup/action.yml"):
            self.assertTrue(validation.select([path])["packages"], path)

    @unittest.skipUnless(os.name == "posix" and shutil.which("bash"), "composite installation uses Bash")
    def test_tool_install_uses_its_own_root_and_fixed_bootstrap_profile(self):
        install = re.search(r"(?ms)^      run: \|\n(        args=\(\).*?)(?=^    #)", RECIPE)
        self.assertIsNotNone(install)
        binary = self.root / "bin"
        binary.mkdir()
        cargo = binary / "cargo"
        cargo.write_text(f"#!{sys.executable}\n" + '''import json, os, pathlib, sys
args = sys.argv[1:]
root = pathlib.Path(args[args.index("--root") + 1])
tool = root / "bin" / args[1]
tool.parent.mkdir(parents=True)
tool.write_text("generated executable")
tool.chmod(0o755)
pathlib.Path(os.environ["INSTALL_LOG"]).write_text(json.dumps({
    "args": args,
    "profile": [os.environ[k] for k in ("CARGO_PROFILE_RELEASE_LTO",
                "CARGO_PROFILE_RELEASE_OPT_LEVEL", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS")],
}))
''', encoding="utf-8")
        cargo.chmod(0o755)
        root = self.root / "finished tool"
        log = self.root / "install.json"
        environment = dict(os.environ, **self.environment, TOOL_ROOT=str(root), INSTALL_LOG=str(log),
                           CARGO_PROFILE_RELEASE_LTO="false", CARGO_PROFILE_RELEASE_OPT_LEVEL="1",
                           CARGO_PROFILE_RELEASE_CODEGEN_UNITS="16")
        environment["PATH"] = str(binary) + os.pathsep + os.defpath
        subprocess.run(["bash", "-euo", "pipefail", "-c", textwrap.dedent(install[1])],
                       env=environment, check=True)
        recorded = json.loads(log.read_text())
        self.assertEqual(recorded["args"], ["install", "cargo-about", "--version", "0.9.3",
                                            "--locked", "--root", str(root), "--features", "cli"])
        self.assertEqual(recorded["profile"], ["false", "1", "16"])
        self.assertTrue((root / "bin/cargo-about").is_file())


if __name__ == "__main__":
    unittest.main()
