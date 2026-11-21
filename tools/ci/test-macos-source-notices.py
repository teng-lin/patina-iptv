#!/usr/bin/env python3
"""Exercise missing-bottle notices without Homebrew or network access."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("bundle", Path(__file__).parents[1] / "bundle-macos.py")
bundle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundle)


class SourceNotices(unittest.TestCase):
    def run_source(self, bad_hash=False, wrong_version=False):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:xz") as archive:
            for name in ["glib-2.90.1/LICENSES/LGPL.txt", "glib-2.90.1/../../LICENSE-escape"]:
                entry = tarfile.TarInfo(name)
                data = b"synthetic license text"
                entry.size = len(data)
                archive.addfile(entry, io.BytesIO(data))
        payload = buffer.getvalue()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            prefix = root / "glib/2.90.1"
            (prefix / ".brew").mkdir(parents=True)
            checksum = "0" * 64 if bad_hash else hashlib.sha256(payload).hexdigest()
            version = "2.90.0" if wrong_version else "2.90.1"
            (prefix / ".brew/glib.rb").write_text(
                f'url "https://download.gnome.org/sources/glib/2.90/glib-{version}.tar.xz"\n'
                f'sha256 "{checksum}"\n')
            destination = root / "notices"
            destination.mkdir()
            with patch.object(bundle.urllib.request, "urlopen", return_value=io.BytesIO(payload)):
                bundle.glib_source_notices(prefix, destination)
            self.assertEqual((destination / "source/LICENSES/LGPL.txt").read_text(), "synthetic license text")
            self.assertFalse((root / "LICENSE-escape").exists())
            self.assertTrue((destination / "source.json").exists())

    def test_matching_source_preserves_notices(self):
        self.run_source()

    def test_mismatched_checksum_refused(self):
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.run_source(bad_hash=True)

    def test_mismatched_version_refused(self):
        with self.assertRaisesRegex(ValueError, "matching source URL"):
            self.run_source(wrong_version=True)


class BundledSources(unittest.TestCase):
    """What the bundler seals into the app for every Homebrew keg it copies from."""

    FORMULA = ('class Synthetic < Formula\n  url "https://example.test/synthetic-1.2.3.tar.xz"\n'
               f'  sha256 "{"a" * 64}"\nend\n')

    def keg(self, root, name, version, formula):
        prefix = root / "Cellar" / name / version
        (prefix / ".brew").mkdir(parents=True)
        if formula:
            (prefix / f".brew/{name}.rb").write_text(formula)
        return prefix

    def test_manifest_note_and_formula_are_written(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            notices = root / "notices/native"
            notices.mkdir(parents=True)
            prefixes = {self.keg(root, "synthetic", "1.2.3_1", self.FORMULA),
                        self.keg(root, "another", "0.5", self.FORMULA.replace("synthetic-1.2.3", "another-0.5"))}
            bundle.record_sources(prefixes, notices, "9.8.7")
            description = json.loads((notices / "sources.json").read_text())
            self.assertEqual((description["platform"], description["release"], description["archive"]),
                             ("macos", "9.8.7", "Patina-9.8.7-macos-native-sources.tar"))
            self.assertEqual([(c["name"], c["version"], c["sources"][0]["url"]) for c in description["components"]], [
                ("another", "0.5", "https://example.test/another-0.5.tar.xz"),
                ("synthetic", "1.2.3_1", "https://example.test/synthetic-1.2.3.tar.xz")])
            self.assertEqual((notices / "synthetic-1.2.3_1/formula.rb").read_text(), self.FORMULA)
            self.assertIn("Patina-9.8.7-macos-native-sources.tar", (notices / "SOURCE.txt").read_text())

    def test_keg_without_a_checksummed_source_fails_the_bundle(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            notices = root / "notices/native"
            notices.mkdir(parents=True)
            for formula in (None, 'class Synthetic < Formula\n  url "https://example.test/x.tar.xz"\nend\n'):
                with self.subTest(formula=formula):
                    with self.assertRaises(ValueError):
                        bundle.record_sources({self.keg(root / str(bool(formula)), "synthetic", "1", formula)},
                                              notices, "9.8.7")
                    self.assertFalse((notices / "sources.json").exists())


if __name__ == "__main__":
    unittest.main()
