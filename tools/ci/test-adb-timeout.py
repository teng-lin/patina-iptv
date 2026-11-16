#!/usr/bin/env python3
"""Synthetic subprocess coverage of the smoke adb process-group deadline."""
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

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

    def hanging_command(self, parent_ignores_term=True):
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
while True: signal.pause()
''', str(state), str(ready), descendant,
                               "ignore" if parent_ignores_term else "exit", "generated-private-argument")
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
        command, state = self.hanging_command(parent_ignores_term=False)
        try:
            result = subprocess.run(command, capture_output=True, timeout=6, check=False)
            self.assertEqual(result.returncode, 124)
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
    def action_script(self):
        workflow = (ROOT / ".github/workflows/android-smoke.yml").read_text()
        # This deliberately accepts only a flat >- block: more-indented or
        # blank lines preserve YAML newlines that upstream executes separately.
        block = workflow.split("          script: >-\n", 1)[1].split("      - uses:", 1)[0]
        lines = block.splitlines()
        self.assertTrue(lines)
        self.assertTrue(all(line.startswith("            ") and line[12:] == line[12:].lstrip()
                            and line[12:] for line in lines))
        return " ".join(line[12:] for line in lines)

    def test_smoke_routes_all_adb_calls_through_the_guard_and_names_cleanup(self):
        script = (ROOT / "tools/ci/android-smoke.sh").read_text()
        self.assertNotRegex(script, r"\$adb(?:\s|$)")
        self.assertIn("adb() {", script)
        self.assertEqual(script.count('--diagnostic-fd 3 -- adb'), 2)
        self.assertIn('phase="failure diagnostics"', script)
        self.assertIn("phase=cleanup", script)
        self.assertIn('step() { phase=$*;', script)

    def test_action_has_its_own_limit_and_recovery_commands_are_guarded(self):
        workflow = (ROOT / ".github/workflows/android-smoke.yml").read_text()
        smoke = workflow.split("  smoke:\n", 1)[1]
        self.assertIn("timeout-minutes: 45", smoke)
        step = smoke.split("      - name: Smoke test on the", 1)[1]
        self.assertTrue(re.match(r"[^\n]+\n        timeout-minutes: 20\n", step))
        for line in workflow.splitlines():
            if re.search(r"\badb (?:reboot|wait-for-device)\b", line):
                self.assertIn("python3 tools/ci/android/adb.py", line)

    @unittest.skipUnless(os.name == "posix", "the action uses sh -c")
    def test_every_upstream_split_script_is_valid_sh_and_system_recovery_is_guarded(self):
        script = self.action_script()
        # Match pinned script-parser.ts, then main.ts's independent sh -c.
        commands = re.split(r"\r\n|\n|\r", script)
        self.assertEqual(len(commands), 1)
        for command in commands:
            subprocess.run(["sh", "-n", "-c", command], check=True, timeout=2)
        for system_crash in (False, True):
            with self.subTest(system_crash=system_crash), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                ci = root / "tools/ci"
                (ci / "android").mkdir(parents=True)
                shutil.copyfile(GUARD, ci / "android/adb.py")
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
                result = subprocess.run(["sh", "-c", commands[0]], cwd=root, capture_output=True,
                                        env={"PATH": str(binary) + os.pathsep + os.environ["PATH"]},
                                        timeout=5, check=False)
                self.assertEqual(result.returncode, 0 if system_crash else 1, result.stderr)
                self.assertEqual((root / "second-run").exists(), system_crash)
                if system_crash:
                    calls = [json.loads(line) for line in (root / "adb-calls.jsonl").read_text().splitlines()]
                    self.assertEqual(calls, [["reboot"], ["wait-for-device"]])
                else:
                    self.assertFalse((root / "adb-calls.jsonl").exists())


if __name__ == "__main__":
    unittest.main()
