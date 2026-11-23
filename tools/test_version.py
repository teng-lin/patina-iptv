"""Synthetic workspace regression tests: python3 -m unittest discover -s tools -p 'test_version.py'."""
import importlib.util
import json
from pathlib import Path
import subprocess
import os
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("bump_version", Path(__file__).with_name("bump-version.py"))
bump_version = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bump_version)


class VersionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "crates/iptv-app").mkdir(parents=True)
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/iptv-app"]\n[workspace.package]\nversion = "1.2.3"\n')
        (self.root / "crates/iptv-app/Cargo.toml").write_text('[package]\nname = "synthetic-app"\nversion.workspace = true\n')
        (self.root / "crates/iptv-app/nsh.json").write_text('{"name": "Synthetic"}\n')
        (self.root / "Cargo.lock").write_text('version = 4\n\n[[package]]\nname = "synthetic-app"\nversion = "1.2.3"\n\n[[package]]\nname = "dependency"\nversion = "1.2.3"\n')

        (self.root / "tools/ci").mkdir(parents=True)
        (self.root / "tools/ci/package.Cargo.lock").write_bytes((self.root / "Cargo.lock").read_bytes())

    def snapshot(self):
        return {p: p.read_bytes() for p in self.root.rglob('*') if p.is_file()}

    def test_bump_updates_only_workspace_packages(self):
        self.assertEqual(bump_version.bump(self.root, "minor"), ("1.2.3", "1.3.0"))
        lock = (self.root / "Cargo.lock").read_text()
        self.assertIn('name = "synthetic-app"\nversion = "1.3.0"', lock)
        self.assertIn('name = "dependency"\nversion = "1.2.3"', lock)
        self.assertEqual(lock, (self.root / "tools/ci/package.Cargo.lock").read_text())
        self.assertIn('"version": "1.3.0"', (self.root / "crates/iptv-app/nsh.json").read_text())

    def test_runner_locks_follow_their_own_members(self):
        workspace = self.root / "Cargo.toml"
        workspace.write_text(workspace.read_text().replace(
            '["crates/iptv-app"]', '["crates/iptv-app", "crates/iptv-tui"]'))
        terminal = self.root / "crates/iptv-tui"
        terminal.mkdir()
        (terminal / "Cargo.toml").write_text('[package]\nname = "iptv-tui"\nversion.workspace = true\n')
        lock = self.root / "Cargo.lock"
        lock.write_text(lock.read_text() + '\n[[package]]\nname = "iptv-tui"\nversion = "1.2.3"\n')

        tizen = self.root / "hosts/tizen-nsh"
        tizen.mkdir(parents=True)
        (tizen / "Cargo.toml").write_text('[workspace]\n\n[package]\nname = "patina-tizen"\n'
                                        'description = "Synthetic [runner]"\nversion = "1.2.3"\n')
        (tizen / "Cargo.lock").write_text((self.root / "tools/ci/package.Cargo.lock").read_text()
                                        + '\n[[package]]\nname = "patina-tizen"\nversion = "1.2.3"\n')

        bump_version.bump(self.root, "patch")
        self.assertIn('name = "iptv-tui"\nversion = "1.2.4"', lock.read_text())
        self.assertIn('name = "synthetic-app"\nversion = "1.2.4"',
                      (self.root / "tools/ci/package.Cargo.lock").read_text())
        self.assertIn('version = "1.2.4"', (tizen / "Cargo.toml").read_text())
        self.assertIn('name = "patina-tizen"\nversion = "1.2.4"', (tizen / "Cargo.lock").read_text())
        self.assertIn('name = "synthetic-app"\nversion = "1.2.4"', (tizen / "Cargo.lock").read_text())

    def test_dry_run_and_increment_choices(self):
        before = self.snapshot()
        for requested, expected in [("patch", "1.2.4"), ("major", "2.0.0"), ("2.4.6", "2.4.6")]:
            self.assertEqual(bump_version.bump(self.root, requested, True)[1], expected)
        self.assertEqual(before, self.snapshot())

    def test_invalid_input_never_writes(self):
        before = self.snapshot()
        for value in ["1.2.3", "0.9.0", "01.2.4", "v2.0.0", "2.0.0-beta", "bad"]:
            with self.assertRaises(ValueError):
                bump_version.bump(self.root, value)
            self.assertEqual(before, self.snapshot())

    def test_stale_package_lockfile_never_writes(self):
        path = self.root / "tools/ci/package.Cargo.lock"
        path.write_text(path.read_text().replace('version = "1.2.3"', 'version = "1.0.0"'))
        before = self.snapshot()
        with self.assertRaises(ValueError):
            bump_version.bump(self.root, "patch")
        self.assertEqual(before, self.snapshot())

    def test_fuzz_lock_follows_the_crates_it_names(self):
        (self.root / "fuzz").mkdir()
        fuzz = self.root / "fuzz/Cargo.lock"
        fuzz.write_text('version = 4\n\n[[package]]\nname = "synthetic-app"\nversion = "1.2.3"\n\n'
                        '[[package]]\nname = "synthetic-fuzz"\nversion = "0.0.0"\n')
        bump_version.bump(self.root, "patch")
        self.assertIn('name = "synthetic-app"\nversion = "1.2.4"', fuzz.read_text())
        self.assertIn('name = "synthetic-fuzz"\nversion = "0.0.0"', fuzz.read_text())
        # A fuzz lock left behind by an earlier release is refused, not skipped.
        fuzz.write_text(fuzz.read_text().replace('version = "1.2.4"', 'version = "1.0.0"'))
        before = self.snapshot()
        with self.assertRaisesRegex(ValueError, "fuzz/Cargo.lock version is out of sync"):
            bump_version.bump(self.root, "patch")
        self.assertEqual(before, self.snapshot())

    def test_stale_lockfile_never_writes(self):
        path = self.root / "Cargo.lock"
        path.write_text(path.read_text().replace('version = "1.2.3"', 'version = "1.0.0"'))
        before = self.snapshot()
        with self.assertRaises(ValueError):
            bump_version.bump(self.root, "patch")
        self.assertEqual(before, self.snapshot())


class BuildHashTests(unittest.TestCase):
    def test_worktree_detached_and_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / ("build-script.exe" if os.name == "nt" else "build-script")
            source = Path(__file__).resolve().parents[1] / "crates/iptv-app/build_version.rs"
            # Compile the exact production version module without the Cargo
            # build script's platform-specific linking/resource dependencies.
            harness = root / "version_harness.rs"
            harness.write_text(f'#[path = {json.dumps(source.as_posix(), ensure_ascii=False)}]\nmod build_version;\n'
                               'fn main() { build_version::emit(); }\n', encoding="utf-8")
            subprocess.run(["rustc", "--edition=2024", str(harness), "-o", str(binary)], check=True)
            repo = root / "repo"
            repo.mkdir()

            def git(*args):
                return subprocess.check_output(["git", "-C", str(repo), *args], stderr=subprocess.DEVNULL, text=True).strip()

            def output(path):
                return subprocess.check_output([str(binary)], env={**os.environ,
                    "CARGO_MANIFEST_DIR": str(path), "CARGO_CFG_TARGET_OS": "linux"}, text=True)

            git("init")
            git("-c", "user.name=Synthetic", "-c", "user.email=synthetic@example.invalid",
                "commit", "--allow-empty", "-m", "Synthetic fixture")
            first = git("rev-parse", "HEAD")[:8]
            worktree = root / "worktree"
            git("worktree", "add", "-b", "synthetic-branch", str(worktree))
            linked = output(worktree)
            self.assertIn(f"cargo:rustc-env=IPTV_GIT_HASH={first}", linked)
            self.assertIn("worktrees/worktree/HEAD", linked)
            self.assertIn("refs/heads/synthetic-branch", linked)
            for line in linked.splitlines():
                if line.startswith("cargo:rerun-if-changed="):
                    watched = line.split("=", 1)[1]
                    if watched != "build.rs":
                        self.assertTrue(Path(watched).exists(), watched)
            git("pack-refs", "--all", "--prune")
            packed = output(worktree)
            self.assertIn(f"IPTV_GIT_HASH={first}", packed)
            for line in packed.splitlines():
                if line.startswith("cargo:rerun-if-changed="):
                    watched = line.split("=", 1)[1]
                    if watched != "build.rs":
                        self.assertTrue(Path(watched).exists(), watched)
            git("-c", "user.name=Synthetic", "-c", "user.email=synthetic@example.invalid",
                "commit", "--allow-empty", "-m", "Next synthetic fixture")
            second = git("rev-parse", "HEAD")[:8]
            self.assertNotEqual(first, second)
            self.assertIn(f"IPTV_GIT_HASH={second}", output(repo))
            git("checkout", "--detach", "HEAD")
            self.assertIn(f"IPTV_GIT_HASH={second}", output(repo))
            archive = root / "archive"
            archive.mkdir()
            self.assertIn("IPTV_GIT_HASH=unknown", output(archive))
