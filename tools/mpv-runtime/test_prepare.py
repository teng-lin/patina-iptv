"""Synthetic regression coverage; no external downloads or compiler required."""

import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile

import prepare


class SourcePreparationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def archive(self, members, filename="source.tar.gz"):
        path = self.root / filename
        with tarfile.open(path, "w:gz") as archive:
            for name, content in members:
                entry = tarfile.TarInfo(name)
                if isinstance(content, tuple):
                    entry.type, entry.linkname = content
                    archive.addfile(entry)
                else:
                    entry.size = len(content)
                    archive.addfile(entry, io.BytesIO(content))
        return path

    def fixture(self, licenses=None):
        source = self.archive([("synthetic-1/COPYING", b"Synthetic license text\n"), ("synthetic-1/main.c", b"/* generated fixture */\n")])
        item = {"name": "synthetic", "version": "1", "url": "https://example.invalid/source.tar.gz", "sha256": prepare.digest(source), "license_files": licenses or ["COPYING"]}
        lock = self.root / "lock.json"
        lock.write_text(json.dumps({"schema": 1, "sources": [item]}), encoding="utf-8")
        work = self.root / "work"
        cache = work / "cache"
        cache.mkdir(parents=True)
        (cache / item["sha256"]).write_bytes(source.read_bytes())
        return lock, work, item

    def test_cached_prepare_and_complete_deterministic_bundle(self):
        lock, work, item = self.fixture()
        with mock.patch.object(prepare.urllib.request, "urlopen", side_effect=AssertionError("Cache should avoid network")):
            prepare.prepare(lock, work)
        self.assertTrue((work / "sources/synthetic/main.c").is_file())
        first, second = self.root / "first.tar.gz", self.root / "second.tar.gz"
        prepare.bundle(lock, work, first)
        prepare.bundle(lock, work, second)
        self.assertEqual(first.read_bytes(), second.read_bytes())
        with tarfile.open(first) as archive:
            self.assertEqual(archive.extractfile(f"archives/synthetic-{item['sha256']}.archive").read(), (work / "cache" / item["sha256"]).read_bytes())
            self.assertEqual(archive.extractfile("licenses/synthetic/COPYING").read(), b"Synthetic license text\n")
            self.assertIn("build-scripts/prepare.py", archive.getnames())
            self.assertIn("components.json", archive.getnames())

    def test_missing_license_fails_closed(self):
        lock, work, _ = self.fixture(["MISSING"])
        with self.assertRaisesRegex(ValueError, "Missing or unsafe license"):
            prepare.prepare(lock, work)
        self.assertFalse((work / "components.json").exists())

    def test_modified_archive_cannot_be_bundled(self):
        lock, work, item = self.fixture()
        prepare.prepare(lock, work)
        (work / "cache" / item["sha256"]).write_bytes(b"corrupt")
        with self.assertRaisesRegex(ValueError, "modified source"):
            prepare.bundle(lock, work, self.root / "bundle.tar.gz")

    def test_modified_license_cannot_be_bundled(self):
        lock, work, _ = self.fixture()
        prepare.prepare(lock, work)
        (work / "sources/synthetic/COPYING").write_text("changed", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "License changed"):
            prepare.bundle(lock, work, self.root / "bundle.tar.gz")

    def test_changed_patch_receipt_cannot_be_bundled(self):
        lock, work, _ = self.fixture()
        prepare.prepare(lock, work)
        path = work / "components.json"
        inventory = json.loads(path.read_text())
        inventory["components"][0]["patches"] = [
            {"path": "patches/synthetic.patch", "sha256": "0" * 64}
        ]
        path.write_text(json.dumps(inventory))
        with self.assertRaisesRegex(ValueError, "patches changed after source preparation"):
            prepare.bundle(lock, work, self.root / "bundle.tar.gz")

    def test_corrupt_download_is_not_cached(self):
        item = {"name": "synthetic", "url": "https://example.invalid/source", "sha256": "0" * 64}
        response = io.BytesIO(b"wrong bytes")
        response.geturl = lambda: item["url"]
        cache = self.root / "downloads"
        with mock.patch.object(prepare.urllib.request, "urlopen", return_value=response):
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                prepare.download(item, cache)
        self.assertEqual(list(cache.iterdir()), [])

    def test_tar_traversal_and_escaping_links_rejected(self):
        for number, members in enumerate([
            [("../outside", b"bad")],
            [("/absolute", b"bad")],
            [("root/link", (tarfile.SYMTYPE, "../../outside"))],
            [("root/link", (tarfile.LNKTYPE, "../outside"))],
            [("root/COPYING", b"one"), ("root/COPYING", b"two")],
        ]):
            with self.subTest(members=members):
                archive = self.archive(members, f"unsafe-{number}.tar.gz")
                with self.assertRaises(ValueError):
                    prepare.extract(archive, self.root / f"extract-{number}")
        self.assertFalse((self.root / "outside").exists())

    def test_zip_traversal_rejected(self):
        archive = self.root / "unsafe.zip"
        with zipfile.ZipFile(archive, "w") as output:
            output.writestr("root/../../outside", b"bad")
        with self.assertRaises(ValueError):
            prepare.extract(archive, self.root / "zip")

    def test_zip_common_root_is_stripped(self):
        archive = self.root / "safe.zip"
        with zipfile.ZipFile(archive, "w") as output:
            output.writestr("synthetic/COPYING", b"Synthetic license")
        destination = self.root / "zip"
        prepare.extract(archive, destination)
        self.assertEqual((destination / "COPYING").read_bytes(), b"Synthetic license")

    def test_omitted_license_inventory_cannot_be_bundled(self):
        lock, work, _ = self.fixture()
        prepare.prepare(lock, work)
        path = work / "components.json"
        inventory = json.loads(path.read_text(encoding="utf-8"))
        inventory["components"][0]["licenses"] = []
        path.write_text(json.dumps(inventory), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "License inventory differs"):
            prepare.bundle(lock, work, self.root / "bundle.tar.gz")

    def test_retained_bundle_restores_without_network(self):
        lock, work, item = self.fixture()
        prepare.prepare(lock, work)
        retained = self.root / "retained.tar.gz"
        prepare.bundle(lock, work, retained)
        restored = self.root / "restored"
        with mock.patch.object(prepare.urllib.request, "urlopen", side_effect=AssertionError("Offline restore must not fetch")) as network:
            prepare.prepare(lock, restored, retained)
        network.assert_not_called()
        self.assertEqual((restored / "sources/synthetic/COPYING").read_bytes(), b"Synthetic license text\n")
        self.assertEqual(prepare.digest(restored / "cache" / item["sha256"]), item["sha256"])

    def test_retained_bundle_mismatch_rejected_without_network(self):
        lock, work, _ = self.fixture()
        prepare.prepare(lock, work)
        retained = self.root / "retained.tar.gz"
        prepare.bundle(lock, work, retained)
        value = json.loads(lock.read_text(encoding="utf-8"))
        value["sources"][0]["version"] = "changed"
        lock.write_text(json.dumps(value), encoding="utf-8")
        restored = self.root / "restored"
        with mock.patch.object(prepare.urllib.request, "urlopen") as network:
            with self.assertRaisesRegex(ValueError, "bundle lock differs"):
                prepare.prepare(lock, restored, retained)
        network.assert_not_called()
        self.assertFalse((restored / "cache").exists())

    def test_retained_bundle_missing_or_corrupt_archive_rejected(self):
        lock, _, item = self.fixture()
        for content in (None, b"corrupted archive"):
            members = [("sources.lock.json", lock.read_bytes())]
            if content is not None:
                members.append((f"archives/{item['name']}-{item['sha256']}.archive", content))
            # An additional directory keeps the bundle's root layout explicit.
            members.append(("build-scripts/example.py", b"# synthetic\n"))
            retained = self.archive(members, "retained.tar.gz")
            restored = self.root / "restored"
            with mock.patch.object(prepare.urllib.request, "urlopen") as network:
                with self.assertRaisesRegex(ValueError, "Missing or modified retained"):
                    prepare.prepare(lock, restored, retained)
            network.assert_not_called()
            self.assertFalse((restored / "cache").exists())

    def test_toolchain_and_duplicate_names_validated(self):
        lock, _, item = self.fixture()
        value = {"schema": 1, "sources": [item], "toolchain": {**item, "name": "SYNTHETIC"}}
        lock.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "duplicate"):
            prepare.read_lock(lock)
        value["toolchain"]["name"] = "synthetic-toolchain"
        lock.write_text(json.dumps(value), encoding="utf-8")
        self.assertEqual(prepare.read_lock(lock)[1][-1][1], "toolchain")


if __name__ == "__main__":
    unittest.main()
