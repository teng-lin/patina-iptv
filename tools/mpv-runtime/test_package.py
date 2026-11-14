"""Synthetic packaging regressions; no network, native tools, or real streams."""
import contextlib
import io
import json
import unittest
from unittest import mock
import zipfile

import package
import prepare
import test_prepare


class PackagingTests(unittest.TestCase):
    def setUp(self):
        fixture = test_prepare.SourcePreparationTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.root = fixture.root
        self.lock, self.work, _ = fixture.fixture()
        with mock.patch.object(prepare.urllib.request, "urlopen", side_effect=AssertionError("No network")):
            prepare.prepare(self.lock, self.work)
        self.build = self.root / "build"
        self.logs = self.build / "logs"
        self.logs.mkdir(parents=True)
        for name in ("libmpv-2.dll", "ffmpeg.exe"):
            (self.build / name).write_bytes(b"MZ synthetic fixture " + name.encode())
        for name in ("cross.ini", "ffmpeg-config.h", "ffmpeg-config.mak", "mpv-options.json"):
            (self.logs / name).write_text("synthetic configuration\n", encoding="utf-8")
        for name in ("libmpv-imports.txt", "ffmpeg-imports.txt"):
            (self.logs / name).write_text("Import {\n  Name: KERNEL32.dll\n}\nImport {\n  Name: api-ms-win-crt-runtime-l1-1-0.dll\n}\n", encoding="utf-8")
        (self.logs / "binaries.sha256").write_text("".join(f"{prepare.digest(self.build / name)}  /build/{name}\n" for name in ("libmpv-2.dll", "ffmpeg.exe")), encoding="utf-8")
        self.output = self.root / "output"

    def run_package(self):
        with contextlib.redirect_stdout(io.StringIO()):
            package.package(self.lock, self.work, self.build, self.output, "synthetic/project")

    def test_runtime_hash_and_test_executable_separation(self):
        self.run_package()
        manifest = json.loads((self.output / "runtime.json").read_text())
        self.assertEqual(manifest["dll_sha256"], prepare.digest(self.build / "libmpv-2.dll"))
        self.assertEqual(manifest["sha256"], prepare.digest(self.output / package.RUNTIME))
        self.assertEqual(manifest["source_sha256"], prepare.digest(self.output / package.SOURCES))
        with zipfile.ZipFile(self.output / package.RUNTIME) as archive:
            self.assertNotIn("ffmpeg.exe", archive.namelist())
            self.assertEqual(archive.read("libmpv-2.dll"), (self.build / "libmpv-2.dll").read_bytes())
            self.assertIn("licenses/synthetic/COPYING", archive.namelist())
        with zipfile.ZipFile(self.output / "test-tools.zip") as archive:
            self.assertEqual(archive.namelist(), ["ffmpeg.exe"])

    def test_modified_lock_rejected(self):
        data = json.loads(self.lock.read_text())
        data["sources"][0]["version"] = "changed"
        self.lock.write_text(json.dumps(data), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "lock differs"):
            self.run_package()

    def test_modified_license_rejected(self):
        (self.work / "sources/synthetic/COPYING").write_text("changed", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "License changed"):
            self.run_package()

    def test_modified_dll_rejected(self):
        (self.build / "libmpv-2.dll").write_bytes(b"MZ modified")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.run_package()

    def test_external_or_missing_imports_rejected(self):
        for report in ("libmpv-imports.txt", "ffmpeg-imports.txt"):
            for content in ("Import {\n Name: libcodec-1.dll\n}\n", "no imports"):
                with self.subTest(report=report, content=content):
                    path = self.logs / report
                    original = path.read_text()
                    path.write_text(content, encoding="utf-8")
                    with self.assertRaisesRegex(ValueError, "DLL import"):
                        self.run_package()
                    path.write_text(original, encoding="utf-8")

    def test_recipe_digest_tracks_scripts_and_lock(self):
        scripts = self.root / "scripts"
        scripts.mkdir()
        script = scripts / "build.sh"
        script.write_bytes(b"first\n")
        with mock.patch.object(package, "__file__", str(scripts / "package.py")):
            first = package.recipe_digest(self.lock)
            script.write_bytes(b"first\r\n")
            self.assertEqual(first, package.recipe_digest(self.lock))
            script.write_bytes(b"second\n")
            second = package.recipe_digest(self.lock)
            self.assertNotEqual(first, second)
            data = json.loads(self.lock.read_text())
            data["sources"][0]["version"] = "2"
            self.lock.write_text(json.dumps(data), encoding="utf-8")
            self.assertNotEqual(second, package.recipe_digest(self.lock))


if __name__ == "__main__":
    unittest.main()

