#!/usr/bin/env python3
"""Rust-only changes must still be compiled for Android on pull requests."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest

SCRIPT = Path(__file__).with_name("android-changes.py")
spec = importlib.util.spec_from_file_location("android_changes", SCRIPT)
changes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(changes)


class Selection(unittest.TestCase):
    def pull_request(self, *paths):
        return changes.select("pull_request", list(paths))

    def test_android_only_rust_source_selects_the_compile_job(self):
        for path in ("crates/iptv-app/src/android_transport.rs", "crates/iptv-core/src/guide/moves.rs",
                     "crates/iptv-store-sqlite/tests/it/main.rs"):
            with self.subTest(path=path):
                self.assertEqual(self.pull_request(path), {"package": False, "compile": True})

    def test_documentation_and_other_hosts_select_nothing(self):
        self.assertEqual(self.pull_request("README.md", "docs/release.md", "tools/mock-server.py",
                                           ".github/workflows/windows.yml", "crates/iptv-app/i18n/synthetic.json"),
                         {"package": False, "compile": False})

    def test_packaging_inputs_select_the_full_job_only(self):
        for path in ("hosts/android/app/build.gradle.kts", "Cargo.lock", "crates/iptv-net/Cargo.toml",
                     "crates/iptv-app/build.rs", "tools/ci/check-android.sh", "nsh.rev", "rust-toolchain.toml",
                     ".github/actions/setup/action.yml", "crates/iptv-player-mpv/fixtures/player-props.json",
                     "crates/iptv-player-mpv/fixtures/player-events.json"):
            with self.subTest(path=path):
                self.assertEqual(self.pull_request(path, "crates/iptv-core/src/menu.rs"),
                                 {"package": True, "compile": False})

    def test_pushes_and_manual_runs_do_both(self):
        for event in ("push", "workflow_dispatch"):
            self.assertEqual(changes.select(event, []), {"package": True, "compile": True})

    def test_every_manifest_and_lock_change_reaches_an_android_build(self):
        for path in ("Cargo.toml", "Cargo.lock", "crates/iptv-epg/Cargo.toml"):
            selected = self.pull_request(path)
            self.assertTrue(selected["package"] or selected["compile"], path)


class NdkVersion(unittest.TestCase):
    def test_compile_check_uses_the_ndk_the_package_job_installs(self):
        root = Path(__file__).resolve().parents[2]
        version = subprocess.run(["bash", "-c", 'source "$1"; printf %s "$IPTV_ANDROID_NDK"', "ndk",
                                  str(root / "tools/ci/android-ndk-env.sh")],
                                 check=True, capture_output=True, text=True).stdout
        self.assertRegex(version, r"^[0-9]+\.[0-9]+\.[0-9]+$")
        recipe = (root / ".github/actions/android/action.yml").read_text()
        self.assertIn(f'"ndk;{version}"', recipe)
        self.assertIn(f"ANDROID_NDK_HOME=$ANDROID_HOME/ndk/{version}", recipe)


class Prerequisites(unittest.TestCase):
    """Execute the composite action's shell with SDK/Cargo stand-ins."""

    def test_arm_targets_runtime_arguments_and_cache_key(self):
        self.run_action("arm64-v8a,armeabi-v7a", ["aarch64-linux-android", "armv7-linux-androideabi"])

    def test_emulator_override(self):
        self.run_action("x86_64", ["x86_64-linux-android"])

    def run_action(self, abis, targets):
        root = Path(__file__).resolve().parents[2]
        recipe = (root / ".github/actions/android/action.yml").read_text()
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            checkout = temporary / "checkout"
            script_directory = checkout / "tools/ci"
            script_directory.mkdir(parents=True)
            (script_directory / "nsh-tool-key.py").write_text('print("a" * 64)\n')
            (temporary / "nsh").mkdir()
            binaries = temporary / "bin"
            binaries.mkdir()
            recorder = ('#!' + sys.executable + '\nimport json,os,sys\nfrom pathlib import Path\n'
                        'with open(os.environ["COMMAND_LOG"], "a") as out:\n'
                        '    out.write(json.dumps([Path(sys.argv[0]).name, *sys.argv[1:]]) + "\\n")\n')
            for command in ("sdkmanager", "rustup", "cargo"):
                stub = binaries / command
                stub.write_text(recorder)
                stub.chmod(0o755)
            record = temporary / "commands"
            output = temporary / "outputs"
            environment = dict(os.environ, PATH=str(binaries) + os.pathsep + os.environ["PATH"],
                               ABI=abis, COMMAND_LOG=str(record), GITHUB_OUTPUT=str(output),
                               GITHUB_ENV=str(temporary / "environment"), ANDROID_HOME=str(temporary / "sdk"))
            for name in ("Install Android build tools", "Identify Android runtime inputs", "Build pinned Android runtime"):
                step = recipe.split("    - name: " + name + "\n", 1)[1].split("    - ", 1)[0]
                script = textwrap.dedent(step.split("      run: |\n", 1)[1])
                subprocess.run(["bash", "-e", "-o", "pipefail", "-c", script], env=environment,
                               cwd=checkout, check=True, capture_output=True, text=True)
            commands = [json.loads(line) for line in record.read_text().splitlines()]
            self.assertEqual([command[1:] for command in commands if command[0] == "rustup"],
                             [["target", "add", target] for target in targets])
            expected = ["cargo", "run", "--locked", "-p", "xtask", "--", "android", "--release"]
            for abi in abis.split(","):
                expected.extend(["--abi", abi])
            self.assertEqual(commands[-1], expected)
            outputs = dict(line.split("=", 1) for line in output.read_text().splitlines())
            self.assertEqual(outputs["abis"], abis.replace(",", "-"))
            self.assertNotIn(",", outputs["abis"])
            # Restore and save must both use the normalized identity.
            cache_keys = [line.strip() for line in recipe.splitlines() if line.strip().startswith("key: nsh-android-")]
            self.assertEqual(len(cache_keys), 2)
            self.assertEqual(cache_keys[0], cache_keys[1])
            self.assertIn("${{ steps.identity.outputs.abis }}", cache_keys[0])


class CommandLine(unittest.TestCase):
    """The workflow step itself, against a synthetic repository."""

    def run_script(self, files):
        with tempfile.TemporaryDirectory() as directory:
            environment = {
                "PATH": os.environ["PATH"], "HOME": directory, "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_AUTHOR_NAME": "Synthetic", "GIT_AUTHOR_EMAIL": "synthetic@example.test",
                "GIT_COMMITTER_NAME": "Synthetic", "GIT_COMMITTER_EMAIL": "synthetic@example.test",
            }

            def git(*arguments):
                return subprocess.run(["git", "-C", directory, *arguments], env=environment,
                                      check=True, capture_output=True, text=True).stdout.strip()

            git("init", "--quiet")
            git("commit", "--quiet", "--allow-empty", "-m", "base")
            base = git("rev-parse", "HEAD")
            for name in files:
                path = Path(directory) / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("synthetic\n")
            git("add", "--all")
            git("commit", "--quiet", "-m", "change")
            environment.update(EVENT="pull_request", BASE_SHA=base, HEAD_SHA=git("rev-parse", "HEAD"))
            return subprocess.run([sys.executable, str(SCRIPT)], cwd=directory, env=environment,
                                  check=True, capture_output=True, text=True).stdout.splitlines()

    def test_outputs_for_a_rust_only_pull_request(self):
        self.assertEqual(self.run_script(["crates/iptv-app/src/android_transport.rs"]),
                         ["package=false", "compile=true"])

    def test_outputs_for_an_android_host_pull_request(self):
        self.assertEqual(self.run_script(["hosts/android/app/src/main/Synthetic.kt"]),
                         ["package=true", "compile=false"])

    def test_missing_revisions_fail(self):
        result = subprocess.run([sys.executable, str(SCRIPT)], env={"PATH": os.environ["PATH"], "EVENT": "pull_request"},
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
