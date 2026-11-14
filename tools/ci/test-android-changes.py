#!/usr/bin/env python3
"""Rust-only changes must still be compiled for Android on pull requests."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
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
