#!/usr/bin/env python3
"""The released SBOM carries no build paths and records the NSH commit."""
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("sbom", Path(__file__).with_name("sbom.py"))
sbom = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sbom)

ROOT = "/synthetic/build/patina"


def document(names=("iptv-core", "iptv-net", "iptv-store-sqlite", "nsh-core", "synthetic-dependency")):
    return json.dumps({
        "bomFormat": "CycloneDX",
        "metadata": {"component": {"bom-ref": f"path+file://{ROOT}/crates/iptv-app#1.2.3"}},
        "components": [{"name": name, "bom-ref": f"path+file:///synthetic/build/nsh/crates/{name}#1.0.0"}
                       for name in names],
    })


class Sbom(unittest.TestCase):
    def test_build_directories_become_repository_relative(self):
        result = json.loads(sbom.finish(document(), ROOT, "a" * 40))
        self.assertEqual(result["metadata"]["component"]["bom-ref"], "path+file://./crates/iptv-app#1.2.3")
        self.assertEqual(result["components"][3]["bom-ref"], "path+file://../nsh/crates/nsh-core#1.0.0")
        self.assertNotIn("/synthetic/build", json.dumps(result))

    def test_nsh_commit_and_scope_are_recorded(self):
        properties = json.loads(sbom.finish(document(), ROOT, "a" * 40))["metadata"]["properties"]
        self.assertIn({"name": "patina:nsh-revision", "value": "a" * 40}, properties)
        self.assertTrue(any(item["name"] == "patina:scope" for item in properties))

    def test_remaining_build_path_is_refused(self):
        text = document().replace('"bomFormat"', f'"note": "built in {ROOT}", "bomFormat"')
        with self.assertRaisesRegex(ValueError, "build directory"):
            sbom.finish(text, ROOT, "a" * 40)

    def test_empty_or_partial_graph_is_refused(self):
        with self.assertRaisesRegex(ValueError, "no component list"):
            sbom.finish(document(names=()), ROOT, "a" * 40)
        with self.assertRaisesRegex(ValueError, "nsh-core"):
            sbom.finish(document(names=("iptv-core", "iptv-net", "iptv-store-sqlite")), ROOT, "a" * 40)

    def test_media_runtime_sbom_from_components_dict(self):
        inventory = {
            "schema": 1,
            "components": [
                {
                    "name": "mpv",
                    "version": "0.42.0",
                    "url": "https://github.com/mpv-player/mpv/archive/test.tar.gz",
                    "sha256": "1" * 64,
                    "licenses": [{"path": "LICENSE.LGPL", "sha256": "2" * 64}],
                },
                {
                    "name": "ffmpeg",
                    "version": "8.1.3",
                    "url": "https://ffmpeg.org/releases/ffmpeg-8.1.3.tar.xz",
                    "sha256": "3" * 64,
                    "license_files": ["COPYING.LGPLv2.1"],
                },
            ],
        }
        raw = sbom.media_runtime_sbom(inventory, version="1.0.0")
        doc = json.loads(raw)
        self.assertEqual(doc["bomFormat"], "CycloneDX")
        self.assertEqual(doc["metadata"]["component"]["name"], "patina-media-runtime")
        self.assertEqual(len(doc["components"]), 2)
        names = {c["name"] for c in doc["components"]}
        self.assertEqual(names, {"mpv", "ffmpeg"})
        mpv = next(c for c in doc["components"] if c["name"] == "mpv")
        self.assertEqual(mpv["hashes"][0]["content"], "1" * 64)
        self.assertEqual(mpv["licenses"][0]["license"]["name"], "LICENSE.LGPL")

    def test_media_runtime_sbom_from_sources_lock(self):
        lock = {
            "schema": 1,
            "toolchain": {
                "name": "llvm-mingw",
                "version": "20261104",
                "url": "https://example.com/llvm-mingw.tar.xz",
                "sha256": "4" * 64,
                "license_files": ["LICENSE.TXT"],
            },
            "sources": [
                {
                    "name": "zlib",
                    "version": "1.3.1",
                    "url": "https://example.com/zlib.tar.gz",
                    "sha256": "5" * 64,
                    "license_files": ["LICENSE"],
                }
            ],
        }
        raw = sbom.media_runtime_sbom(lock)
        doc = json.loads(raw)
        self.assertEqual(len(doc["components"]), 2)
        names = {c["name"] for c in doc["components"]}
        self.assertEqual(names, {"llvm-mingw", "zlib"})

    def test_android_classpath_sbom(self):
        report = {
            "dependencies": [
                {
                    "moduleName": "androidx.annotation:annotation",
                    "moduleUrl": "https://developer.android.com/jetpack/androidx/releases/annotation#1.8.1",
                    "moduleVersion": "1.8.1",
                    "moduleLicense": "The Apache Software License, Version 2.0",
                    "moduleLicenseUrl": "http://www.apache.org/licenses/LICENSE-2.0.txt",
                },
                {
                    "moduleName": "com.google.errorprone:error_prone_annotations",
                    "moduleVersion": "2.27.0",
                },
            ]
        }
        raw = sbom.android_classpath_sbom(report, version="1.0.0")
        doc = json.loads(raw)
        self.assertEqual(doc["bomFormat"], "CycloneDX")
        self.assertEqual(doc["metadata"]["component"]["name"], "patina-android-classpath")
        self.assertEqual(len(doc["components"]), 2)
        androidx = next(c for c in doc["components"] if c["name"] == "annotation")
        self.assertEqual(androidx["group"], "androidx.annotation")
        self.assertEqual(androidx["purl"], "pkg:maven/androidx.annotation/annotation@1.8.1")
        self.assertEqual(androidx["licenses"][0]["license"]["name"], "The Apache Software License, Version 2.0")


if __name__ == "__main__":
    unittest.main()
