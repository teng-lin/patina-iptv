#!/usr/bin/env python3
"""Exercise the real smoke wait against synthetic ActivityManager observations."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SOURCE = (ROOT / "tools/ci/android-smoke.sh").read_text()
WAIT = re.search(r"(?ms)^wait_stopped\(\) \{\n.*?^\}", SOURCE)
BACKGROUND = re.search(r'(?ms)^step "background and resume"\n(.*?)^echo "  ok: activity stopped"', SOURCE)
if WAIT is None or BACKGROUND is None:
    raise RuntimeError("Android smoke must expose its stopped-state wait and background journey")

MOCK = '''import json, os, pathlib, sys
root = pathlib.Path(os.environ["LIFECYCLE_CASE"])
calls = root / "calls.json"
history = json.loads(calls.read_text()) if calls.exists() else []
responses = json.loads((root / "responses.json").read_text())
response = responses[min(len(history), len(responses) - 1)]
history.append(sys.argv[1:])
calls.write_text(json.dumps(history))
print(response["text"])
raise SystemExit(response.get("code", 0))
'''


@unittest.skipUnless(os.name == "posix" and shutil.which("bash"), "Android smoke uses a Unix host")
class Lifecycle(unittest.TestCase):
    def run_wait(self, responses, attempts=None, background=False):
        with tempfile.TemporaryDirectory(prefix="patina-lifecycle-") as temporary:
            root = Path(temporary)
            (root / "responses.json").write_text(json.dumps(responses))
            (root / "adb.py").write_text(MOCK)
            script = '''
pkg=com.synthetic.patina
out=$LIFECYCLE_CASE
adb() { "$LIFECYCLE_PYTHON" "$LIFECYCLE_CASE/adb.py" "$@"; }
pause() { :; }
mark() { :; }
keys() { :; }
fail() { echo "$*" >&2; exit 1; }
'''
            script += WAIT[0] + "\n"
            if background:
                script += BACKGROUND[1]
            else:
                script += "wait_stopped" + (f" {attempts}" if attempts is not None else "")
            result = subprocess.run(
                [shutil.which("bash"), "-euo", "pipefail", "-c", script], text=True,
                capture_output=True, check=False,
                env=dict(os.environ, LIFECYCLE_CASE=str(root), LIFECYCLE_PYTHON=sys.executable),
            )
            calls = json.loads((root / "calls.json").read_text())
            for call in calls:
                self.assertEqual(call, ["shell", "dumpsys", "activity", "-p", "com.synthetic.patina", "activities"])
            diagnostic = root / "activities.log"
            return result, calls, diagnostic.read_text() if diagnostic.exists() else None

    def test_one_successful_observation_is_not_rechecked(self):
        result, calls, diagnostic = self.run_wait([
            {"text": "ActivityRecord{synthetic state=STOPPED}"},
            {"text": "mState=RESUMED"},
        ], background=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 1)
        self.assertIsNone(diagnostic)

    def test_supported_activity_dump_formats(self):
        for stopped in ("state=STOPPED", "  mState=STOPPED", "  mAppStopped=true"):
            with self.subTest(stopped=stopped):
                result, calls, _ = self.run_wait([{"text": stopped}], 20)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(len(calls), 1)

    def test_transitional_states_wait_for_actual_stop(self):
        result, calls, _ = self.run_wait([
            {"text": "mState=RESUMED\nmAppStopped=false"},
            {"text": "mState=PAUSED"}, {"text": "mState=STOPPING"},
            {"text": ""}, {"text": "mState=STOPPED"},
        ], 20)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 5)

    def test_unstopped_activity_fails_with_existing_poll_limits_and_diagnostics(self):
        for attempts, expected, background in ((20, 20, False), (None, 40, False), (None, 20, True)):
            with self.subTest(attempts=attempts, background=background):
                result, calls, diagnostic = self.run_wait(
                    [{"text": "mState=RESUMED\nmAppStopped=false"}], attempts, background,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(len(calls), expected)
                self.assertIn("did not stop", result.stderr)
                self.assertIn("mState=RESUMED", diagnostic)

    def test_failed_adb_output_cannot_acknowledge_shutdown(self):
        result, calls, diagnostic = self.run_wait([{"text": "mState=STOPPED", "code": 7}], 20)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(calls), 1)
        self.assertIn("cannot inspect", result.stderr)
        self.assertIn("mState=STOPPED", diagnostic)

    def test_similar_field_values_do_not_acknowledge_shutdown(self):
        result, _, _ = self.run_wait([{"text": "mState=STOPPED_LATER\nmAppStopped=trueSoon"}], 2)
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
