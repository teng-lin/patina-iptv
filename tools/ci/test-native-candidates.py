#!/usr/bin/env python3
"""Native candidate integrity tests use generated archives and no network."""

import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock
import urllib.error
import zipfile

import native_candidates as native


def sha(data):
    return hashlib.sha256(data).hexdigest()


class Candidate(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.cache = self.root / "cache"
        self.runtime, self.sources = b"generated native runtime", b"generated corresponding source"
        self.tag = "mpv-runtime-" + "a" * 64
        self.repository = "generated-owner/generated-repository"
        base = f"https://github.com/{self.repository}/releases/download/{self.tag}/"
        self.pin = {
            "schema": 1,
            "tag": self.tag,
            "url": base + native.SPECS["windows"][1],
            "sha256": sha(self.runtime),
            "source_url": base + native.SPECS["windows"][2],
            "source_sha256": sha(self.sources),
        }
        path = self.root / native.SPECS["windows"][0]
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps(self.pin))
        self.registry_path = self.root / "tools/ci/native-candidates.json"
        self.registry_path.parent.mkdir(parents=True)
        self.archive = self.cache / "artifacts/7.zip"
        self.archive.parent.mkdir(parents=True)
        self.write_archive()

    def write_archive(self, runtime=None, manifest=None):
        with zipfile.ZipFile(self.archive, "w") as archive:
            archive.writestr(native.SPECS["windows"][1], self.runtime if runtime is None else runtime)
            archive.writestr(native.SPECS["windows"][2], self.sources)
            archive.writestr("runtime.json", json.dumps(self.pin if manifest is None else manifest))
        self.registry_path.write_text(
            json.dumps({"schema": 1, "repository": self.repository, "platforms": {"windows": {"tag": self.tag, "artifact_id": 7, "artifact_sha256": native.digest(self.archive)}}})
        )

    def missing(self):
        return mock.patch.object(native.urllib.request, "urlopen", side_effect=urllib.error.HTTPError(self.pin["url"], 404, "generated missing release", None, None))

    def test_unpublished_pin_uses_only_the_exact_candidate_bytes(self):
        with self.missing(), mock.patch.object(native.subprocess, "run", side_effect=AssertionError("verified cache avoids GitHub")):
            path = native.fetch("windows", "runtime", self.cache, self.root)
            self.assertEqual(path.read_bytes(), self.runtime)
            self.assertEqual(native.fetch("windows", "sources", self.cache, self.root).read_bytes(), self.sources)

    def test_corrupt_candidate_payload_is_refused_even_with_a_valid_outer_checksum(self):
        self.write_archive(runtime=b"wrong generated archive")
        with self.missing(), self.assertRaisesRegex(ValueError, "asset checksum mismatch"):
            native.fetch("windows", "runtime", self.cache, self.root)
        self.assertFalse((self.cache / self.tag / native.SPECS["windows"][1]).exists())

    def test_candidate_manifest_must_equal_the_committed_pin(self):
        self.write_archive(manifest={**self.pin, "sha256": "b" * 64})
        with self.missing(), self.assertRaisesRegex(ValueError, "manifest differs"):
            native.fetch("windows", "runtime", self.cache, self.root)

    def test_external_release_url_is_rejected_before_network_access(self):
        path = self.root / native.SPECS["windows"][0]
        path.write_text(json.dumps({**self.pin, "url": "https://127.0.0.1:9/generated.zip"}))
        with mock.patch.object(native.urllib.request, "urlopen") as network:
            with self.assertRaisesRegex(ValueError, "outside its pinned release"):
                native.fetch("windows", "runtime", self.cache, self.root)
            network.assert_not_called()

    def test_a_corrupt_published_asset_does_not_fall_back_to_a_candidate(self):
        response = io.BytesIO(b"corrupt generated published payload")
        response.geturl = lambda: self.pin["url"]
        with mock.patch.object(native.urllib.request, "urlopen", return_value=response), mock.patch.object(native, "candidate_file") as fallback:
            with self.assertRaisesRegex(ValueError, "Published native asset checksum mismatch"):
                native.fetch("windows", "runtime", self.cache, self.root)
            fallback.assert_not_called()


class Provenance(unittest.TestCase):
    def setUp(self):
        self.commit = "a" * 40
        self.entry = {"run_id": 4, "source_commit": self.commit, "artifact_id": 7, "artifact_sha256": "b" * 64}
        self.run = {"conclusion": "success", "event": "workflow_dispatch", "head_sha": self.commit,
                    "head_repository": {"full_name": "generated/repository"}, "path": ".github/workflows/mpv-runtime.yml"}
        self.jobs = {"jobs": [{"name": name, "conclusion": "success"} for name in ("build", "smoke")]}
        self.artifact = {"expired": False, "digest": "sha256:" + self.entry["artifact_sha256"],
                         "workflow_run": {"id": 4, "head_sha": self.commit}}

    def verify(self):
        with mock.patch.object(native, "gh_json", side_effect=[self.run, self.jobs, self.artifact]):
            native.verify_run("windows", self.entry, "generated/repository")

    def test_candidate_requires_both_build_and_native_playback_success(self):
        self.verify()
        self.jobs["jobs"][1]["conclusion"] = "failure"
        with self.assertRaisesRegex(ValueError, "successful platform playback"):
            self.verify()

    def test_other_workflows_cannot_supply_a_candidate(self):
        self.run["path"] = ".github/workflows/generated-unrelated.yml"
        with self.assertRaisesRegex(ValueError, "expected validation workflow"):
            self.verify()

    def test_a_validated_run_cannot_authorize_another_runs_artifact(self):
        self.artifact["workflow_run"]["id"] = 5
        with self.assertRaisesRegex(ValueError, "does not belong"):
            self.verify()

    def test_expired_or_replaced_artifacts_are_refused(self):
        for key, value in [("expired", True), ("digest", "sha256:" + "c" * 64)]:
            with self.subTest(key=key):
                original = self.artifact[key]
                self.artifact[key] = value
                with self.assertRaisesRegex(ValueError, "does not belong"):
                    self.verify()
                self.artifact[key] = original


class Promotion(unittest.TestCase):
    setUp = Candidate.setUp
    write_archive = Candidate.write_archive
    def test_publication_refuses_branch_pull_request_and_other_repositories(self):
        spec = importlib.util.spec_from_file_location("promote_native", Path(__file__).with_name("promote-native-candidates.py"))
        promote = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(promote)
        cases = [
            {"GITHUB_REF": "refs/heads/generated", "GITHUB_EVENT_NAME": "push", "GITHUB_REPOSITORY": self.repository},
            {"GITHUB_REF": "refs/heads/master", "GITHUB_EVENT_NAME": "pull_request", "GITHUB_REPOSITORY": self.repository},
            {"GITHUB_REF": "refs/heads/master", "GITHUB_EVENT_NAME": "push", "GITHUB_REPOSITORY": "generated/other"},
        ]
        for environment in cases:
            with self.subTest(environment=environment), mock.patch.dict(promote.os.environ, environment, clear=True):
                with mock.patch.object(promote.native, "load_registry", return_value={"repository": self.repository}):
                    with mock.patch.object(promote.native.sys, "argv", ["promote-native", "--publish"]):
                        with self.assertRaisesRegex(ValueError, "trusted master push"):
                            promote.main()

    def test_release_checksums_must_match_the_candidate_manifest(self):
        spec = importlib.util.spec_from_file_location("promote_native", Path(__file__).with_name("promote-native-candidates.py"))
        promote = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(promote)
        with zipfile.ZipFile(self.archive, "a") as archive:
            archive.writestr("SHA256SUMS", "generated false checksum list")
        registry = json.loads(self.registry_path.read_text())
        registry["platforms"]["windows"]["artifact_sha256"] = native.digest(self.archive)
        with mock.patch.object(native, "verify_run"), mock.patch.object(native, "expected_tag", return_value=self.tag):
            with self.assertRaisesRegex(ValueError, "SHA256SUMS differs"):
                promote.release_assets("windows", self.pin, registry, self.cache)


class DraftRelease(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location("promote_native", Path(__file__).with_name("promote-native-candidates.py"))
        self.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.module)
        self.tag = "mpv-runtime-" + "a" * 64
        self.draft = {"id": 17, "tag_name": self.tag, "draft": True}

    def test_an_uploaded_draft_is_found_before_its_tag_exists(self):
        with mock.patch.object(native, "gh_json", side_effect=[RuntimeError("HTTP 404"), [self.draft]]):
            self.assertEqual(self.module.existing_release("generated/repository", self.tag), self.draft)

    def test_duplicate_drafts_are_refused(self):
        with mock.patch.object(native, "gh_json", side_effect=[RuntimeError("HTTP 404"), [self.draft, {**self.draft, "id": 18}]]):
            with self.assertRaisesRegex(ValueError, "Multiple dependency drafts"):
                self.module.existing_release("generated/repository", self.tag)

    def test_permission_errors_are_not_treated_as_missing_drafts(self):
        with mock.patch.object(native, "gh_json", side_effect=RuntimeError("HTTP 403")) as api:
            with self.assertRaisesRegex(RuntimeError, "HTTP 403"):
                self.module.existing_release("generated/repository", self.tag)
            self.assertEqual(api.call_count, 1)

    def test_only_a_complete_matching_draft_can_resume_without_uploading(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            names = [*native.SPECS["windows"][1:4], "SHA256SUMS"]
            paths = []
            for name in names:
                path = root / name
                path.write_bytes(("generated " + name).encode())
                paths.append(path)
            pin = {"schema": 1, "tag": self.tag, "url": f"https://github.com/generated/repository/releases/download/{self.tag}/{names[0]}",
                   "source_url": f"https://github.com/generated/repository/releases/download/{self.tag}/{names[1]}",
                   "sha256": native.digest(paths[0]), "source_sha256": native.digest(paths[1])}
            draft = {**self.draft, "assets": [{"name": path.name, "digest": "sha256:" + native.digest(path)} for path in paths]}
            registry = {"repository": "generated/repository"}
            with mock.patch.object(native, "load_pin", return_value=pin), mock.patch.object(self.module, "release_assets", return_value=paths):
                with mock.patch.object(self.module, "existing_release", return_value=draft), mock.patch.object(self.module.subprocess, "run") as command:
                    self.module.promote("windows", registry, root, True)
                    command.assert_called_once_with(["gh", "release", "edit", self.tag, "--repo", "generated/repository", "--draft=false", "--latest=false"], check=True)
                draft["assets"][0]["digest"] = "sha256:" + "c" * 64
                with mock.patch.object(self.module, "existing_release", return_value=draft), mock.patch.object(self.module.subprocess, "run") as command:
                    with self.assertRaisesRegex(ValueError, "refusing to overwrite"):
                        self.module.promote("windows", registry, root, True)
                    command.assert_not_called()


if __name__ == "__main__":
    unittest.main()
