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

    def run_script(self, *arguments, listed="imports\nmanifests", crashing="", workers="1"):
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
            compiler = root / "toolchains/nightly-synthetic/bin"
            compiler.mkdir(parents=True)
            (compiler / "rustc").write_text('#!/bin/sh\n[ "$*" = "--print host-tuple" ] && echo x86_64-unknown-linux-gnu\n')
            (compiler / "rustc").chmod(0o755)
            (tools / "rustup").write_text('#!/bin/sh\nprintf "%s/%s/bin/cargo\\n" "$STUB_TOOLCHAINS" "$3"\n')
            (tools / "cargo-fuzz").write_text(
                '#!/bin/sh\n'
                'if [ "$1" = list ]; then printf "%s\\n" "$STUB_LISTED"; exit; fi\n'
                'echo "$RUSTUP_TOOLCHAIN $IPTV_REAL_CARGO $*" >> "$STUB_LOG"\n'
                '[ "$2" != "$STUB_CRASHING" ]\n')
            for tool in tools.iterdir():
                tool.chmod(0o755)
            log = root / "calls.log"
            environment = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ["PATH"], STUB_LOG=str(log),
                               STUB_TOOLCHAINS=str(compiler.parent.parent), STUB_LISTED=listed, STUB_CRASHING=crashing, IPTV_FUZZ_TOOLCHAIN="nightly-synthetic",
                               IPTV_FUZZ_JOBS=workers)
            result = subprocess.run(["bash", str(root / "tools/ci/fuzz-imports.sh"), *arguments],
                                    env=environment, capture_output=True, text=True)
            calls = log.read_text().splitlines() if log.exists() else []
            corpus = sorted(path.name for path in (root / "fuzz/corpus/imports").iterdir())
            return result, calls, corpus

    def test_all_listed_targets_run_with_the_pinned_nightly_and_seeds(self):
        result, calls, corpus = self.run_script("60")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(all(call.startswith("nightly-synthetic ") for call in calls))
        self.assertEqual([call.split("/nightly-synthetic/bin/cargo ")[1] for call in calls], [
            f"run {target} --target x86_64-unknown-linux-gnu -- -max_total_time=60 -max_len=65537 -rss_limit_mb=1024 -malloc_limit_mb=512"
            for target in ("imports", "manifests")
        ])
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

    def test_parallel_targets_share_one_build_and_report_every_crash(self):
        result, calls, _ = self.run_script("300", workers="3", crashing="imports")
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(calls[0].endswith("cargo build --target x86_64-unknown-linux-gnu"))
        self.assertEqual({call.split(" run ")[1].split()[0] for call in calls[1:]}, {"imports", "manifests"})
        self.assertTrue(all("-max_total_time=300" in call for call in calls[1:]))
        self.assertIn("fuzzing failed for: imports", result.stderr)

    def test_parallel_selection_cannot_run_two_writers_on_one_corpus(self):
        result, calls, _ = self.run_script("60", "imports", "imports", workers="3")
        self.assertEqual(result.returncode, 2)
        self.assertIn("parallel fuzz targets must be unique: imports", result.stderr)
        self.assertEqual(calls, [])

    def test_no_targets_and_toolchain_query(self):
        result, calls, _ = self.run_script("60", listed="")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no targets", result.stderr)
        result, calls, _ = self.run_script("--toolchain")
        self.assertEqual((result.returncode, result.stdout, calls), (0, "nightly-synthetic\n", []))


if __name__ == "__main__":
    unittest.main()
