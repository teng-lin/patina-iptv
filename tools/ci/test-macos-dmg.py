#!/usr/bin/env python3
"""Simulated hdiutil failures never skip or weaken disk-image verification."""
import contextlib
import base64
import hashlib
import importlib.util
import io
from pathlib import Path
import subprocess
import plistlib
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("verify-macos-dmg.py")
SPEC = importlib.util.spec_from_file_location("verify_macos_dmg", SCRIPT)
VERIFY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFY)
SPARKLE_SPEC = importlib.util.spec_from_file_location("macos_sparkle", Path(__file__).with_name("macos-sparkle.py"))
SPARKLE = importlib.util.module_from_spec(SPARKLE_SPEC)
SPARKLE_SPEC.loader.exec_module(SPARKLE)
IMAGE = "/generated/Patina test image.dmg"
TRANSIENT = (
    f'hdiutil: verify: unable to recognize "{IMAGE}" as a disk image. (Resource temporarily unavailable)\n'
    "hdiutil: verify failed - Resource temporarily unavailable\n"
)


class Verification(unittest.TestCase):
    def run_verify(self, responses):
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.object(VERIFY.subprocess, "run", side_effect=responses) as run, \
                patch.object(VERIFY.time, "sleep") as sleep, \
                contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = VERIFY.main([IMAGE])
        for call in run.call_args_list:
            self.assertEqual(call.args, (["hdiutil", "verify", IMAGE],))
            self.assertEqual(call.kwargs["env"]["LC_ALL"], "C")
        return code, run, sleep, stdout.getvalue(), stderr.getvalue()

    def result(self, code=0, error="", output=""):
        return subprocess.CompletedProcess(["hdiutil", "verify", IMAGE], code, output, error)

    def test_success_verifies_once_and_preserves_output(self):
        code, run, sleep, output, error = self.run_verify([
            self.result(output="verified checksum\n"),
        ])
        self.assertEqual(code, 0)
        self.assertEqual(run.call_count, 1)
        sleep.assert_not_called()
        self.assertIn("verified checksum\n", output)
        self.assertIn("attempt 1/4", output)
        self.assertEqual(error, "")

    def test_transient_failure_retries_full_verification_then_succeeds(self):
        code, run, sleep, output, error = self.run_verify([
            self.result(1, TRANSIENT), self.result(1, TRANSIENT),
            self.result(output="verified checksum\n"),
        ])
        self.assertEqual(code, 0)
        self.assertEqual(run.call_count, 3)
        self.assertEqual([call.args for call in sleep.call_args_list], [(1,), (2,)])
        self.assertIn("attempt 3/4", output)
        self.assertIn("verified checksum", output)
        self.assertEqual(error.count(TRANSIENT), 2)

    def test_permanent_failure_fails_immediately_with_original_status(self):
        code, run, sleep, _, error = self.run_verify([
            self.result(7, "hdiutil: verify failed - checksum mismatch\n"),
        ])
        self.assertEqual(code, 7)
        self.assertEqual(run.call_count, 1)
        sleep.assert_not_called()
        self.assertIn("checksum mismatch", error)

    def test_transient_retries_are_bounded_and_keep_last_failure_status(self):
        code, run, sleep, output, _ = self.run_verify([
            self.result(status, TRANSIENT) for status in (1, 1, 1, 9)
        ])
        self.assertEqual(code, 9)
        self.assertEqual(run.call_count, 4)
        self.assertEqual([call.args for call in sleep.call_args_list], [(1,), (2,), (3,)])
        self.assertIn("attempt 4/4", output)

    def test_extra_corruption_diagnostic_is_not_retried(self):
        code, run, sleep, _, _ = self.run_verify([
            self.result(1, "checksum mismatch\n" + TRANSIENT),
        ])
        self.assertEqual(code, 1)
        self.assertEqual(run.call_count, 1)
        sleep.assert_not_called()

    def test_permanent_error_after_transient_stops_without_exhausting_attempts(self):
        code, run, sleep, _, _ = self.run_verify([
            self.result(1, TRANSIENT), self.result(2, "No such file or directory\n"),
        ])
        self.assertEqual(code, 2)
        self.assertEqual(run.call_count, 2)
        sleep.assert_called_once_with(1)


class SparkleConfiguration(unittest.TestCase):
    def environment(self, archive="/generated/Sparkle.tar.xz"):
        return dict(zip(SPARKLE.CONFIG, (archive, "https://updates.example.test/appcast.xml",
                                        base64.b64encode(bytes(32)).decode())))

    def test_unconfigured_and_partial_configuration_fail_closed(self):
        self.assertIsNone(SPARKLE.configuration({}))
        complete = self.environment()
        for name in SPARKLE.CONFIG:
            partial = complete.copy()
            del partial[name]
            with self.subTest(missing=name), self.assertRaises(ValueError):
                SPARKLE.configuration(partial)

    def test_transport_and_public_key_validation(self):
        for value in ("http://updates.example.test/a", "https://user@updates.example.test/a",
                      "https://@updates.example.test/a", "https://updates.example.test/a#",
                      "https://updates.example.test/a#fragment", "https:///a"):
            environment = self.environment()
            environment["PATINA_SPARKLE_FEED"] = value
            with self.subTest(feed=value), self.assertRaises(ValueError):
                SPARKLE.configuration(environment)
        for value in ("not base64", base64.b64encode(bytes(31)).decode()):
            environment = self.environment()
            environment["PATINA_SPARKLE_PUBLIC_KEY"] = value
            with self.subTest(key=value), self.assertRaises(ValueError):
                SPARKLE.configuration(environment)

    def test_unconfigured_preview_does_not_modify_bundle(self):
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory) / "Patina.app"
            self.assertFalse(SPARKLE.configure(app, {}))
            self.assertFalse(app.exists())

    def test_pinned_bundle_preserves_version_and_requires_user_confirmation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app = root / "Patina.app"
            (app / "Contents").mkdir(parents=True)
            path = app / "Contents/Info.plist"
            path.write_bytes(plistlib.dumps({"CFBundleVersion": "42", "CFBundleShortVersionString": "0.2.0"}))
            archive = root / "Sparkle.tar.xz"
            fields = {"Sparkle.framework/Versions/B/Resources/Info.plist": plistlib.dumps({
                "CFBundleIdentifier": "org.sparkle-project.Sparkle", "CFBundleShortVersionString": SPARKLE.VERSION}),
                "LICENSE": b"Synthetic upstream license"}
            with tarfile.open(archive, "w:xz") as stream:
                for name, payload in fields.items():
                    member = tarfile.TarInfo(name)
                    member.size = len(payload)
                    stream.addfile(member, io.BytesIO(payload))
                link = tarfile.TarInfo("Sparkle.framework/Resources")
                link.type = tarfile.SYMTYPE
                link.linkname = "Versions/B/Resources"
                stream.addfile(link)
            environment = self.environment(str(archive))
            with self.assertRaisesRegex(ValueError, "checksum"):
                SPARKLE.configure(app, environment)
            self.assertNotIn("PatinaSparkleEnabled", plistlib.loads(path.read_bytes()))
            with patch.object(SPARKLE, "SHA256", hashlib.sha256(archive.read_bytes()).hexdigest()), \
                    patch.object(SPARKLE.subprocess, "run") as verify:
                self.assertTrue(SPARKLE.configure(app, environment))
            verify.assert_called_once()
            self.assertEqual(verify.call_args.args[0][:4], ["codesign", "--verify", "--deep", "--strict"])
            info = plistlib.loads(path.read_bytes())
            self.assertEqual(info["CFBundleVersion"], "42")
            self.assertEqual(info["CFBundleShortVersionString"], "0.2.0")
            self.assertTrue(info["PatinaSparkleEnabled"])
            for key in ("SUEnableAutomaticChecks", "SUAllowsAutomaticUpdates", "SUAutomaticallyUpdate", "SUEnableJavaScript"):
                self.assertFalse(info[key])
            self.assertEqual((app / "Contents/Resources/notices/sparkle/LICENSE").read_bytes(), b"Synthetic upstream license")

    @unittest.skipUnless(sys.platform == "darwin", "requires native AppKit/Security")
    def test_native_version_and_recording_restart_fences(self):
        source = Path(__file__).with_name("fixtures") / "macos-sparkle.m"
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "sparkle-contract"
            subprocess.run(["clang", "-fobjc-arc", "-Werror", str(source), "-framework", "AppKit",
                            "-framework", "Security", "-o", str(executable)], check=True)
            subprocess.run([str(executable)], check=True)
            marker = Path(directory) / "normal-quit.txt"
            subprocess.run([str(executable), "--quit-child", str(marker)], check=True, timeout=15)
            self.assertIn("Cocoa cancelled", marker.read_text())


if __name__ == "__main__":
    unittest.main()
