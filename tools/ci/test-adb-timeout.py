#!/usr/bin/env python3
"""Synthetic subprocess coverage of the smoke adb process-group deadline."""
import json
import os
from pathlib import Path
import re
import runpy
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
GUARD = ROOT / "tools/ci/android/adb.py"


@unittest.skipUnless(os.name == "posix", "Android smoke runs on Linux/macOS")
class Deadline(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def command(self, script, *arguments, timeout="2", grace="0.15"):
        fake = self.root / "generated-adb.py"
        fake.write_text(script, encoding="utf-8")
        return [sys.executable, str(GUARD), "--timeout", timeout, "--grace", grace,
                "--phase", "generated lifecycle phase", "--category", "shell", "--",
                sys.executable, str(fake), *arguments]

    def test_success_preserves_argv_binary_stdin_stdout_and_stderr(self):
        payload = b"generated input\x00\nwith a second line\n"
        arguments = ["-s", "generated device serial", "shell", "an argument with spaces", "$(literal)"]
        command = self.command('''import json, sys
sys.stdout.buffer.write(json.dumps(sys.argv[1:]).encode() + b"\\n")
sys.stdout.buffer.write(sys.stdin.buffer.read())
sys.stderr.write("generated stderr\\n")
''', *arguments)
        result = subprocess.run(command, input=payload, capture_output=True, timeout=5, check=False)
        self.assertEqual(result.returncode, 0)
        first, body = result.stdout.split(b"\n", 1)
        self.assertEqual(json.loads(first), arguments)
        self.assertEqual(body, payload)
        self.assertEqual(result.stderr, b"generated stderr\n")

    def test_normal_nonzero_exit_is_preserved_without_guard_diagnostics(self):
        result = subprocess.run(self.command("raise SystemExit(7)\n"),
                                capture_output=True, timeout=5, check=False)
        self.assertEqual(result.returncode, 7)
        self.assertEqual(result.stdout, b"")
        self.assertEqual(result.stderr, b"")

    def hanging_command(self, parent="ignore"):
        state = self.root / "owned-pids.json"
        ready = self.root / "descendant-ready"
        descendant = '''import os, signal, sys
from pathlib import Path
signal.signal(signal.SIGTERM, signal.SIG_IGN)
Path(sys.argv[1]).write_text(str(os.getpid()))
while True: signal.pause()
'''
        command = self.command('''import json, os, signal, subprocess, sys, time
from pathlib import Path
if sys.argv[4] == "ignore": signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen([sys.executable, "-c", sys.argv[3], sys.argv[2]])
deadline = time.monotonic() + 1
while not Path(sys.argv[2]).exists() and time.monotonic() < deadline: time.sleep(0.01)
Path(sys.argv[1]).write_text(json.dumps([os.getpid(), child.pid]))
if sys.argv[4] == "success": raise SystemExit(0)
while True: signal.pause()
''', str(state), str(ready), descendant,
                               parent, "generated-private-argument")
        return command, state

    def assert_dead(self, state):
        self.assertTrue(state.exists(), "the fake adb and its descendant actually started")
        processes = json.loads(state.read_text())
        # A killed orphan can briefly remain as a zombie awaiting host reaping.
        for pid in processes:
            result = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)],
                                    capture_output=True, text=True, timeout=2, check=False)
            self.assertTrue(not result.stdout.strip() or result.stdout.lstrip().startswith("Z"),
                            f"owned fake process {pid} remains running")

    def kill_owned_group(self, state):
        if state.exists():
            try:
                os.killpg(json.loads(state.read_text())[0], signal.SIGKILL)
            except ProcessLookupError:
                pass

    def test_timeout_kills_term_ignoring_adb_and_descendant_with_status_124(self):
        command, state = self.hanging_command()
        try:
            result = subprocess.run(command, capture_output=True, text=True, timeout=6, check=False)
            self.assertEqual(result.returncode, 124)
            self.assertIn("generated lifecycle phase", result.stderr)
            self.assertIn("category='shell'", result.stderr)
            self.assertIn("timed out", result.stderr)
            self.assertNotIn("generated-private-argument", result.stderr)
            self.assert_dead(state)
        finally:
            self.kill_owned_group(state)

    def test_timeout_kills_descendant_even_when_parent_exits_on_term(self):
        command, state = self.hanging_command(parent="exit")
        try:
            result = subprocess.run(command, capture_output=True, timeout=6, check=False)
            self.assertEqual(result.returncode, 124)
            self.assert_dead(state)
        finally:
            self.kill_owned_group(state)

    def test_normal_exit_releases_stdout_retained_by_an_owned_descendant(self):
        command, state = self.hanging_command(parent="success")
        try:
            result = subprocess.run(command, capture_output=True, timeout=6, check=False)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(result.stdout, b"")
            self.assertEqual(result.stderr, b"")
            self.assert_dead(state)
        finally:
            self.kill_owned_group(state)

    def test_outer_termination_cleans_the_owned_group_and_returns_143(self):
        command, state = self.hanging_command()
        process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 3
            while not state.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue(state.exists(), "fake adb reached its hang before interruption")
            process.terminate()
            _, error = process.communicate(timeout=4)
            self.assertEqual(process.returncode, 143)
            self.assertIn(b"interrupted", error)
            self.assert_dead(state)
        finally:
            self.kill_owned_group(state)
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=2)

    def test_timeout_diagnostic_survives_suppressed_command_stderr(self):
        command = self.command("import time; time.sleep(30)\n", timeout="0.3")
        with (self.root / "diagnostics.log").open("w+") as log:
            command[2:2] = ["--diagnostic-fd", str(log.fileno())]
            result = subprocess.run(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                    pass_fds=(log.fileno(),), timeout=4, check=False)
            self.assertEqual(result.returncode, 124)
            log.seek(0)
            self.assertIn("category='shell'", log.read())


class Integration(unittest.TestCase):
    def test_smoke_routes_all_adb_calls_through_the_guard_and_names_cleanup(self):
        script = (ROOT / "tools/ci/android-smoke.sh").read_text()
        self.assertNotRegex(script, r"\$adb(?:\s|$)")
        self.assertIn("adb() {", script)
        self.assertEqual(script.count('--diagnostic-fd 3 -- adb'), 1)
        self.assertIn('--diagnostic-fd 3 -- adb -s "$serial"', script)
        self.assertIn('phase="failure diagnostics"', script)
        self.assertIn("phase=cleanup", script)
        self.assertIn('step() { phase=$*;', script)
        cleanup = script.split("cleanup() {", 1)[1].split("trap cleanup EXIT", 1)[0]
        self.assertLess(cleanup.index('adb shell am force-stop "$pkg"'),
                        cleanup.index('restore_setting global'))

    def test_workflow_caps_the_emulator_launcher(self):
        workflow = (ROOT / ".github/workflows/android-smoke.yml").read_text()
        smoke = workflow.split("  smoke:\n", 1)[1]
        self.assertIn("timeout-minutes: 45", smoke)
        step = smoke.split("      - name: Smoke test on the", 1)[1]
        self.assertTrue(re.match(r"[^\n]+\n        timeout-minutes: 20\n", step))
        self.assertIn("python3 tools/ci/android/ci-emulator.py", step)

    @unittest.skipUnless(os.name == "posix", "the CI emulator requires a Unix host")
    def test_sdk_reuses_installed_tools_and_requests_only_the_missing_image(self):
        with patch.object(sys, "path", [str(GUARD.parent), *sys.path]):
            prepare = runpy.run_path(str(GUARD.with_name("ci-emulator.py")))["prepare_sdk"]
        with tempfile.TemporaryDirectory() as directory, patch.object(subprocess, "run") as run:
            sdk = Path(directory)
            for relative in ("platform-tools/adb", "emulator/emulator", "build-tools/35.0.1/d8"):
                tool = sdk / relative
                tool.parent.mkdir(parents=True, exist_ok=True)
                tool.touch(mode=0o755)
            tools, image = prepare(sdk, "android-tv", {})
            self.assertEqual(run.call_args.args[0], [tools / "sdkmanager", "--install", image])
            self.assertEqual(image, "system-images;android-36;android-tv;x86_64")
            properties = sdk / "system-images/android-36/android-tv/x86_64/source.properties"
            properties.parent.mkdir(parents=True)
            properties.touch()
            run.reset_mock()
            prepare(sdk, "android-tv", {})
            run.assert_not_called()

    @unittest.skipUnless(os.name == "posix", "the CI emulator requires a Unix host")
    def test_boot_rechecks_avd_after_offline_settings_write(self):
        with patch.object(sys, "path", [str(GUARD.parent), *sys.path]):
            wait = runpy.run_path(str(GUARD.with_name("ci-emulator.py")))["wait_for_boot"]
        # Lose ADB at the final settings write, then recover on the same AVD.
        before_drop = ["generated", "1", "", "", ""]
        offline = subprocess.CalledProcessError(1, ["adb"], stderr="adb: device offline")
        adb = Mock(side_effect=[*before_drop, offline, *before_drop, ""])
        process = Mock()
        process.poll.return_value = None
        with patch.dict(wait.__globals__, adb=adb), patch.object(time, "sleep") as delay:
            wait(process, "generated", {})
            delay.assert_called_once()
            self.assertEqual(adb.call_args_list[6].args, ("emu", "avd", "name"))

        denied = subprocess.CalledProcessError(1, ["adb"], stderr="permission denied")
        with patch.dict(wait.__globals__, adb=Mock(side_effect=[*before_drop, denied])), \
                self.assertRaises(subprocess.CalledProcessError) as raised:
            wait(process, "generated", {})
        self.assertIs(raised.exception, denied)

    @unittest.skipUnless(os.name == "posix", "the CI emulator requires a Unix host")
    def test_emulator_system_recovery_is_guarded(self):
        command = (
            "import runpy; from pathlib import Path; import os, sys; "
            "sys.path.insert(0, 'tools/ci/android'); "
            "driver = runpy.run_path('tools/ci/android/ci-emulator.py'); "
            "sys.exit(driver['run_smoke']('generated.apk', os.environ, Path('target/android-smoke')))"
        )
        for system_crash in (False, True):
            with self.subTest(system_crash=system_crash), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                ci = root / "tools/ci"
                (ci / "android").mkdir(parents=True)
                shutil.copyfile(GUARD, ci / "android/adb.py")
                shutil.copyfile(GUARD.with_name("ci-emulator.py"), ci / "android/ci-emulator.py")
                smoke = ci / "android-smoke.sh"
                smoke.write_text('''#!/usr/bin/env bash
if [ ! -e first-run ]; then touch first-run; exit 1; fi
touch second-run
''')
                crash = root / "target/android-smoke/crash.log"
                crash.parent.mkdir(parents=True)
                crash.write_text("DeadSystemException" if system_crash else "generated app crash")
                binary = root / "bin"
                binary.mkdir()
                adb = binary / "adb"
                adb.write_text(f"#!{sys.executable}\n" + '''import json, sys
with open("adb-calls.jsonl", "a") as stream: stream.write(json.dumps(sys.argv[1:]) + "\\n")
''')
                adb.chmod(0o755)
                result = subprocess.run([sys.executable, "-c", command], cwd=root, capture_output=True,
                                        env={"PATH": str(binary) + os.pathsep + os.environ["PATH"]},
                                        timeout=5, check=False)
                self.assertEqual(result.returncode, 0 if system_crash else 1, result.stderr)
                self.assertEqual((root / "second-run").exists(), system_crash)
                if system_crash:
                    calls = [json.loads(line) for line in (root / "adb-calls.jsonl").read_text().splitlines()]
                    self.assertEqual(calls, [["-s", "emulator-5554", "reboot"], ["-s", "emulator-5554", "wait-for-device"]])
                else:
                    self.assertFalse((root / "adb-calls.jsonl").exists())


@unittest.skipUnless(shutil.which("javac") and shutil.which("java"), "requires a JDK")
class KeyInjection(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory()
        cls.root = Path(cls.directory.name)
        sources = {
            "android/view/InputEvent.java": "package android.view; public class InputEvent {}",
            "android/view/KeyEvent.java": '''package android.view;
public class KeyEvent extends InputEvent {
    public final long time; public final int action;
    public KeyEvent(long down, long time, int action, int code, int repeat) {
        this.time = time; this.action = action;
    }
    public static int keyCodeFromString(String name) { return name.equals("KEYCODE_DPAD_CENTER") ? 23 : 0; }
    public void setSource(int source) {}
}''',
            "android/os/SystemClock.java": '''package android.os;
public class SystemClock {
    public static long uptimeMillis() { return System.nanoTime() / 1000000; }
}''',
            "android/hardware/input/InputManagerGlobal.java": '''package android.hardware.input;
import android.view.InputEvent; import android.view.KeyEvent;
public class InputManagerGlobal {
    public static InputManagerGlobal getInstance() { return new InputManagerGlobal(); }
    public boolean injectInputEvent(InputEvent input, int mode) throws Exception {
        KeyEvent key = (KeyEvent) input;
        // Model delayed Down acknowledgement; a hold must begin after this.
        if (key.action == 0 && mode == 2) Thread.sleep(80);
        System.out.printf("%d %d %d %d%n", key.action, key.time, mode, System.nanoTime());
        return true;
    }
}''',
        }
        paths = []
        for relative, source in sources.items():
            path = cls.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source)
            paths.append(str(path))
        subprocess.run(["javac", "--release", "11", "-d", str(cls.root), *paths,
                        str(ROOT / "tools/ci/android/ShortKeys.java")], check=True, timeout=30)

    @classmethod
    def tearDownClass(cls):
        cls.directory.cleanup()

    def invoke(self, *arguments):
        return subprocess.run(["java", "-cp", str(self.root), "ShortKeys", *arguments],
                              capture_output=True, text=True, timeout=5, check=False)

    def test_short_pairs_queue_up_without_waiting_on_down(self):
        result = self.invoke("KEYCODE_DPAD_CENTER", "KEYCODE_DPAD_CENTER")
        self.assertEqual(result.returncode, 0, result.stderr)
        events = [list(map(int, line.split())) for line in result.stdout.splitlines()]
        self.assertEqual([(e[0], e[2]) for e in events], [(0, 0), (1, 2)] * 2)
        for down, up in zip(events[::2], events[1::2]):
            self.assertEqual(up[1], down[1] + 1)

    def test_deliberate_hold_waits_after_down_delivery_and_then_releases(self):
        result = self.invoke("--hold-ms", "100", "KEYCODE_DPAD_CENTER")
        self.assertEqual(result.returncode, 0, result.stderr)
        down, up = [list(map(int, line.split())) for line in result.stdout.splitlines()]
        self.assertEqual((down[0], down[2], up[0], up[2]), (0, 2, 1, 2))
        self.assertGreaterEqual(up[3] - down[3], 100_000_000)
        self.assertGreaterEqual(up[1] - down[1], 180)

    def test_invalid_holds_do_not_inject_events(self):
        for duration in ["0", "-1", "5001", "invalid"]:
            result = self.invoke("--hold-ms", duration, "KEYCODE_DPAD_CENTER")
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
