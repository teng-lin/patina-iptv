#!/usr/bin/env python3
"""Exercise bootstrap archive provenance, integrity and fallback using synthetic files."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SPEC = importlib.util.spec_from_file_location("retained_tool", Path(__file__).with_name("restore-macos-tool.py"))
TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TOOL)
NAME = "patina-nsh-cli-ARM64-generated"
BINARY = b"synthetic executable"
REPOSITORY = "generated/example"
REPOSITORY_ID = 1
RECORD = {"repository_id": 1, "head_repository_id": 1, "head_branch": "master", "head_sha": "generated", "id": 3}
RUN = {"head_branch": "master", "head_sha": "generated", "event": "push", "path": ".github/workflows/validation.yml"}


def zipped(filename):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr(filename, BINARY)
    return buffer.getvalue()


class RetainedMacTool(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.destination = Path(directory.name) / "target/release/nsh"

    def repository(self, *, run=RUN, record=RECORD, contents=None, digest=None, expired=False):
        contents = zipped("nsh") if contents is None else contents
        artifact = {
            "name": NAME, "id": 2, "size_in_bytes": len(contents), "workflow_run": record,
            "digest": digest or "sha256:" + hashlib.sha256(contents).hexdigest(), "expired": expired,
        }
        data = {
            "artifacts": json.dumps({"artifacts": [artifact]}).encode(),
            "runs/3": json.dumps(run).encode(),
            "artifacts/2/zip": contents,
        }
        return patch.object(TOOL, "api", side_effect=lambda repo, endpoint, *_: data[endpoint])

    def restore(self):
        return TOOL.restore(REPOSITORY, REPOSITORY_ID, NAME, self.destination)

    def test_installs_the_verified_executable(self):
        with self.repository():
            self.assertTrue(self.restore())
        self.assertEqual(self.destination.read_bytes(), BINARY)
        self.assertTrue(self.destination.stat().st_mode & 0o111)

    def test_excludes_untrusted_runs_before_downloading(self):
        for record, run in (
            (RECORD | {"head_branch": "generated"}, RUN),
            (RECORD | {"head_repository_id": 99}, RUN),
            (RECORD, RUN | {"event": "pull_request"}),
            (RECORD, RUN | {"event": "pull_request_target"}),
            (RECORD, RUN | {"path": ".github/workflows/generated.yml"}),
            (RECORD, RUN | {"head_sha": "different"}),
        ):
            with self.subTest(record=record, run=run), self.repository(record=record, run=run) as api:
                self.assertFalse(self.restore())
                self.assertTrue(all(call.args[1] != "artifacts/2/zip" for call in api.call_args_list))
        self.assertFalse(self.destination.exists())

    def test_expired_archives_are_skipped(self):
        with self.repository(expired=True) as api:
            self.assertFalse(self.restore())
            self.assertEqual(api.call_count, 1)

    def test_archive_integrity_is_checked_before_install(self):
        with self.repository(digest="sha256:wrong"), self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.restore()
        with self.repository(contents=zipped("../nsh")), self.assertRaisesRegex(ValueError, "unexpected files"):
            self.restore()
        self.assertFalse(self.destination.exists())


if __name__ == "__main__":
    unittest.main()
