#!/usr/bin/env python3
"""Changelog, explicit preview policy and candidate-bound provenance contracts."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile


def module(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), Path(__file__).with_name(name + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


class UpdateIndex(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.output = self.work / "index.json"

    def record(self, version, suffix):
        verifier = module("verify-public-release")
        dist = self.work / f"{version}-{suffix}"
        dist.mkdir()
        (dist / f"Patina-{version}-{suffix}").write_bytes(b"synthetic package")
        record = verifier.package_record(dist, "synthetic/repo", f"v{version}", "a" * 40, self.work)
        file = dist / "record.json"
        file.write_text(json.dumps(record))
        return file

    def promote(self, *records):
        return subprocess.run([sys.executable, str(Path(__file__).with_name("update-index.py")),
                               "--repository", "synthetic/repo", self.output, *records], capture_output=True)

    def test_partial_release_and_late_retry_preserve_the_latest_package_versions(self):
        first = self.record("0.1.9", "android-tablet-tv.apk")
        newer = self.record("0.1.10", "android-tablet-tv.apk")
        linux = self.record("0.1.9", "linux-x86_64.deb")
        self.assertEqual(self.promote(first, linux).returncode, 0)
        self.assertEqual(self.promote(newer).returncode, 0)
        published = self.output.read_bytes()
        packages = json.loads(published)["packages"]
        self.assertEqual([(p["format"], p["version"]) for p in packages], [("apk", "0.1.10"), ("deb", "0.1.9")])
        self.assertEqual(self.promote(first, newer).returncode, 0)
        self.assertEqual(self.output.read_bytes(), published)

    def test_conflict_or_invalid_input_preserves_previous_index(self):
        first = self.record("0.1.9", "linux-x86_64.deb")
        self.assertEqual(self.promote(first).returncode, 0)
        previous = self.output.read_bytes()
        original = first.read_text()
        for field, bad in (("sha256", "b" * 64), ("length", -1), ("url", "https://example.invalid/file")):
            record = json.loads(original)
            record["packages"][0][field] = bad
            first.write_text(json.dumps(record))
            self.assertNotEqual(self.promote(first).returncode, 0)
            self.assertEqual(self.output.read_bytes(), previous)


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

    def test_manual_promotion_requires_both_inputs_on_master_without_rehearsal(self):
        plan = module("release-plan")
        valid = ("workflow_dispatch", "refs/heads/master", "android-desktop", "false", "v9.8.7", "17")
        plan.check_promotion(*valid)
        for index, invalid in ((0, "push"), (1, "refs/heads/topic"), (2, "all"), (3, "true"),
                               (4, ""), (4, "../v9.8.7"), (5, ""), (5, "17/attempts/1")):
            with self.subTest(index=index, invalid=invalid), self.assertRaises(ValueError):
                arguments = list(valid)
                arguments[index] = invalid
                plan.check_promotion(*arguments)

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

    def test_promotion_reads_only_successful_exact_master_builds_and_their_artifacts(self):
        self.run["run_attempt"] = 3
        jobs = {"jobs": [{"name": name, "conclusion": "success"} for name in self.candidate.PROMOTION_JOBS]}
        artifacts = [{"name": name, "id": index, "expired": False, "digest": "sha256:" + "b" * 64,
                      "workflow_run": {"id": self.run["id"], "head_sha": self.run["head_sha"]}}
                     for index, name in enumerate(sorted(self.candidate.REQUIRED), 1)]
        repository, commit = self.run["head_repository"]["full_name"], self.run["head_sha"]

        def inspect(run, jobs, artifacts):
            with patch.object(self.candidate, "api", side_effect=[
                run, jobs, {"artifacts": artifacts, "total_count": len(artifacts)},
            ]) as api:
                result = self.candidate.promotion_inputs(repository, str(self.run["id"]), commit)
                self.assertIn("/attempts/3/jobs?", api.call_args_list[1].args[0])
                return result

        attempt, checks, inventory = inspect(self.run, jobs, artifacts)
        self.assertEqual(attempt, 3)
        self.assertEqual(checks, dict.fromkeys(self.candidate.PROMOTION_JOBS, "success"))
        self.assertEqual(set(inventory), self.candidate.REQUIRED)
        split = (self.candidate.PROMOTION_JOBS - {self.candidate.LEGACY_ANDROID_JOB}) | self.candidate.SPLIT_ANDROID_JOBS
        split_jobs = {"jobs": [{"name": name, "conclusion": "success"} for name in sorted(split)]}
        self.assertEqual(inspect(self.run, split_jobs, artifacts)[1], dict.fromkeys(split, "success"))
        split_jobs['jobs'] = [job for job in split_jobs['jobs'] if job['name'] != 'candidate / android-package']
        with self.assertRaisesRegex(ValueError, 'every unsigned package build'):
            inspect(self.run, split_jobs, artifacts)
        for run, job_result, artifact_result in (
            ({**self.run, "head_sha": "c" * 40}, jobs, artifacts),
            ({**self.run, "event": "pull_request"}, jobs, artifacts),
            ({**self.run, "conclusion": "failure"}, jobs, artifacts),
            (self.run, {"jobs": jobs["jobs"][1:]}, artifacts),
            (self.run, {"jobs": [{**jobs["jobs"][0], "conclusion": "skipped"}, *jobs["jobs"][1:]]}, artifacts),
            (self.run, jobs, artifacts[1:]),
            (self.run, jobs, [*artifacts, artifacts[0]]),
            (self.run, jobs, [{**artifacts[0], "digest": None}, *artifacts[1:]]),
            (self.run, jobs, [{**artifacts[0], "expired": True}, *artifacts[1:]]),
            (self.run, jobs, [{**artifacts[0], "workflow_run": {"id": 99, "head_sha": commit}}, *artifacts[1:]]),
        ):
            with self.assertRaises(ValueError):
                inspect(run, job_result, artifact_result)

    def test_seal_names_the_missing_android_artifact(self):
        android = self.directory / 'signing-input-android'
        (android / 'synthetic.bin').unlink()
        android.rmdir()
        with self.assertRaisesRegex(ValueError, 'missing candidate artifact groups: signing-input-android'):
            self.candidate.seal(self.directory, self.environment, self.root)

    def test_promotion_artifacts_cannot_write_outside_their_group_or_overwrite_members(self):
        group = self.root / "group"
        group.mkdir()
        archive = self.root / "artifact.zip"
        with zipfile.ZipFile(archive, "w") as output:
            output.writestr("synthetic.bin", b"generated bytes")
        self.candidate.unpack_zip(archive, group)
        self.assertEqual((group / "synthetic.bin").read_bytes(), b"generated bytes")
        with self.assertRaises(FileExistsError):
            self.candidate.unpack_zip(archive, group)
        for name in ("../outside", "/outside", "directory/member"):
            with self.subTest(name=name), zipfile.ZipFile(archive, "w") as output:
                output.writestr(name, b"generated bytes")
            with self.assertRaises(ValueError):
                self.candidate.unpack_zip(archive, group)


if __name__ == "__main__":
    unittest.main()
