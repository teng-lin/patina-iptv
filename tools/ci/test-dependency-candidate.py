#!/usr/bin/env python3
"""Dependency candidates require the pin and never bypass failed validation."""
import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import Mock

SPEC = importlib.util.spec_from_file_location("candidate", Path(__file__).with_name("dependency-candidate.py"))
c = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(c)


class Candidate(unittest.TestCase):
    def test_pinned_sibling_precedes_update_and_validation(self):
        runner = Mock(return_value=subprocess.CompletedProcess([], 0))
        self.assertEqual(c.candidate(runner=runner), 0)
        commands = [call.args[0] for call in runner.call_args_list]
        self.assertEqual(commands[0][-1], "check-nsh")
        self.assertEqual(commands[1], ["cargo", "update"])
        self.assertEqual(commands[2], ["cargo", "deny", "--locked", "check"])
        self.assertEqual(commands[3][-3:], ["run", "--tier", "pr"])
        self.assertFalse(any(command[0] in ("git", "gh") for command in commands))

    def test_each_failure_stops_later_commands(self):
        for index in range(4):
            runner = Mock(side_effect=[subprocess.CompletedProcess([], 0)] * index + [subprocess.CompletedProcess([], 7)])
            self.assertEqual(c.candidate(runner=runner), 7)
            self.assertEqual(runner.call_count, index + 1)


if __name__ == "__main__":
    unittest.main()
