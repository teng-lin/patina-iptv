#!/usr/bin/env python3
"""Both ARM architectures must survive APK/AAB packaging before upload/signing."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile

CHECK = Path(__file__).with_name("check-android-artifacts.py")
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


if __name__ == "__main__":
    unittest.main()
