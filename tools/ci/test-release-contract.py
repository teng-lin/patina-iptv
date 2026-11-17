#!/usr/bin/env python3
"""Changelog, explicit preview policy and candidate-bound provenance contracts."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock


def module(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), Path(__file__).with_name(name + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


class Contract(unittest.TestCase):
    def test_build_inventory_records_fixed_inputs_without_environment(self):
        inventory = module("build-inventory")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in ("Cargo.lock", "rust-toolchain.toml", "nsh.rev", "tools/ci/package.Cargo.lock"):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("synthetic pinned input")
            runner = Mock(return_value="synthetic version\n")
            result = inventory.inventory(root, runner, lambda name: name if name == "cargo" else None)
            self.assertEqual(len(result["files"]), 4)
            self.assertEqual(result["tools"], {"cargo": "synthetic version"})
            self.assertNotIn("environment", result)
            self.assertEqual(runner.call_count, 2)

    def test_changelog_requires_exact_version_and_real_release_notes(self):
        plan = module("release-plan")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Cargo.toml").write_text('[workspace.package]\nversion="9.8.7"\n')
            page = root / "CHANGELOG.md"
            for text in ("## [Unreleased]\n- Synthetic.\n", "## [9.8.70]\n- Synthetic.\n",
                         "## [9.8.7]\n### Fixed\n\n## [9.8.6]\n- Earlier.\n"):
                page.write_text(text)
                with self.assertRaises(ValueError):
                    plan.check_changelog(root, "refs/tags/v9.8.7")
            page.write_text("## [9.8.7] - 2026-01-01\n\n- Synthetic release.\n")
            plan.check_changelog(root, "refs/tags/v9.8.7")
            with self.assertRaisesRegex(ValueError, "tag"):
                plan.check_changelog(root, "refs/tags/v9.8.8")

    def test_explicit_signing_mode_cannot_silently_downgrade_or_accept_partial_secrets(self):
        signing = module("signing-config")
        self.assertTrue(signing.validate("signed", [True] * 6))
        self.assertFalse(signing.validate("preview", [False] * 6))
        for mode, values in (("signed", [False] * 6), ("preview", [True] * 6),
                             ("signed", [True] * 5 + [False]), ("preview", [False] * 5 + [True]),
                             ("automatic", [False] * 6)):
            with self.assertRaises(ValueError):
                signing.validate(mode, values)

    def test_every_final_file_requires_build_predicate_workflow_commit_and_ref(self):
        provenance = module("build-provenance")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Synthetic.apk").write_bytes(b"generated")
            (root / "SHA256SUMS").write_bytes(b"generated checksum")
            runner = Mock()
            provenance.verify(root, "synthetic/repo", "a" * 40, "refs/tags/v9.8.7", runner)
            self.assertEqual(runner.call_count, 2)
            for call in runner.call_args_list:
                args = call.args[0]
                self.assertEqual(args[args.index("--predicate-type") + 1], "https://slsa.dev/provenance/v1")
                self.assertIn("synthetic/repo/.github/workflows/release.yml", args)
                self.assertIn("a" * 40, args)
                self.assertIn("refs/tags/v9.8.7", args)
            runner = Mock(side_effect=subprocess.CalledProcessError(1, ["gh", "attestation", "verify"]))
            with self.assertRaises(subprocess.CalledProcessError):
                provenance.verify(root, "synthetic/repo", "a" * 40, "refs/tags/v9.8.7", runner)
            for repo, sha, ref in ((None, "a" * 40, "refs/tags/v1"), ("synthetic/repo", "", "refs/tags/v1"),
                                   ("synthetic/repo", "a" * 40, "v1")):
                with self.assertRaises(ValueError):
                    provenance.verify(root, repo, sha, ref)


if __name__ == "__main__":
    unittest.main()
