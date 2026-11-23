#!/usr/bin/env python3
"""Both ARM architectures must survive APK/AAB packaging before upload/signing."""
import os
from pathlib import Path
import runpy
import struct
import subprocess
import sys
import tempfile
import unittest
import zipfile

CHECK = Path(__file__).with_name("check-android-artifacts.py")
MERGE = runpy.run_path(str(Path(__file__).with_name("merge-android-native.py")))["merge"]
NOTICES = ("THIRD-PARTY.html", "components.json", "NSH-RUNNERS.html",
           "runner-components.json", "gradle/index.json")


class Artifacts(unittest.TestCase):
    def check_packages(self, libraries, *, abi_override=None, missing_notice=None):
        with tempfile.TemporaryDirectory() as directory:
            artifacts = [Path(directory) / ("synthetic" + suffix) for suffix in (".apk", ".aab")]
            for artifact in artifacts:
                prefix = "base/" if artifact.suffix == ".aab" else ""
                with zipfile.ZipFile(artifact, "w") as package:
                    for abi, data in libraries.items():
                        package.writestr(prefix + f"lib/{abi}/libnsh.so", data)
                    for notice in NOTICES:
                        if notice != missing_notice:
                            package.writestr(prefix + "assets/notices/" + notice, b"synthetic")
            environment = os.environ.copy()
            environment.pop("ABIS", None)
            if abi_override is not None:
                environment["ABIS"] = abi_override
            return [subprocess.run([sys.executable, str(CHECK), str(artifact)], env=environment,
                                   capture_output=True, text=True) for artifact in artifacts]

    def test_universal_arm_packages_pass(self):
        for result in self.check_packages({"arm64-v8a": b"\x7fELFsynthetic64", "armeabi-v7a": b"\x7fELFsynthetic32"}):
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_either_missing_arm_library_fails(self):
        for present, missing in (("arm64-v8a", "armeabi-v7a"), ("armeabi-v7a", "arm64-v8a")):
            for result in self.check_packages({present: b"\x7fELFsynthetic"}):
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("missing required native library", result.stderr)
                self.assertIn(f"lib/{missing}/libnsh.so", result.stderr)

    def test_invalid_armv7_library_fails(self):
        for result in self.check_packages({"arm64-v8a": b"\x7fELFsynthetic", "armeabi-v7a": b"invalid"}):
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("invalid native library", result.stderr)

    def test_notices_remain_required(self):
        for result in self.check_packages({"arm64-v8a": b"\x7fELFsynthetic", "armeabi-v7a": b"\x7fELFsynthetic"},
                                          missing_notice="THIRD-PARTY.html"):
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("THIRD-PARTY.html", result.stderr)

    def test_emulator_abi_override(self):
        for result in self.check_packages({"x86_64": b"\x7fELFsynthetic"}, abi_override="x86_64"):
            self.assertEqual(result.returncode, 0, result.stderr)


class NativeArtifacts(unittest.TestCase):
    def setUp(self):
        root = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.inputs = root / "inputs"
        self.output = root / "output/nsh"
        self.shards = {}
        for abi, elf_class, machine, size in (("arm64-v8a", 2, 183, 64), ("armeabi-v7a", 1, 40, 52)):
            shard = self.inputs / f"android-native-{abi}"
            self.shards[abi] = shard
            library = shard / "jniLibs" / abi / "libnsh.so"
            library.parent.mkdir(parents=True)
            header = bytearray(size)
            header[:7] = b"\x7fELF" + bytes([elf_class, 1, 1])
            struct.pack_into("<HH", header, 16, 3, machine)
            library.write_bytes(header)
            (shard / "assets/nsh").mkdir(parents=True)
            (shard / "assets/nsh/app.json").write_text('{"id":"example.test"}')
            (shard / "commit.txt").write_text("synthetic-commit\n")

    def test_stages_both_libraries_and_one_copy_of_assets(self):
        MERGE(self.inputs, self.output, "synthetic-commit")
        self.assertEqual({str(path.relative_to(self.output)) for path in self.output.rglob("*") if path.is_file()}, {
            "assets/nsh/app.json", "jniLibs/arm64-v8a/libnsh.so", "jniLibs/armeabi-v7a/libnsh.so",
        })

    def test_wrong_commit_fails_before_staging(self):
        (self.shards["armeabi-v7a"] / "commit.txt").write_text("other-commit\n")
        with self.assertRaisesRegex(ValueError, "different commit"):
            MERGE(self.inputs, self.output, "synthetic-commit")
        self.assertFalse(self.output.exists())

    def test_swapped_or_missing_library_fails_before_staging(self):
        arm = self.shards["armeabi-v7a"] / "jniLibs/armeabi-v7a/libnsh.so"
        arm.write_bytes((self.shards["arm64-v8a"] / "jniLibs/arm64-v8a/libnsh.so").read_bytes())
        with self.assertRaisesRegex(ValueError, "shared library for armeabi-v7a"):
            MERGE(self.inputs, self.output, "synthetic-commit")
        arm.unlink()
        with self.assertRaises(FileNotFoundError):
            MERGE(self.inputs, self.output, "synthetic-commit")
        self.assertFalse(self.output.exists())

    def test_mismatched_assets_fail_before_staging(self):
        (self.shards["arm64-v8a"] / "assets/nsh/app.json").write_text("{}")
        with self.assertRaisesRegex(ValueError, "different NSH assets"):
            MERGE(self.inputs, self.output, "synthetic-commit")
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
