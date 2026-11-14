#!/usr/bin/env python3
"""The NSH and Rust pins have one source each; nothing else repeats them."""
import importlib.util
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("pins", Path(__file__).with_name("pins.py"))
pins = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pins)

REVISION = "0123456789abcdef0123456789abcdef01234567"


def git(directory, *arguments):
    environment = {
        "PATH": os.environ["PATH"], "HOME": str(directory), "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_AUTHOR_NAME": "Synthetic", "GIT_AUTHOR_EMAIL": "synthetic@example.test",
        "GIT_COMMITTER_NAME": "Synthetic", "GIT_COMMITTER_EMAIL": "synthetic@example.test",
    }
    return subprocess.run(["git", "-C", str(directory), *arguments], env=environment,
                          check=True, capture_output=True, text=True).stdout.strip()


class Revision(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def test_one_full_commit_is_accepted(self):
        (self.root / "nsh.rev").write_text(REVISION + "\n")
        self.assertEqual(pins.nsh_revision(self.root), REVISION)

    def test_branches_short_hashes_and_extra_content_are_refused(self):
        for text in ("tv-keys\n", REVISION[:12] + "\n", REVISION.upper() + "\n", REVISION,
                     REVISION + "\n" + REVISION + "\n", "# pin\n" + REVISION + "\n", ""):
            with self.subTest(text=text):
                (self.root / "nsh.rev").write_text(text)
                with self.assertRaises(ValueError):
                    pins.nsh_revision(self.root)

    def test_toolchain_must_be_an_exact_release(self):
        (self.root / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.2.3"\n')
        self.assertEqual(pins.toolchain(self.root), "1.2.3")
        for channel in ("stable", "nightly-2026-01-01", "1.2"):
            (self.root / "rust-toolchain.toml").write_text(f'[toolchain]\nchannel = "{channel}"\n')
            with self.assertRaises(ValueError):
                pins.toolchain(self.root)


class Checkout(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name) / "app"
        self.checkout = Path(temporary.name) / "nsh"
        self.root.mkdir()
        self.checkout.mkdir()
        git(self.checkout, "init", "--quiet")
        git(self.checkout, "commit", "--quiet", "--allow-empty", "-m", "synthetic")
        self.head = git(self.checkout, "rev-parse", "HEAD")

    def test_checkout_at_the_pin_passes(self):
        (self.root / "nsh.rev").write_text(self.head + "\n")
        self.assertIsNone(pins.check_nsh(self.checkout, self.root, {}))

    def test_failed_status_is_not_a_clean_checkout(self):
        (self.root / "nsh.rev").write_text(self.head + "\n")
        results = [
            subprocess.CompletedProcess([], 0, self.head + "\n", ""),
            subprocess.CompletedProcess([], 128, "", "synthetic status failure"),
        ]
        with patch.object(pins.subprocess, "run", side_effect=results):
            problem = pins.check_nsh(self.checkout, self.root, {})
        self.assertIn("git status failed", problem)

    def test_dirty_checkout_fails(self):
        (self.root / "nsh.rev").write_text(self.head + "\n")
        (self.checkout / "dirty.txt").write_text("uncommitted")
        problem = pins.check_nsh(self.checkout, self.root, {})
        self.assertIsNotNone(problem)
        self.assertIn("uncommitted changes", problem)

    def test_dirty_checkout_allowed_with_override(self):
        (self.root / "nsh.rev").write_text(self.head + "\n")
        (self.checkout / "dirty.txt").write_text("uncommitted")
        self.assertIsNone(pins.check_nsh(self.checkout, self.root, {"IPTV_ALLOW_NSH_DRIFT": "1"}))

    def test_other_commit_fails_and_names_both(self):
        (self.root / "nsh.rev").write_text(REVISION + "\n")
        problem = pins.check_nsh(self.checkout, self.root, {})
        self.assertIn(self.head, problem)
        self.assertIn(REVISION, problem)
        self.assertIn("IPTV_ALLOW_NSH_DRIFT", problem)

    def test_missing_checkout_fails(self):
        (self.root / "nsh.rev").write_text(REVISION + "\n")
        problem = pins.check_nsh(self.root / "absent", self.root, {})
        self.assertIn("not a Git checkout", problem)

    def test_override_allows_fork_development_only_when_exactly_one(self):
        (self.root / "nsh.rev").write_text(REVISION + "\n")
        self.assertIsNone(pins.check_nsh(self.checkout, self.root, {"IPTV_ALLOW_NSH_DRIFT": "1"}))
        for value in ("", "0", "true"):
            self.assertIsNotNone(pins.check_nsh(self.checkout, self.root, {"IPTV_ALLOW_NSH_DRIFT": value}))

    def test_command_line_reports_failure(self):
        script = Path(__file__).with_name("pins.py")
        environment = {key: value for key, value in os.environ.items() if key != "IPTV_ALLOW_NSH_DRIFT"}
        result = subprocess.run([sys.executable, str(script), "check-nsh", str(self.checkout)],
                                env=environment, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("nsh.rev pins", result.stderr)


class Repository(unittest.TestCase):
    """The tracked files agree, and the workflows read the pins instead of copying them."""

    def test_outputs_are_the_tracked_pins(self):
        result = subprocess.run([sys.executable, str(Path(__file__).with_name("pins.py"))],
                                check=True, capture_output=True, text=True)
        self.assertEqual(result.stdout.splitlines(),
                         [f"nsh={pins.nsh_revision()}", f"toolchain={pins.toolchain()}"])

    def test_declared_minimum_rust_is_the_tested_toolchain(self):
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
        self.assertEqual(manifest["workspace"]["package"]["rust-version"], pins.toolchain())

    def test_actions_and_workflows_do_not_repeat_the_pins(self):
        revision, channel = pins.nsh_revision(), pins.toolchain()
        files = [*ROOT.glob(".github/actions/*/action.yml"), *ROOT.glob(".github/workflows/*.yml"),
                 ROOT / "README.md"]
        self.assertGreater(len(files), 6)
        for path in files:
            text = path.read_text()
            self.assertNotIn(revision, text, path)
            self.assertIsNone(re.search(r"toolchain:\s*['\"]?" + re.escape(channel), text), path)
        for action in ("setup", "setup-windows"):
            text = (ROOT / f".github/actions/{action}/action.yml").read_text()
            self.assertIn("ref: ${{ steps.pins.outputs.nsh }}", text)
            self.assertIn("toolchain: ${{ steps.pins.outputs.toolchain }}", text)


if __name__ == "__main__":
    unittest.main()
