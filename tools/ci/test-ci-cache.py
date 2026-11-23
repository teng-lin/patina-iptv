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
            "RUNNER_OS": "Windows" if os.name == "nt" else "Linux",
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
        executable = "cargo-about.exe" if os.name == "nt" else "cargo-about"
        self.assertEqual(Path(before["path"]), Path(before["root"]) / "bin" / executable)

    def test_tool_cache_targets_the_installed_executable_on_each_platform(self):
        for platform, executable in (("Windows", "cargo-about.exe"), ("Linux", "cargo-about"),
                                     ("macOS", "cargo-about")):
            with self.subTest(platform=platform):
                identity = self.identity(RUNNER_OS=platform)
                self.assertEqual(Path(identity["path"]), Path(identity["root"]) / "bin" / executable)

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

    def test_clean_shipping_builds_can_reuse_exact_tools_without_bulk_cargo_cache(self):
        setup = (ROOT / ".github/actions/setup/action.yml").read_text(encoding="utf-8")
        self.assertIn("cache: ${{ inputs.tool-cache }}", setup)
        self.assertIn("if: inputs.cache == 'true'", setup)
        self.assertIn("if: inputs.cache == 'true'", RECIPE)
        self.assertIn("github.ref == 'refs/heads/master'", RECIPE)
        self.assertIn("steps.tool.outputs.cache-hit != 'true'", RECIPE)

    def test_packaging_cache_uses_the_generated_graph_on_previews_and_nightly(self):
        workflow = (ROOT / ".github/workflows/validation.yml").read_text(encoding="utf-8")
        self.assertIn("  schedule:", workflow)
        self.assertIn("uses: ./.github/workflows/package-check.yml", workflow)
        package = (ROOT / ".github/workflows/package-check.yml").read_text(encoding="utf-8")
        self.assertIn("workspaces: crates/iptv-app/.nsh/rust -> ../../../../target/desktop-cargo", package)
        self.assertEqual(package.count("CARGO_TARGET_DIR: ${{ github.workspace }}/target/desktop-cargo"), 2)
        self.assertIn("'tools/ci/package.Cargo.lock'", package)
        self.assertIn("cargo-cache: 'false'", package)
        self.assertIn("save-if: ${{ github.ref == 'refs/heads/master' }}", package)
        spec = importlib.util.spec_from_file_location("validation", ROOT / "tools/ci/validation-plan.py")
        validation = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(validation)
        self.assertTrue(validation.select([], tier="full")["packages"])
        for path in ("Cargo.lock", "nsh.rev", ".github/actions/setup/action.yml"):
            self.assertTrue(validation.select([path])["packages"], path)

    def test_validation_retains_successful_dependency_work_after_test_failure_only_on_master(self):
        for action in ("setup", "setup-windows"):
            text = (ROOT / f".github/actions/{action}/action.yml").read_text()
            self.assertIn("cache-on-failure: ${{ inputs.cache-on-failure }}", text)
            self.assertIn("save-if: ${{ github.ref == 'refs/heads/master' }}", text)
            self.assertIn("if: inputs.cache == 'true'", text)
        for workflow in ("ci", "windows", "coverage", "import-stress"):
            text = (ROOT / f".github/workflows/{workflow}.yml").read_text()
            self.assertIn("cache-on-failure: 'true'", text)

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


APT = ROOT / "tools/ci/apt-install.sh"
SETUP = (ROOT / ".github/actions/setup/action.yml").read_text(encoding="utf-8")
# Stand-ins for the root-only commands, so the script runs unprivileged.
FAKES = {
    "sudo": 'exec "$@"\n',
    "chown": "exit 0\n",
    "debconf-set-selections": 'cat >> "$FAKE_LOG.debconf"\n',
    # `timeout --kill-after=30 SECONDS COMMAND...` records the bound it was given.
    "timeout": 'echo "bound $2" >> "$FAKE_LOG"; shift 2; exec "$@"\n',
    "dpkg": 'if [ "$1" = --print-architecture ]; then echo amd64; else echo "dpkg $*" >> "$FAKE_LOG"; fi\n',
    "apt-get": textwrap.dedent("""\
        echo "apt-get $*" >> "$FAKE_LOG"
        count=$(grep -c '^apt-get' "$FAKE_LOG")
        for failing in ${FAKE_FAIL:-}; do
          if [ "$failing" = "$count" ]; then exit 100; fi
        done
        """),
}


@unittest.skipUnless(os.name == "posix" and shutil.which("bash"), "the Linux package install uses Bash")
class AptInstall(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        for name, body in FAKES.items():
            path = bin_dir / name
            path.write_text("#!/usr/bin/env bash\n" + body)
            path.chmod(0o755)
        self.log = self.root / "log"
        self.initramfs = self.root / "update-initramfs.conf"
        self.initramfs.write_text("MODULES=generated\nupdate_initramfs=yes\n")
        self.man_db = self.root / "man-db-auto-update"
        self.man_db.write_text("")
        sources = self.root / "apt"
        (sources / "sources.list.d").mkdir(parents=True)
        self.ubuntu = sources / "sources.list.d/ubuntu.sources"
        self.ubuntu.write_text(
            "Types: deb\nURIs: mirror+file:/etc/apt/apt-mirrors.txt\nSuites: noble noble-updates\n"
            "Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n"
            "\nTypes: deb\nURIs: http://security.ubuntu.com/ubuntu\nSuites: noble-security\n"
        )
        self.legacy = sources / "sources.list"
        self.legacy.write_text("deb http://us-east-2.ec2.archive.ubuntu.com/ubuntu/ noble main\n")
        self.other = sources / "sources.list.d/other.list"
        self.other.write_text("deb https://packages.example.test/ubuntu noble main\n")
        self.environment = {
            "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}", "FAKE_LOG": str(self.log),
            "GITHUB_OUTPUT": str(self.root / "install-output"),
            "PATINA_APT_CONF": str(self.root / "apt.conf"), "PATINA_INITRAMFS_CONF": str(self.initramfs),
            "PATINA_MAN_DB_TRIGGER": str(self.man_db), "PATINA_APT_INSTALL_SECONDS": "7",
            "PATINA_APT_REFRESH_SECONDS": "5", "PATINA_APT_DOWNLOAD_SECONDS": "11", "PATINA_APT_SOURCES": str(sources),
        }

    def install(self, restored, fail=""):
        environment = os.environ | self.environment | {"FAKE_FAIL": fail}
        result = subprocess.run(["bash", str(APT), str(self.root / "state"), restored, "generated-a", "generated-b"],
                                env=environment, capture_output=True, text=True)
        calls = self.log.read_text().splitlines() if self.log.exists() else []
        return result, [call for call in calls if not call.startswith("bound")], calls

    def test_cache_miss_refreshes_lists_once_and_bounds_every_apt_call(self):
        result, calls, raw = self.install("false")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["apt-get update -q -o APT::Update::Error-Mode=any", "apt-get install -q --download-only generated-a generated-b",
                                 "apt-get install -q --no-download generated-a generated-b"])
        self.assertEqual([call for call in raw if call.startswith("bound")], ["bound 5", "bound 11", "bound 7"])
        self.assertEqual((self.root / "install-output").read_text(), "refreshed=true\n")

    def test_cache_hit_installs_without_contacting_the_mirror(self):
        result, calls, _ = self.install("true")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["apt-get install -q --no-download generated-a generated-b"])
        self.assertEqual((self.root / "install-output").read_text(), "refreshed=false\n")

    def test_an_incomplete_cache_falls_back_to_fresh_downloads(self):
        result, calls, _ = self.install("true", fail="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("cached apt install failed", result.stdout)
        self.assertEqual(calls, ["apt-get install -q --no-download generated-a generated-b", "dpkg --configure -a",
                                 "apt-get update -q -o APT::Update::Error-Mode=any", "apt-get install -q --download-only generated-a generated-b",
                                 "apt-get install -q --no-download generated-a generated-b"])

    def test_failed_download_switches_mirrors_before_install(self):
        result, calls, _ = self.install("false", fail="2")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("trying the Ubuntu archive", result.stdout)
        self.assertEqual(calls, ["apt-get update -q -o APT::Update::Error-Mode=any", "apt-get install -q --download-only generated-a generated-b",
                                 "apt-get update -q -o APT::Update::Error-Mode=any", "apt-get install -q --download-only generated-a generated-b",
                                 "apt-get install -q --no-download generated-a generated-b"])
        changed = self.ubuntu.read_text()
        self.assertIn("URIs: https://archive.ubuntu.com/ubuntu\n", changed)
        self.assertIn("URIs: http://security.ubuntu.com/ubuntu\n", changed)
        self.assertIn("Suites: noble noble-updates\n", changed)
        self.assertIn("Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n", changed)
        self.assertEqual(self.legacy.read_text(), "deb https://archive.ubuntu.com/ubuntu/ noble main\n")
        self.assertEqual(self.other.read_text(), "deb https://packages.example.test/ubuntu noble main\n")

    def test_offline_install_failure_repairs_dpkg_without_another_download(self):
        result, calls, _ = self.install("false", fail="3")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("offline apt install failed", result.stdout)
        self.assertEqual(calls[-2:], ["dpkg --configure -a",
                                      "apt-get install -q --no-download --fix-broken generated-a generated-b"])
        self.assertEqual(sum("--download-only" in call for call in calls), 1)

    def test_the_cache_keeps_no_truncated_downloads(self):
        partial = self.root / "state/archives/partial"
        partial.mkdir(parents=True)
        (partial / "generated.deb").write_text("truncated")
        result, _, _ = self.install("false")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(partial.is_dir())
        self.assertEqual(list(partial.iterdir()), [])

    def test_a_second_failure_fails_the_step(self):
        result, calls, _ = self.install("false", fail="2 4")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len([call for call in calls if call.startswith("apt-get install")]), 2)

    def test_apt_retries_times_out_and_skips_slow_triggers(self):
        result, _, _ = self.install("false")
        self.assertEqual(result.returncode, 0, result.stderr)
        conf = Path(self.environment["PATINA_APT_CONF"]).read_text()
        state = self.root / "state"
        for line in ('Acquire::Retries "2";', 'Acquire::http::Timeout "30";', 'Acquire::https::Timeout "30";',
                     f'Dir::State::Lists "{state}/lists";', f'Dir::Cache::Archives "{state}/archives";',
                     'Binary::apt-get::APT::Keep-Downloaded-Packages "true";'):
            self.assertIn(line, conf)
        self.assertIn("update_initramfs=no", self.initramfs.read_text())
        self.assertIn("MODULES=generated", self.initramfs.read_text())
        self.assertFalse(self.man_db.exists())
        self.assertIn("man-db/auto-update boolean false", Path(f"{self.log}.debconf").read_text())

    def test_setup_caches_packages_per_runner_image_and_package_list(self):
        self.assertIn("bash tools/ci/apt-install.sh", SETUP)
        self.assertIn("ImageVersion", SETUP)
        self.assertIn('cat tools/ci/apt-install.sh', SETUP)
        self.assertIn("${{ inputs.apt-packages }}", SETUP)
        # Android native builds can reuse host packages with fresh Cargo outputs.
        self.assertEqual(SETUP.count("steps.apt.outputs.key != '' && (inputs.cache == 'true' || inputs.apt-cache == 'true')"), 2)
        self.assertIn("steps.apt-cache.outputs.cache-hit != 'true' && github.ref == 'refs/heads/master'", SETUP)
        self.assertIn("steps.packages.outputs.refreshed == 'true'", SETUP)
        self.assertNotIn("sudo apt-get", SETUP)
        self.assertIn("continue-on-error: true", SETUP.split("actions/cache/save", 1)[1].split("- ", 1)[0])

    def test_image_refresh_can_restore_the_same_package_archives(self):
        script = SETUP.split("    - name: Identify the Linux packages\n", 1)[1]
        script = textwrap.dedent(script.split("      run: |\n", 1)[1].split("    - uses:", 1)[0])
        output = self.root / "output"
        common = os.environ | self.environment | {
            "PREREQUISITES": "android-smoke", "EXTRA_PACKAGES": "", "RUNNER_TEMP": str(self.root),
            "GITHUB_OUTPUT": str(output),
        }

        def identity(image, extra=""):
            output.write_text("")
            subprocess.run(["bash", "-euo", "pipefail", "-c", script], cwd=ROOT, check=True,
                           env=common | {"ImageVersion": image, "EXTRA_PACKAGES": extra})
            return dict(line.split("=", 1) for line in output.read_text().splitlines())

        old = identity("2026.1")
        new = identity("2026.2")
        self.assertNotEqual(old["key"], new["key"])
        self.assertTrue(old["key"].startswith(new["restore-prefix"]))
        self.assertTrue(new["key"].startswith(new["restore-prefix"]))
        self.assertNotEqual(new["restore-prefix"], identity("2026.2", "generated-pkg")["restore-prefix"])
        self.assertIn("restore-keys: ${{ steps.apt.outputs.restore-prefix }}", SETUP)
        self.assertIn("APT_RESTORED: ${{ steps.apt-cache.outputs.cache-matched-key != '' }}", SETUP)

    def test_every_setup_step_has_its_own_timeout(self):
        for workflow in sorted((ROOT / ".github/workflows").glob("*.yml")):
            text = workflow.read_text(encoding="utf-8")
            for step in steps(text):
                if re.search(r"(?m)^\s*(- )?uses: ['\"]?\./\.github/actions/setup['\"]?\s*$", step):
                    with self.subTest(workflow=workflow.name):
                        self.assertRegex(step, r"(?m)^\s*(- )?timeout-minutes: \d+\s*$")

    def test_step_scan_finds_keys_in_any_order(self):
        found = steps("    steps:\n      - name: generated\n        uses: ./.github/actions/setup\n"
                      "        with:\n          cache: 'true'\n        timeout-minutes: 9\n      - run: generated\n")
        self.assertEqual(len(found), 2)
        self.assertIn("timeout-minutes: 9", found[0])
        self.assertNotIn("run: generated", found[0])


def steps(text):
    """Each workflow step's lines, whatever order its keys are written in."""
    found, current, indent = [], None, None
    for line in text.splitlines():
        match = re.match(r"( *)- ", line)
        stripped = len(line) - len(line.lstrip(" "))
        if match and (indent is None or len(match[1]) == indent):
            if current is not None:
                found.append("\n".join(current))
            current, indent = [line], len(match[1])
        elif current is not None and line.strip() and stripped <= indent:
            found.append("\n".join(current))
            current, indent = None, None
        elif current is not None:
            current.append(line)
    if current is not None:
        found.append("\n".join(current))
    return found


if __name__ == "__main__":
    unittest.main()
