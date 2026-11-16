#!/usr/bin/env python3
"""Simulated hdiutil failures never skip or weaken disk-image verification."""
import contextlib
import importlib.util
import io
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("verify-macos-dmg.py")
SPEC = importlib.util.spec_from_file_location("verify_macos_dmg", SCRIPT)
VERIFY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFY)
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


if __name__ == "__main__":
    unittest.main()
