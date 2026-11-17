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

    def test_android_distribution_upgrade_numbers_are_stable_and_bounded(self):
        version = module('version')
        self.assertEqual(version.android_version_code('0.1.5'), 1_001_005)
        values = ['0.1.5', '0.1.6', '0.2.0', '0.999.999', '1.0.0']
        codes = [version.android_version_code(value) for value in values]
        self.assertEqual(codes, sorted(set(codes)))
        for value in ('0.1.5-beta', '01.2.3', '1.1000.0', '0.1.1000', '2100.0.0'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                version.android_version_code(value)

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


class Candidate(unittest.TestCase):
    def setUp(self):
        import json
        self.json = json
        self.candidate = module("release-candidate")
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.directory = self.root / "artifacts"
        self.directory.mkdir()
        for name in self.candidate.INPUTS:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("synthetic pinned input")
        (self.root / 'Cargo.toml').write_text('[workspace.package]\nversion="9.8.7"\n')
        (self.root / 'fuzz').mkdir()
        (self.root / 'fuzz/Cargo.toml').write_text('[[bin]]\nname="synthetic"\n')
        for name in self.candidate.REQUIRED:
            path = self.directory / name / "synthetic.bin"
            path.parent.mkdir()
            path.write_bytes(b"synthetic unsigned package")
        self.environment = {'GITHUB_EVENT_NAME': 'push', 'GITHUB_REF': 'refs/heads/master',
                            'GITHUB_REPOSITORY': 'synthetic/repo', 'GITHUB_SHA': 'a' * 40,
                            'GITHUB_RUN_ID': '17', 'GITHUB_RUN_ATTEMPT': '2',
                            'PLAN': json.dumps(self.candidate.validation.select([], 'release', self.root, candidate_build=True)),
                            'NEEDS': json.dumps({job: {'result': 'success'} for job in ('plan', 'validation', 'candidate')})}
        self.run = {'event': 'push', 'head_branch': 'master', 'head_sha': 'a' * 40,
                    'status': 'completed', 'conclusion': 'success', 'path': '.github/workflows/validation.yml',
                    'head_repository': {'full_name': 'synthetic/repo'}, 'id': 17, 'run_attempt': 2}
        self.candidate.seal(self.directory, self.environment, self.root)

    def verify(self):
        return self.candidate.verify(self.directory, self.run, 'synthetic/repo', 'a' * 40, self.root)

    def test_only_complete_exact_candidate_is_promoted(self):
        result = self.verify()
        self.assertEqual(len(result['files']), len(self.candidate.REQUIRED))
        self.assertEqual(result['build']['version'], '9.8.7')
        for key, value in (('event', 'pull_request'), ('head_branch', 'synthetic'), ('head_sha', 'b' * 40),
                           ('conclusion', 'failure'), ('status', 'in_progress'), ('path', '.github/workflows/ci.yml'),
                           ('head_repository', {'full_name': 'synthetic/fork'}), ('id', 18), ('run_attempt', 3)):
            with self.subTest(key=key):
                old = self.run[key]
                self.run[key] = value
                with self.assertRaises(ValueError):
                    self.verify()
                self.run[key] = old

    def test_changed_inputs_artifact_bytes_and_extra_files_fail_closed(self):
        for name in self.candidate.INPUTS:
            path = self.root / name
            content = path.read_bytes()
            path.write_bytes(b'changed pin')
            with self.assertRaisesRegex(ValueError, 'identity'):
                self.verify()
            path.write_bytes(content)
        path = next(self.directory.rglob('synthetic.bin'))
        content = path.read_bytes()
        path.write_bytes(b'changed package')
        with self.assertRaisesRegex(ValueError, 'bytes'):
            self.verify()
        path.write_bytes(content)
        extra = path.with_name('extra.bin')
        extra.write_bytes(b'not in manifest')
        with self.assertRaisesRegex(ValueError, 'bytes'):
            self.verify()
        extra.unlink()
        path.unlink()
        with self.assertRaisesRegex(ValueError, 'empty'):
            self.verify()

    def test_manifest_check_and_artifact_membership_cannot_be_weakened(self):
        manifest_path = self.directory / 'manifest.json'
        manifest = self.json.loads(manifest_path.read_text())
        for field, value in (('plan', {}), ('checks', {'validation': 'success'}),
                             ('build', {**manifest['build'], 'credentials': True}),
                             ('sha', 'b' * 40), ('run_id', 18), ('attempt', 1)):
            with self.subTest(field=field):
                manifest_path.write_text(self.json.dumps({**manifest, field: value}))
                with self.assertRaisesRegex(ValueError, 'identity'):
                    self.verify()
        manifest_path.write_text(self.json.dumps(manifest))
        extra = self.directory / 'packages-unexpected'
        extra.mkdir()
        (extra / 'synthetic.bin').write_bytes(b'extra group')
        with self.assertRaisesRegex(ValueError, 'groups'):
            self.verify()

    def test_pr_and_partial_or_failed_checks_cannot_seal(self):
        for key, value in (('GITHUB_EVENT_NAME', 'pull_request'), ('GITHUB_REF', 'refs/heads/synthetic'),
                           ('PLAN', '{}'), ('NEEDS', '{}'),
                           ('NEEDS', self.json.dumps({'plan': {'result': 'success'}, 'validation': {'result': 'success'},
                                                     'candidate': {'result': 'skipped'}}))):
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.candidate.seal(self.directory, {**self.environment, key: value}, self.root)

    def test_absent_expired_and_untrusted_runs_fall_back_but_bad_manifest_fails(self):
        from unittest.mock import patch
        environment = {'CANDIDATE_REUSE': 'true', 'GH_REPO': 'synthetic/repo', 'GITHUB_SHA': 'a' * 40}
        for runs, artifacts in (([], []), ([{**self.run, 'event': 'pull_request'}], []),
                                ([self.run], [{'name': 'release-candidate', 'expired': True}])):
            with patch.object(self.candidate, 'api', side_effect=lambda path: {'workflow_runs': runs} if '/workflows/' in path else {'artifacts': artifacts}), \
                    patch.object(self.candidate.subprocess, 'run') as runner:
                self.assertEqual(self.candidate.select(self.directory, environment), '')
                runner.assert_not_called()
        with patch.object(self.candidate, 'api', side_effect=[{'workflow_runs': [self.run]},
                {'artifacts': [{'name': 'release-candidate', 'expired': False}]}]), \
                patch.object(self.candidate.subprocess, 'run'), patch.object(self.candidate, 'verify', side_effect=ValueError('bad manifest')):
            with self.assertRaisesRegex(ValueError, 'bad manifest'):
                self.candidate.select(self.directory, environment)


if __name__ == "__main__":
    unittest.main()
