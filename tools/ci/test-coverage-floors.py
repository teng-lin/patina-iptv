#!/usr/bin/env python3
"""Tests for check-coverage-floors.py."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("check_coverage_floors", Path(__file__).with_name("check-coverage-floors.py"))
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)

SYNTHETIC_SUMMARY = """
home/runner/work/patina/patina/crates/iptv-core/src/browse.rs    100 2 98.00% 10 0 100.00% 50 1 98.00% 0 0 -
home/runner/work/patina/patina/crates/iptv-core/src/guide.rs     200 4 98.00% 20 0 100.00% 50 4 92.00% 0 0 -
home/runner/work/patina/patina/crates/iptv-nav/src/action.rs     100 0 100.00% 10 0 100.00% 100 1 99.00% 0 0 -
"""


class CoverageFloors(unittest.TestCase):
    def test_parse_summary(self):
        totals = checker.parse_summary(SYNTHETIC_SUMMARY)
        self.assertEqual(totals["iptv-core"], [100, 5])
        self.assertEqual(totals["iptv-nav"], [100, 1])

    def test_shortened_and_windows_paths_keep_the_same_totals(self):
        shortened = "\n".join(line.split("crates/")[-1] for line in SYNTHETIC_SUMMARY.splitlines())
        expected = checker.parse_summary(SYNTHETIC_SUMMARY)
        self.assertEqual(checker.parse_summary(shortened), expected)
        self.assertEqual(checker.parse_summary(shortened.replace("/", "\\")), expected)

    def test_evaluate_passes_when_above_floor(self):
        totals = {"iptv-core": [100, 5]}  # 95%
        results, failed = checker.evaluate(totals, {"iptv-core": 94.0})
        self.assertEqual(failed, [])
        self.assertTrue(results["iptv-core"][2])

    def test_evaluate_fails_when_below_floor(self):
        totals = {"iptv-core": [100, 10]}  # 90%
        results, failed = checker.evaluate(totals, {"iptv-core": 94.0})
        self.assertEqual(failed, [("iptv-core", 90.0, 94.0)])
        self.assertFalse(results["iptv-core"][2])

    def test_evaluate_fails_when_crate_missing(self):
        totals = {}
        results, failed = checker.evaluate(totals, {"iptv-core": 94.0})
        self.assertEqual(failed, [("iptv-core", 0.0, 94.0)])
        self.assertFalse(results["iptv-core"][2])


if __name__ == "__main__":
    unittest.main()
