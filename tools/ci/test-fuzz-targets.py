#!/usr/bin/env python3
"""Every fuzz target is run: the matrix comes from the manifest, not a fixed list."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = Path(__file__).with_name("fuzz-targets.py")
spec = importlib.util.spec_from_file_location("fuzz_targets", SCRIPT)
fuzz_targets = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fuzz_targets)

MANIFEST = '''[package]
name = "synthetic-fuzz"

[[bin]]
name = "manifests"
path = "fuzz_targets/manifests.rs"

[[bin]]
name = "imports"
path = "fuzz_targets/imports.rs"
'''


class Targets(unittest.TestCase):
    def test_every_binary_is_a_target(self):
        self.assertEqual(fuzz_targets.targets(MANIFEST), ["imports", "manifests"])

    def test_empty_duplicate_and_unsafe_names_are_refused(self):
        for manifest in ('[package]\nname = "synthetic-fuzz"\n', MANIFEST.replace("manifests", "imports"),
                         MANIFEST.replace('"manifests"', '"bad name; echo"')):
            with self.subTest(manifest=manifest):
                with self.assertRaises(ValueError):
                    fuzz_targets.targets(manifest)

    def test_repository_manifest_lists_the_import_target(self):
        result = subprocess.run([sys.executable, str(SCRIPT)], check=True, capture_output=True, text=True)
        self.assertIn("m3u", json.loads(result.stdout))


class Runner(unittest.TestCase):
    """fuzz-imports.sh with stub tools: which targets it runs, and how it fails."""

    def run_script(self, *arguments, listed="imports\nmanifests", crashing=""):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "tools/ci").mkdir(parents=True)
            shutil.copyfile(ROOT / "tools/ci/fuzz-imports.sh", root / "tools/ci/fuzz-imports.sh")
            (root / "tools/ci/locked-cargo.py").write_text("")
            (root / "fuzz/seeds/imports").mkdir(parents=True)
            (root / "fuzz/seeds/imports/seed-synthetic").write_text("synthetic")
            (root / "fuzz/corpus/imports").mkdir(parents=True)
            (root / "fuzz/corpus/imports/kept-from-an-earlier-run").write_text("synthetic")
            tools = root / "bin"
            tools.mkdir()
            (tools / "rustup").write_text('#!/bin/sh\necho "/synthetic/$3/bin/cargo"\n')
            (tools / "cargo-fuzz").write_text(
                '#!/bin/sh\n'
                'if [ "$1" = list ]; then printf "%s\\n" "$STUB_LISTED"; exit; fi\n'
                'echo "$RUSTUP_TOOLCHAIN $IPTV_REAL_CARGO $*" >> "$STUB_LOG"\n'
                '[ "$2" != "$STUB_CRASHING" ]\n')
            for tool in tools.iterdir():
                tool.chmod(0o755)
            log = root / "calls.log"
            environment = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ["PATH"], STUB_LOG=str(log),
                               STUB_LISTED=listed, STUB_CRASHING=crashing, IPTV_FUZZ_TOOLCHAIN="nightly-synthetic")
            result = subprocess.run(["bash", str(root / "tools/ci/fuzz-imports.sh"), *arguments],
                                    env=environment, capture_output=True, text=True)
            calls = log.read_text().splitlines() if log.exists() else []
            corpus = sorted(path.name for path in (root / "fuzz/corpus/imports").iterdir())
            return result, calls, corpus

    def test_all_listed_targets_run_with_the_pinned_nightly_and_seeds(self):
        result, calls, corpus = self.run_script("60")
        self.assertEqual(result.returncode, 0, result.stderr)
        prefix = "nightly-synthetic /synthetic/nightly-synthetic/bin/cargo run "
        self.assertEqual(calls, [prefix + f"{target} -- -max_total_time=60 -max_len=65537 -rss_limit_mb=512"
                                 for target in ("imports", "manifests")])
        self.assertEqual(corpus, ["kept-from-an-earlier-run", "seed-synthetic"])

    def test_named_target_runs_alone_and_unknown_one_is_refused(self):
        result, calls, _ = self.run_script("60", "manifests")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 1)
        self.assertIn(" run manifests ", calls[0])
        result, calls, _ = self.run_script("60", "absent")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unknown fuzz target: absent", result.stderr)
        self.assertEqual(calls, [])

    def test_crash_fails_after_the_other_targets_still_ran(self):
        result, calls, _ = self.run_script("60", crashing="imports")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("fuzzing failed for: imports", result.stderr)
        self.assertEqual(len(calls), 2)

    def test_no_targets_and_toolchain_query(self):
        result, calls, _ = self.run_script("60", listed="")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no targets", result.stderr)
        result, calls, _ = self.run_script("--toolchain")
        self.assertEqual((result.returncode, result.stdout, calls), (0, "nightly-synthetic\n", []))


if __name__ == "__main__":
    unittest.main()
