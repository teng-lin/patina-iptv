"""Source dependency additions must invalidate a stale generated runner lock."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "package_lock", Path(__file__).with_name("check-package-lock.py"))
checks = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checks)


class SourceIdentities(unittest.TestCase):
    def test_missing_new_name_version_and_checksum_are_rejected(self):
        app = {"name": "synthetic-app", "version": "1.0.0"}
        dependency = {"name": "synthetic-resource-compiler", "version": "2.0.0",
                      "source": "registry+https://example.test/index", "checksum": "a" * 64}
        for packages in ([app], [app, dict(dependency, version="1.0.0")],
                         [app, dict(dependency, checksum="b" * 64)]):
            with self.subTest(packages=packages):
                with self.assertRaisesRegex(SystemExit, "synthetic-resource-compiler 2.0.0"):
                    checks.validate_source_identities(packages, [app, dependency])

    def test_runner_only_packages_and_extra_versions_are_allowed(self):
        dependency = {"name": "synthetic-dependency", "version": "2.0.0"}
        checks.validate_source_identities(
            [dependency, dict(dependency, version="1.0.0"),
             {"name": "synthetic-runner", "version": "1.0.0"}], [dependency])


class MemberDependencies(unittest.TestCase):
    REGISTRY = "registry+https://example.test/index"
    STORE = ("synthetic-store", "1.0.0", None)
    CODEC = ("synthetic-codec", "2.0.0", REGISTRY)

    def runner_lock(self, *app_dependencies):
        return [
            {"name": "synthetic-app", "version": "1.0.0",
             "dependencies": sorted(app_dependencies)},
            {"name": "synthetic-store", "version": "1.0.0"},
            {"name": "synthetic-codec", "version": "1.0.0", "source": self.REGISTRY},
            {"name": "synthetic-codec", "version": "2.0.0", "source": self.REGISTRY},
            # A registry package's list follows the runner's own features.
            {"name": "synthetic-runner-dependency", "version": "1.0.0",
             "source": self.REGISTRY, "dependencies": ["synthetic-store"]},
        ]

    def test_matching_lists_pass(self):
        checks.validate_member_dependencies(
            self.runner_lock("synthetic-codec 2.0.0", "synthetic-store"),
            {("synthetic-app", "1.0.0"): {self.STORE, self.CODEC}})

    def test_a_dev_dependency_turned_normal_is_rejected(self):
        # The identity is in both locks already; only the edge is new.
        with self.assertRaisesRegex(SystemExit, r"synthetic-app \(\+synthetic-store 1.0.0\)"):
            checks.validate_member_dependencies(
                self.runner_lock("synthetic-codec 2.0.0"),
                {("synthetic-app", "1.0.0"): {self.STORE, self.CODEC}})

    def test_a_removed_or_moved_dependency_is_rejected(self):
        with self.assertRaisesRegex(SystemExit, r"\+synthetic-codec 2.0.0, -synthetic-codec 1.0.0"):
            checks.validate_member_dependencies(
                self.runner_lock("synthetic-codec 1.0.0", "synthetic-store"),
                {("synthetic-app", "1.0.0"): {self.STORE, self.CODEC}})
        with self.assertRaisesRegex(SystemExit, r"-synthetic-store 1.0.0"):
            checks.validate_member_dependencies(
                self.runner_lock("synthetic-codec 2.0.0", "synthetic-store"),
                {("synthetic-app", "1.0.0"): {self.CODEC}})

    def test_registry_and_runner_only_packages_are_not_compared(self):
        checks.validate_member_dependencies(
            self.runner_lock("synthetic-store"),
            {("synthetic-app", "1.0.0"): {self.STORE},
             ("synthetic-unpackaged", "1.0.0"): {self.CODEC}})


if __name__ == "__main__":
    unittest.main()
