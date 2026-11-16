#!/usr/bin/env python3
"""Every platform's check script covers every workspace crate.

tools/check.sh (Linux and macOS) is the reference. The Windows check and the
coverage job each repeat its crate lists, so a crate added to the workspace
must not be silently tested on one platform only.
"""
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = re.compile(r"-p\s+([a-z0-9-]+)")


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
        check = (ROOT / "tools/check.sh").read_text(encoding="utf-8")
        self.wasm = set(re.search(r"^for c in ([a-z0-9 -]+); do$", check, re.M)[1].split())

    def test_reference_script_tests_and_formats_every_crate(self):
        self.assertEqual(packages(lines_with("tools/check.sh", "cargo test")), self.members)
        self.assertEqual(packages(lines_with("tools/check.sh", "cargo fmt")), self.members)
        self.assertLess(self.wasm, self.libraries)

    def test_windows_check_runs_the_same_crates(self):
        tested = packages(lines_with("tools/build-windows.ps1", "cargo test"))
        self.assertEqual(tested - {"nsh-platform-windows"}, self.members)
        script = (ROOT / "tools/build-windows.ps1").read_text(encoding="utf-8")
        loop = re.search(r"foreach \(\$crate in @\(([^)]*)\)\)", script)[1]
        self.assertEqual(set(re.findall(r"'([a-z0-9-]+)'", loop)), self.wasm)
        self.assertIn("pins.py\" check-nsh", script)

    def test_coverage_measures_every_library_crate(self):
        self.assertEqual(packages(lines_with("tools/ci/coverage.sh", "crates=(")), self.libraries)

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

    def test_local_target_parser_ignores_external_links_and_sections(self):
        self.assertEqual(
            local_targets("[a](../tools/check.sh#example) [b](architecture.md) "
                          "[c](#section) [d](https://example.test/path) "
                          "[e](mailto:generated@example.test)"),
            {"../tools/check.sh", "architecture.md"},
        )


if __name__ == "__main__":
    unittest.main()
