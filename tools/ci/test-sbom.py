#!/usr/bin/env python3
"""The released SBOMs: no build paths in the Rust one, and nothing guessed in the others.

Synthetic data only. The media runtime tests use hand-written inventories in
both packaged schemas and inventories written by the real packagers
(tools/mpv-runtime/package.py, tools/mpv-runtime-unix/runtime.py) from
generated archives; nothing is downloaded.
"""
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile

spec = importlib.util.spec_from_file_location("sbom", Path(__file__).with_name("sbom.py"))
sbom = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sbom)

ROOT = "/synthetic/build/patina"
TOOLS = Path(__file__).resolve().parents[1]
RECIPE, LIBRARY, TEST_TOOL, SOURCE_ARCHIVE = "a" * 64, "b" * 64, "c" * 64, "d" * 64


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


def component(name, version, digit, role="source", licenses=(("COPYING", "9"),)):
    """One row as prepare.py writes it: the lock entry, its role and its license file hashes."""
    return {"name": name, "version": version, "url": f"https://example.invalid/{name}-{version}.tar.gz",
            "sha256": digit * 64, "license_files": [path for path, _ in licenses], "role": role,
            "licenses": [{"path": path, "sha256": value * 64} for path, value in licenses]}


def sources():
    return [component("mpv", "0.42.0", "1", licenses=(("LICENSE.LGPL", "2"), ("Copyright", "3"))),
            component("ffmpeg", "8.1.3", "4", licenses=(("COPYING.LGPLv2.1", "5"),))]


def windows_inventory():
    """The components.json of tools/mpv-runtime/package.py."""
    tag = "mpv-runtime-" + RECIPE
    return {"schema": 1,
            "components": sources() + [component("llvm-mingw", "20261104", "6", role="toolchain")],
            "build": {"tag": tag, "recipe_sha256": RECIPE, "dll_sha256": LIBRARY, "ffmpeg_sha256": TEST_TOOL,
                      "source_url": f"https://example.invalid/download/{tag}/mpv-runtime-sources.tar.gz",
                      "source_sha256": SOURCE_ARCHIVE}}


def unix_inventory(platform):
    """The components.json of tools/mpv-runtime-unix/runtime.py."""
    tag = f"mpv-runtime-{platform}-{RECIPE}"
    facts = {"linux": {"arch": "x86_64", "library": "libmpv.so.2", "library_build_id": "0123456789abcdef" * 2 + "01234567",
                       "glibc_minimum": "2.34", "needed": ["libc.so.6"]},
             "macos": {"arch": "arm64", "library": "libmpv.2.dylib", "macos_minimum": "14.0",
                       "library_uuid": "0A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F9"}}[platform]
    return {"schema": 1,
            "components": sources(),
            "build": {"platform": platform, "tag": tag, "recipe_sha256": RECIPE, "library_sha256": LIBRARY,
                      "ffmpeg_sha256": TEST_TOOL, "source_sha256": SOURCE_ARCHIVE,
                      "source_url": f"https://example.invalid/download/{tag}/mpv-runtime-{platform}-sources.tar.gz",
                      **facts}}


def manifest_for(inventory):
    """The runtime manifest either packager writes beside the archive of `inventory`."""
    build = inventory["build"]
    manifest = {key: value for key, value in build.items() if key not in ("recipe_sha256", "ffmpeg_sha256", "needed")}
    return {"schema": 1, "url": build["source_url"].replace("sources", "runtime"), "sha256": "e" * 64, **manifest}


def properties(record):
    return {item["name"]: item["value"] for item in record.get("properties", [])}


def named(document, name):
    return next(item for item in document["components"] if item["name"] == name)


class MediaRuntime(unittest.TestCase):
    def document(self, inventory, **options):
        return json.loads(sbom.media_runtime_sbom(inventory, version="1.2.3", **options))

    def test_windows_runtime_document(self):
        document = self.document(windows_inventory())
        self.assertEqual((document["bomFormat"], document["specVersion"]), ("CycloneDX", "1.5"))
        self.assertEqual(document["metadata"]["component"]["name"], "patina-media-runtime")
        self.assertEqual(document["metadata"]["component"]["version"], "1.2.3")
        facts = properties(document["metadata"])
        self.assertEqual(facts["patina:runtime-platform"], "windows")
        self.assertEqual(facts["patina:runtime-arch"], "x86_64")
        self.assertEqual(facts["patina:runtime-tag"], "mpv-runtime-" + RECIPE)
        self.assertEqual(facts["patina:runtime-recipe-sha256"], RECIPE)
        self.assertEqual([item["name"] for item in document["components"]], ["libmpv-2.dll", "mpv", "ffmpeg"])
        library = document["components"][0]
        self.assertEqual(library["hashes"], [{"alg": "SHA-256", "content": LIBRARY}])
        self.assertEqual(library["version"], "mpv-runtime-" + RECIPE)
        self.assertEqual(properties(library)["patina:role"], "bundled-binary")
        self.assertEqual(document["dependencies"], [{"ref": "runtime", "dependsOn": ["library"]},
                                                    {"ref": "library", "dependsOn": ["source:mpv", "source:ffmpeg"]}])
        references = [item["bom-ref"] for item in document["components"]] + ["runtime", "build", "toolchain:llvm-mingw"]
        self.assertEqual(len(references), len(set(references)))

    def test_linux_and_macos_runtime_documents(self):
        expected = {"linux": ("x86_64", "libmpv.so.2", "patina:library-build-id", "patina:glibc-minimum"),
                    "macos": ("arm64", "libmpv.2.dylib", "patina:library-uuid", "patina:macos-minimum")}
        for platform, (arch, name, identity, minimum) in expected.items():
            with self.subTest(platform=platform):
                inventory = unix_inventory(platform)
                # Naming the platform and leaving it to the inventory give the same document.
                document = self.document(inventory, platform=platform)
                self.assertEqual(document, self.document(inventory))
                facts = properties(document["metadata"])
                self.assertEqual((facts["patina:runtime-platform"], facts["patina:runtime-arch"]), (platform, arch))
                self.assertIn(f"{platform} {arch}", facts["patina:scope"])
                library = document["components"][0]
                self.assertEqual((library["name"], library["version"]), (name, f"mpv-runtime-{platform}-{RECIPE}"))
                self.assertEqual(library["hashes"], [{"alg": "SHA-256", "content": LIBRARY}])
                self.assertEqual(properties(library)[identity], inventory["build"][identity.split(":")[1].replace("-", "_")])
                self.assertIn(minimum, properties(library))
                # No toolchain is pinned for these builds, so no build input is invented.
                self.assertNotIn("formulation", document)

    def test_source_checksums_are_not_binary_hashes(self):
        for inventory in (windows_inventory(), unix_inventory("linux"), unix_inventory("macos")):
            document = self.document(inventory)
            # Exactly one bundled file, so exactly one component hash: the library's.
            self.assertEqual([item["name"] for item in document["components"] if "hashes" in item],
                             [document["components"][0]["name"]])
            mpv = named(document, "mpv")
            self.assertEqual(mpv["type"], "library")
            self.assertEqual(properties(mpv)["patina:role"], "source")
            self.assertEqual(properties(mpv)["patina:source-archive-sha256"], "1" * 64)
            self.assertEqual(mpv["externalReferences"], [{
                "type": "distribution", "url": "https://example.invalid/mpv-0.42.0.tar.gz",
                "hashes": [{"alg": "SHA-256", "content": "1" * 64}],
                "comment": "pinned source archive; the hash is of this archive"}])
            self.assertEqual(mpv["purl"], "pkg:generic/mpv@0.42.0?download_url=https://example.invalid/mpv-0.42.0.tar.gz"
                                          "&checksum=sha256:" + "1" * 64)
            self.assertEqual([item["license"]["name"] for item in mpv["licenses"]], ["LICENSE.LGPL", "Copyright"])
            self.assertIn({"name": "patina:license-file", "value": "LICENSE.LGPL sha256:" + "2" * 64}, mpv["properties"])
            # The library keeps its own hash and points at the corresponding source archive.
            library = document["components"][0]
            self.assertEqual(properties(library)["patina:source-archive-sha256"], SOURCE_ARCHIVE)
            self.assertEqual(library["externalReferences"], [{
                "type": "distribution", "url": inventory["build"]["source_url"],
                "hashes": [{"alg": "SHA-256", "content": SOURCE_ARCHIVE}],
                "comment": "corresponding source archive: sources, licenses, lock and build scripts"}])
            self.assertNotIn(LIBRARY, json.dumps(document["components"][1:]))
            # The test executable is built beside the library but never bundled.
            self.assertNotIn(TEST_TOOL, json.dumps(document))

    def test_toolchain_is_a_build_input_not_a_bundled_library(self):
        document = self.document(windows_inventory())
        self.assertNotIn("llvm-mingw", [item["name"] for item in document["components"]])
        self.assertNotIn("toolchain:llvm-mingw", json.dumps(document["dependencies"]))
        (formula,) = document["formulation"]
        (toolchain,) = formula["components"]
        self.assertEqual((toolchain["name"], toolchain["version"], toolchain["type"]),
                         ("llvm-mingw", "20261104", "application"))
        self.assertEqual(properties(toolchain)["patina:role"], "toolchain")
        self.assertNotIn("hashes", toolchain)
        self.assertEqual(properties(toolchain)["patina:toolchain-archive-sha256"], "6" * 64)
        self.assertEqual(toolchain["externalReferences"], [{
            "type": "distribution", "url": "https://example.invalid/llvm-mingw-20261104.tar.gz",
            "hashes": [{"alg": "SHA-256", "content": "6" * 64}],
            "comment": "pinned toolchain archive; the hash is of this archive"}])

    def test_source_lock_is_refused(self):
        # What the release workflow used to pass for every platform.
        lock = {"schema": 1,
                "toolchain": {"name": "llvm-mingw", "version": "20261104", "url": "https://example.invalid/llvm-mingw.tar.xz",
                              "sha256": "4" * 64, "license_files": ["LICENSE.TXT"]},
                "sources": [{"name": "zlib", "version": "1.3.1", "url": "https://example.invalid/zlib.tar.gz",
                             "sha256": "5" * 64, "license_files": ["LICENSE"]}]}
        for source in (lock, TOOLS / "mpv-runtime/sources.lock.json"):
            for platform in (None, "windows", "linux", "macos"):
                with self.subTest(source=type(source).__name__, platform=platform):
                    with self.assertRaisesRegex(ValueError, "source lock, not a packaged runtime inventory"):
                        sbom.media_runtime_sbom(source, platform=platform)

    def test_unpackaged_inventory_is_refused(self):
        # prepare.py's work inventory: the same rows, but no build has vouched for a library yet.
        inventory = windows_inventory()
        del inventory["build"]
        with self.assertRaisesRegex(ValueError, "no build record"):
            sbom.media_runtime_sbom(inventory)

    def test_inventory_of_another_platform_is_refused(self):
        cases = [(windows_inventory(), "linux", "describes the windows runtime, not the linux one"),
                 (windows_inventory(), "macos", "describes the windows runtime, not the macos one"),
                 (unix_inventory("linux"), "windows", "describes the linux runtime, not the windows one"),
                 (unix_inventory("linux"), "macos", "describes the linux runtime, not the macos one"),
                 (unix_inventory("macos"), "linux", "describes the macos runtime, not the linux one"),
                 (windows_inventory(), "android", "unknown runtime platform")]
        for inventory, platform, message in cases:
            with self.subTest(platform=platform, message=message):
                with self.assertRaisesRegex(ValueError, message):
                    sbom.media_runtime_sbom(inventory, platform=platform)

    def test_malformed_build_provenance_is_refused(self):
        def change(runtime, **values):
            inventory = windows_inventory() if runtime == "windows" else unix_inventory(runtime)
            for key, value in values.items():
                if value is None:
                    del inventory["build"][key]
                else:
                    inventory["build"][key] = value
            return inventory
        cases = [
            (change("windows", dll_sha256=None), "exactly one bundled library hash"),
            (change("windows", library_sha256=LIBRARY), "exactly one bundled library hash"),
            (change("windows", platform="windows"), "mixes the Windows and the Linux/macOS schema"),
            (change("windows", dll_sha256="B" * 64), "malformed SHA-256 dll_sha256"),
            (change("windows", dll_sha256="b" * 40), "malformed SHA-256 dll_sha256"),
            (change("windows", recipe_sha256=None), "malformed SHA-256 recipe_sha256"),
            (change("windows", tag=None), "tag is not the dependency release"),
            (change("windows", tag="mpv-runtime-" + "f" * 64), "tag is not the dependency release"),
            (change("windows", tag="mpv-runtime-linux-" + RECIPE), "tag is not the dependency release"),
            (change("windows", source_sha256=None), "malformed SHA-256 source_sha256"),
            (change("windows", source_url=None), "malformed source_url"),
            (change("windows", source_url="http://example.invalid/sources.tar.gz"), "source_url is not an HTTPS URL"),
            (change("windows", ffmpeg_sha256="unknown"), "malformed SHA-256 ffmpeg_sha256"),
            (change("linux", library_sha256=None), "exactly one bundled library hash"),
            (change("linux", platform="freebsd"), "unknown platform"),
            (change("linux", platform=None), "unknown platform"),
            (change("linux", library="libmpv.2.dylib"), "expected libmpv.so.2 for x86_64"),
            (change("linux", arch="arm64"), "expected libmpv.so.2 for x86_64"),
            (change("linux", tag="mpv-runtime-" + RECIPE), "tag is not the dependency release"),
            (change("linux", library_build_id=None), "malformed library_build_id"),
            (change("macos", library_uuid="not-a-uuid"), "malformed library_uuid"),
            (change("macos", macos_minimum=14), "malformed macos_minimum"),
        ]
        for inventory, message in cases:
            with self.subTest(message=message, build=inventory["build"]):
                with self.assertRaisesRegex(ValueError, message):
                    sbom.media_runtime_sbom(inventory)

    def test_empty_or_malformed_inventory_is_refused(self):
        def change(index=0, **values):
            inventory = windows_inventory()
            for key, value in values.items():
                if value is None:
                    del inventory["components"][index][key]
                else:
                    inventory["components"][index][key] = value
            return inventory

        def top(**values):
            return {**windows_inventory(), **values}
        cases = [
            ([], "not a JSON object"),
            ({}, "schema 1"),
            (top(schema=2), "schema 1"),
            (top(components=[]), "lists no components"),
            (top(components={"mpv": {}}), "lists no components"),
            (top(components=["mpv"]), "component 0 is not an object"),
            (top(components=sources() + [sources()[0]]), "duplicate name 'mpv'"),
            (top(components=[component("llvm-mingw", "1", "6", role="toolchain")]), "no source the library was built from"),
            (top(components=sources() + [component(name, "1", "6", role="toolchain") for name in ("one", "two")]),
             "more than one toolchain"),
            (change(name=None), "malformed or duplicate name None"),
            (change(name=""), "malformed or duplicate name ''"),
            (change(name="../mpv"), "malformed or duplicate name"),
            (change(version=None), "component mpv: missing or malformed version"),
            (change(version=""), "component mpv: missing or malformed version"),
            (change(version=42), "component mpv: missing or malformed version"),
            (change(url=None), "component mpv: missing or malformed url"),
            (change(url="http://example.invalid/mpv.tar.gz"), "component mpv: url is not an HTTPS URL"),
            (change(sha256=None), "component mpv: missing or malformed SHA-256 sha256"),
            (change(sha256="sha256:" + "1" * 64), "component mpv: missing or malformed SHA-256 sha256"),
            (change(role=None), "role must be source or toolchain"),
            (change(role="library"), "role must be source or toolchain"),
            (change(licenses=None), "component mpv: missing license files"),
            (change(licenses=[]), "component mpv: missing license files"),
            (change(licenses=["LICENSE.LGPL"]), "component mpv: missing license files"),
            (change(licenses=[{"path": "LICENSE.LGPL"}], license_files=["LICENSE.LGPL"]), "malformed SHA-256 sha256"),
            (change(licenses=[{"path": "../LICENSE", "sha256": "2" * 64}]), "unsafe license path"),
            (change(license_files=["LICENSE.LGPL"]), "license files differ"),
            (change(license_files=None), "license files differ"),
        ]
        for inventory, message in cases:
            with self.subTest(message=message):
                with self.assertRaisesRegex(ValueError, message):
                    sbom.media_runtime_sbom(inventory)
        for version in ("", None, "1.2.3 beta"):
            with self.subTest(version=version), self.assertRaisesRegex(ValueError, "release version"):
                sbom.media_runtime_sbom(windows_inventory(), version=version)

    def test_manifest_must_pin_the_same_build(self):
        for inventory in (windows_inventory(), unix_inventory("linux"), unix_inventory("macos")):
            manifest = manifest_for(inventory)
            document = self.document(inventory, manifest=manifest)
            archive, source = document["components"][0]["externalReferences"]
            self.assertEqual((archive["url"], archive["hashes"][0]["content"]), (manifest["url"], "e" * 64))
            self.assertEqual(source["hashes"][0]["content"], SOURCE_ARCHIVE)
            # The archive's checksum is a reference; the component hash stays the library's.
            self.assertEqual(document["components"][0]["hashes"], [{"alg": "SHA-256", "content": LIBRARY}])
            hash_key = "dll_sha256" if "dll_sha256" in manifest else "library_sha256"
            for key, value in ((hash_key, "f" * 64), ("tag", "mpv-runtime-" + "f" * 64), ("source_sha256", "f" * 64),
                               ("source_url", "https://example.invalid/other.tar.gz"), ("schema", 2),
                               ("sha256", "unknown"), ("url", "http://example.invalid/runtime.zip")):
                with self.subTest(key=key), self.assertRaisesRegex(ValueError, "runtime manifest"):
                    sbom.media_runtime_sbom(inventory, manifest={**manifest, key: value})
        with self.assertRaisesRegex(ValueError, "does not pin this inventory's runtime: .*platform"):
            sbom.media_runtime_sbom(unix_inventory("linux"), manifest=manifest_for(unix_inventory("macos")))
        with self.assertRaisesRegex(ValueError, "library_build_id differ"):
            sbom.media_runtime_sbom(unix_inventory("linux"),
                                    manifest={**manifest_for(unix_inventory("linux")), "library_build_id": "f" * 40})


class Packagers(unittest.TestCase):
    """The inventories the real packagers write are the ones the SBOM reads."""

    @classmethod
    def setUpClass(cls):
        # runtime.py puts tools/mpv-runtime on the path itself; both tests modules import by bare name.
        for directory in ("mpv-runtime-unix", "mpv-runtime"):
            sys.path.insert(0, str(TOOLS / directory))
        cls.addClassCleanup(lambda: [sys.path.remove(str(TOOLS / name)) for name in ("mpv-runtime-unix", "mpv-runtime")])
        import package
        import runtime
        import test_package
        import test_runtime
        cls.package, cls.runtime, cls.test_package, cls.test_runtime = package, runtime, test_package, test_runtime

    def test_platform_table_matches_the_packagers(self):
        self.assertEqual(self.package.RUNTIME, f"mpv-runtime-windows-{sbom.RUNTIMES['windows']['arch']}.zip")
        self.assertEqual({name: (entry["arch"], entry["library"]) for name, entry in self.runtime.PLATFORMS.items()},
                         {name: (sbom.RUNTIMES[name]["arch"], sbom.RUNTIMES[name]["library"]) for name in ("linux", "macos")})
        self.assertEqual(self.runtime.IDENTITY, {name: sbom.RUNTIMES[name]["identity"][0] for name in ("linux", "macos")})

    def windows(self):
        fixture = self.test_package.PackagingTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        fixture.run_package()
        with zipfile.ZipFile(fixture.output / self.package.RUNTIME) as archive:
            inventory = json.loads(archive.read("components.json"))
        return fixture.root, inventory, fixture.output / "runtime.json", fixture.build / "libmpv-2.dll"

    def unix(self, platform):
        runtime, prepare = self.runtime, self.runtime.prepare
        fixture = self.test_runtime.BuildFixture()
        fixture.platform = platform
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        selection, _ = fixture.selection()
        work, output = fixture.root / "work", fixture.root / "dist"
        runtime.write_lock(platform, work / "lock.json", selection)
        with mock.patch.object(prepare.urllib.request, "urlopen", side_effect=AssertionError("No network")):
            prepare.prepare(work / "lock.json", work)
        build = fixture.make_build()
        with contextlib.redirect_stdout(io.StringIO()):
            runtime.package(platform, work / "lock.json", work, build, output, "synthetic/project", selection)
        files = runtime.names(platform)
        with tarfile.open(output / files["runtime"]) as archive:
            inventory = json.load(archive.extractfile("components.json"))
        return fixture.root, inventory, output / files["manifest"], build / runtime.PLATFORMS[platform]["library"]

    def test_packaged_inventories_become_per_platform_documents(self):
        digest = self.runtime.prepare.digest
        for platform in ("windows", "linux", "macos"):
            with self.subTest(platform=platform):
                root, inventory, manifest, library = self.windows() if platform == "windows" else self.unix(platform)
                packaged, output = root / "packaged-components.json", root / "sbom/runtime.cdx.json"
                packaged.write_text(json.dumps(inventory), encoding="utf-8")
                with contextlib.redirect_stdout(io.StringIO()) as printed:
                    sbom.main(["media-runtime", str(packaged), str(output), "1.2.3",
                               "--platform", platform, "--manifest", str(manifest)])
                self.assertEqual(printed.getvalue().strip(), str(output))
                document = json.loads(output.read_text(encoding="utf-8"))
                self.assertEqual(properties(document["metadata"])["patina:runtime-platform"], platform)
                bundled = document["components"][0]
                self.assertEqual(bundled["name"], library.name)
                self.assertEqual(bundled["hashes"], [{"alg": "SHA-256", "content": digest(library)}])
                pinned = json.loads(manifest.read_text(encoding="utf-8"))
                self.assertEqual(bundled["version"], pinned["tag"])
                self.assertEqual([(item["url"], item["hashes"][0]["content"]) for item in bundled["externalReferences"]],
                                 [(pinned["url"], pinned["sha256"]), (pinned["source_url"], pinned["source_sha256"])])
                rows = {row["name"]: row for row in inventory["components"]}
                self.assertEqual({item["name"] for item in document["components"][1:]}, set(rows))
                for item in document["components"][1:]:
                    self.assertNotIn("hashes", item)
                    self.assertEqual(item["externalReferences"][0]["hashes"][0]["content"], rows[item["name"]]["sha256"])
                    self.assertNotEqual(rows[item["name"]]["sha256"], digest(library))
                # The positional form the release workflow uses reads the platform from the inventory.
                with contextlib.redirect_stdout(io.StringIO()):
                    sbom.main(["media-runtime", str(packaged), str(root / "sbom/plain.cdx.json"), "1.2.3"])
                plain = json.loads((root / "sbom/plain.cdx.json").read_text(encoding="utf-8"))
                self.assertEqual(plain["components"][0]["externalReferences"], bundled["externalReferences"][1:])
                # Another platform's request or pin, and the lock packaged beside the inventory, fail.
                other = "linux" if platform != "linux" else "macos"
                for arguments in (["--platform", other], ["--manifest", str(TOOLS / "mpv-runtime/runtime.json")]):
                    refused = root / "sbom/refused.cdx.json"
                    with self.assertRaises(SystemExit) as failure:
                        sbom.main(["media-runtime", str(packaged), str(refused), "1.2.3", *arguments])
                    self.assertIn("sbom.py media-runtime:", str(failure.exception))
                    self.assertFalse(refused.exists())


class CommandLine(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def run_main(self, *arguments):
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            sbom.main([str(argument) for argument in arguments])

    def refused(self, message, *arguments):
        output = self.root / "refused.cdx.json"
        with self.assertRaises(SystemExit) as failure:
            self.run_main(arguments[0], arguments[1], output, *arguments[2:])
        self.assertRegex(str(failure.exception), message)
        self.assertFalse(output.exists(), "a refused input must not leave a document behind")

    def file(self, name, content):
        path = self.root / name
        path.write_text(content if isinstance(content, str) else json.dumps(content), encoding="utf-8")
        return path

    def test_invalid_input_writes_no_document(self):
        lock = TOOLS / "mpv-runtime/sources.lock.json"
        self.refused("media-runtime: this is a source lock", "media-runtime", lock)
        self.refused("media-runtime: this is a source lock", "media-runtime", lock, "1.2.3", "--platform", "linux")
        self.refused("media-runtime: .*lists no components", "media-runtime",
                     self.file("empty.json", {**windows_inventory(), "components": []}), "1.2.3")
        self.refused("media-runtime: .*not valid JSON", "media-runtime", self.file("broken.json", '{"schema": 1,'))
        self.refused("media-runtime: .*duplicate key 'build'", "media-runtime",
                     self.file("twice.json", json.dumps(windows_inventory())[:-1] + ', "build": {}}'))
        self.refused("media-runtime: ", "media-runtime", self.root / "missing.json")
        self.refused("android-classpath: dependency report lists no dependencies", "android-classpath",
                     self.file("none.json", {"dependencies": []}), "1.2.3")
        self.refused("android-classpath: .*not valid JSON", "android-classpath", self.file("empty.txt", ""))
        self.refused("android-classpath: ", "android-classpath", self.root / "missing.json")

    def test_usage_errors(self):
        for arguments in ((), ("media-runtime",), ("media-runtime", "in.json"),
                          ("media-runtime", "in.json", "out.json", "1", "extra"),
                          ("media-runtime", "in.json", "out.json", "--platform", "android"),
                          ("android-classpath", "in.json", "out.json", "--platform", "linux"),
                          ("rust",), ("one.cdx.json", "two.cdx.json")):
            with self.subTest(arguments=arguments), self.assertRaises(SystemExit) as failure:
                self.run_main(*arguments)
            self.assertNotIn(failure.exception.code, (0, None))

    def test_android_classpath_document_is_written(self):
        report = self.file("index.json", {"dependencies": [
            {"moduleName": "org.example.synthetic:library", "moduleVersion": "1.0.0"}]})
        output = self.root / "nested/classpath.cdx.json"
        self.run_main("android-classpath", report, output, "1.2.3")
        document = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(document["metadata"]["component"]["version"], "1.2.3")
        self.assertEqual(document["components"][0]["purl"], "pkg:maven/org.example.synthetic/library@1.0.0")


class AndroidClasspath(unittest.TestCase):
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
                    "moduleUrl": None,
                    "moduleLicense": None,
                },
                {
                    "moduleName": "org.example.synthetic:dual-licensed",
                    "moduleVersion": "1.0.0-rc01+build.7",
                    "moduleLicenses": [
                        {"moduleLicense": "Synthetic License A", "moduleLicenseUrl": "https://example.invalid/a"},
                        {"moduleLicense": "Synthetic License B", "moduleLicenseUrl": None},
                    ],
                },
            ],
            "importedModules": [],
        }
        raw = sbom.android_classpath_sbom(report, version="1.0.0")
        doc = json.loads(raw)
        self.assertEqual(doc["bomFormat"], "CycloneDX")
        self.assertEqual(doc["metadata"]["component"]["name"], "patina-android-classpath")
        self.assertEqual(len(doc["components"]), 3)
        androidx = next(c for c in doc["components"] if c["name"] == "annotation")
        self.assertEqual(androidx["group"], "androidx.annotation")
        self.assertEqual(androidx["purl"], "pkg:maven/androidx.annotation/annotation@1.8.1")
        self.assertEqual(androidx["licenses"][0]["license"]["name"], "The Apache Software License, Version 2.0")
        self.assertEqual(androidx["licenses"][0]["license"]["url"], "http://www.apache.org/licenses/LICENSE-2.0.txt")
        self.assertEqual(androidx["externalReferences"][0]["type"], "website")
        plain = next(c for c in doc["components"] if c["name"] == "error_prone_annotations")
        self.assertEqual(plain["purl"], "pkg:maven/com.google.errorprone/error_prone_annotations@2.27.0")
        self.assertNotIn("licenses", plain)
        self.assertNotIn("externalReferences", plain)
        dual = next(c for c in doc["components"] if c["name"] == "dual-licensed")
        # A purl version is percent-encoded: "+" would otherwise read as a space.
        self.assertEqual(dual["purl"], "pkg:maven/org.example.synthetic/dual-licensed@1.0.0-rc01%2Bbuild.7")
        self.assertEqual(dual["version"], "1.0.0-rc01+build.7")
        self.assertEqual(dual["licenses"], [
            {"license": {"name": "Synthetic License A", "url": "https://example.invalid/a"}},
            {"license": {"name": "Synthetic License B"}}])
        for item in doc["components"]:
            self.assertRegex(item["purl"], r"^pkg:maven/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+@[A-Za-z0-9_.%-]+$")
            self.assertEqual(item["bom-ref"], item["purl"])

    def test_empty_or_malformed_report_is_refused(self):
        valid = {"moduleName": "org.example.synthetic:library", "moduleVersion": "1.0.0"}
        cases = [
            ([], "not a JSON object"),
            ({}, "lists no dependencies"),
            ({"dependencies": []}, "lists no dependencies"),
            ({"dependencies": None}, "lists no dependencies"),
            ({"dependencies": {"org.example.synthetic:library": "1.0.0"}}, "lists no dependencies"),
            ({"dependencies": ["org.example.synthetic:library:1.0.0"]}, "dependency 0 is not an object"),
            ({"dependencies": [valid, valid]}, "listed twice at 1.0.0"),
        ]
        for name in (None, "", 7, "library", ":library", "org.example.synthetic:", "org.example.synthetic:library:1.0.0",
                     "org.example synthetic:library", "org.example.synthetic/library:core", "org.example.synthetic:lib@1"):
            cases.append(({"dependencies": [{**valid, "moduleName": name}]}, "is not a group:name Maven coordinate"))
        for version in (None, "", 1, " 1.0.0", "1.0 beta", "[1.0,2.0)", "1.0/2", "-1", "+"):
            cases.append(({"dependencies": [{**valid, "moduleVersion": version}]}, "missing or malformed moduleVersion"))
        for version in ("unspecified", "1.+", "latest.release", "1.0.0-SNAPSHOT"):
            cases.append(({"dependencies": [{**valid, "moduleVersion": version}]}, "is not a fixed release version"))
        for key in ("moduleUrl", "moduleLicense", "moduleLicenseUrl"):
            cases.append(({"dependencies": [{**valid, key: ["synthetic"]}]}, f"malformed {key}"))
        cases.append(({"dependencies": [{**valid, "moduleLicenses": "Synthetic License"}]}, "malformed moduleLicenses"))
        cases.append(({"dependencies": [valid, {"moduleVersion": "1.0.0"}]}, "dependency 1: moduleName None"))
        for report, message in cases:
            with self.subTest(report=report, message=message):
                with self.assertRaisesRegex(ValueError, message):
                    sbom.android_classpath_sbom(copy.deepcopy(report), version="1.0.0")
        with self.assertRaisesRegex(ValueError, "release version"):
            sbom.android_classpath_sbom({"dependencies": [valid]}, version="")


if __name__ == "__main__":
    unittest.main()
