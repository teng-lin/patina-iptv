#!/usr/bin/env python3
"""Exercise the real smoke wait against synthetic ActivityManager observations."""
import json
import importlib.util
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
SOURCE = (ROOT / "tools/ci/android-smoke.sh").read_text()
WAIT = re.search(r"(?ms)^wait_stopped\(\) \{\n.*?^\}", SOURCE)
BACKGROUND = re.search(r'(?ms)^step "background and resume"\n(.*?)^echo "  ok: activity stopped"', SOURCE)
if WAIT is None or BACKGROUND is None:
    raise RuntimeError("Android smoke must expose its stopped-state wait and background journey")

MOCK = '''import json, os, pathlib, sys
root = pathlib.Path(os.environ["LIFECYCLE_CASE"])
# Diagnostic snapshots write directly to a named file. Poll observations are
# captured through a pipe by wait_stopped; keep their response sequence separate.
snapshot = next((path for path in root.glob("activities-*.log")
                 if os.path.samestat(path.stat(), os.fstat(sys.stdout.fileno()))), None)
if snapshot is not None:
    calls = root / "snapshots.json"
    history = json.loads(calls.read_text()) if calls.exists() else []
    history.append({"file": snapshot.name, "args": sys.argv[1:]})
    calls.write_text(json.dumps(history))
    print("mState=RESUMED")
    raise SystemExit(0)
calls = root / "calls.json"
history = json.loads(calls.read_text()) if calls.exists() else []
responses = json.loads((root / "responses.json").read_text())
response = responses[min(len(history), len(responses) - 1)]
history.append(sys.argv[1:])
calls.write_text(json.dumps(history))
print(response["text"])
raise SystemExit(response.get("code", 0))
'''


UI_SPEC = importlib.util.spec_from_file_location("android_ui_snapshot", ROOT / "tools/ci/android/ui-snapshot.py")
UI = importlib.util.module_from_spec(UI_SPEC)
UI_SPEC.loader.exec_module(UI)


class FavoritesAccessibility(unittest.TestCase):
    def tree(self, *nodes):
        root = ET.Element("hierarchy")
        for attributes in nodes:
            ET.SubElement(root, "node", attributes)
        return root

    def test_counts_accept_native_text_and_labels_without_double_counting(self):
        tree = self.tree({"text": "Favorites · Generated group · 2 of 3"},
                         {"content-desc": "Favorites · Generated group · 2 of 3"})
        self.assertEqual(UI.favorites_count(tree), (2, 3))
        self.assertEqual(UI.favorites_count(self.tree({"text": "Favorites · Generated · 0 of 3"})), (0, 3))

    def test_missing_conflicting_or_invalid_counts_fail_closed(self):
        for tree in (self.tree(),
                     self.tree({"text": "Favorites · Generated · 4 of 3"}),
                     self.tree({"text": "Favorites · Generated · 1 of 3"},
                               {"text": "Favorites · Other generated · 0 of 2"})):
            with self.assertRaises(ValueError):
                UI.favorites_count(tree)

    def test_menu_navigation_observes_actual_selected_item(self):
        tree = self.tree({"content-desc": "Manage favorites…", "selected": "false"},
                         {"content-desc": "Generated action", "selected": "true"})
        self.assertFalse(UI.selected(tree, "Manage favorites…"))
        tree[0].set("selected", "true")
        tree[1].set("selected", "false")
        self.assertTrue(UI.selected(tree, "Manage favorites…"))

    def test_touch_uses_enabled_button_bounds_not_its_child_text(self):
        tree = self.tree({"content-desc": "Cancel", "clickable": "true", "enabled": "true", "bounds": "[40,80][240,180]"},
                         {"text": "Cancel", "clickable": "false", "enabled": "true", "bounds": "[80,100][160,160]"})
        self.assertEqual(UI.button_center(tree, "Cancel"), (140, 130))

    def test_ambiguous_disabled_or_invalid_touch_targets_fail_closed(self):
        button = {"content-desc": "Remove all", "clickable": "true", "enabled": "true", "bounds": "[10,20][110,120]"}
        for tree in (self.tree(), self.tree(button, dict(button, bounds="[10,140][110,240]")),
                     self.tree(dict(button, enabled="false")),
                     self.tree(dict(button, bounds="[-10,20][0,120]")),
                     self.tree(dict(button, bounds="[10,20][10,120]"))):
            with self.assertRaises(ValueError):
                UI.button_center(tree, "Remove all")

    def test_confirmation_is_distinct_from_toolbar_remove_all(self):
        self.assertFalse(UI.confirmation_open(self.tree({"content-desc": "Remove all"})))
        self.assertTrue(UI.confirmation_open(self.tree({"text": UI.CONFIRMATION})))


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
            expected_call = ["shell", "dumpsys", "activity", "-p", "com.synthetic.patina", "activities"]
            for call in calls:
                self.assertEqual(call, expected_call)
            snapshots_file = root / "snapshots.json"
            snapshots = json.loads(snapshots_file.read_text()) if snapshots_file.exists() else []
            self.assertEqual(snapshots, [
                {"file": "activities-before-home.log", "args": expected_call},
            ] if background else [])
            if background:
                self.assertEqual((root / "activities-before-home.log").read_text(), "mState=RESUMED\n")
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
