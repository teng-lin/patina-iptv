"""Generated artifact and process-environment contracts, without an Apple SDK."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile

import build


class Inputs(unittest.TestCase):
    def test_format_source_matches_the_pinned_mpvkit_component(self):
        notices = json.loads((build.ROOT / "tools/ci/mpvkit-licenses.json").read_text())
        component = next(item for item in notices["components"] if item["component"] == "FFmpeg")
        lock, components = build.prepare.read_lock(build.HERE / "sources.lock.json")
        self.assertEqual(len(components), 1)
        item = lock["sources"][0]
        self.assertEqual("n" + item["version"], component["ref"])
        self.assertIn(component["commit"], item["url"])

    def test_framework_zip_has_stable_bytes_and_an_extractable_root(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            framework = root / "Generated.xcframework"
            framework.mkdir()
            (framework / "generated").write_bytes(b"synthetic archive contents")
            first, second = root / "first.zip", root / "second.zip"
            build.zip_bundle(framework, first)
            os.utime(framework / "generated", (100, 100))
            build.zip_bundle(framework, second)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            with zipfile.ZipFile(first) as archive:
                self.assertEqual(archive.namelist(), ["Generated.xcframework/generated"])
                self.assertEqual(archive.read(archive.namelist()[0]), b"synthetic archive contents")

    @unittest.skipIf(os.name == "nt", "the iOS simulator launcher runs on macOS")
    def test_launcher_forwards_proxy_values_and_removes_stale_child_values(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            shim = root / "xcrun"
            shim.write_text(f"#!{sys.executable}\nimport json, os\nprint(json.dumps({{key: value for key, value in os.environ.items() if key.startswith('SIMCTL_CHILD_')}}))\n")
            shim.chmod(0o755)
            entry = root / "launch.py"
            build.launcher(entry, "generated-device", root / "generated-probe")
            environment = {
                "PATH": str(root) + os.pathsep + os.environ["PATH"],
                "http_proxy": "http://127.0.0.1:9999",
                "NO_PROXY": "127.0.0.1",
                "SIMCTL_CHILD_http_proxy": "http://127.0.0.1:1111",
                "SIMCTL_CHILD_no_proxy": "stale generated value",
            }
            result = subprocess.run([sys.executable, str(entry)], env=environment, capture_output=True, text=True, check=True)
            self.assertEqual(
                json.loads(result.stdout),
                {
                    "SIMCTL_CHILD_http_proxy": "http://127.0.0.1:9999",
                    "SIMCTL_CHILD_NO_PROXY": "127.0.0.1",
                },
            )


if __name__ == "__main__":
    unittest.main()
