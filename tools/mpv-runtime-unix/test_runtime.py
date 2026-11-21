"""Synthetic regression coverage; no network, compiler, native tools or real streams."""

import contextlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock

import runtime
from runtime import prepare

FFMPEG_ON = runtime.EXPECTED["linux"]["ffmpeg"]
MPV_ON = runtime.EXPECTED["linux"]["mpv"]

READELF = """
Dynamic section at offset 0x1 contains 6 entries:
  Tag        Type                         Name/Value
 0x0000000000000001 (NEEDED)             Shared library: [libpulse.so.0]
 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]
 0x000000000000000e (SONAME)             Library soname: [libmpv.so.2]
"""
NM = """
00000000000a1000 T mpv_create
00000000000a2000 T mpv_render_context_create
"""
OBJDUMP = """
0000000000000000      DF *UND*  0000000000000000 (GLIBC_2.34) pthread_create
0000000000000000      DF *UND*  0000000000000000 (GLIBC_2.2.5) memcpy
0000000000000000      DF *UND*  0000000000000000 (GLIBCXX_3.4.32) _ZSt4cout
"""
NOTES = "  GNU                  0x00000014	NT_GNU_BUILD_ID (unique build ID bitstring)\n    Build ID: 0123456789abcdef0123456789abcdef01234567\n"
OTOOL_L = "/build/libmpv.2.dylib:\n\t@rpath/libmpv.2.dylib (compatibility version 2.0.0, current version 2.5.0)\n\t/System/Library/Frameworks/CoreAudio.framework/Versions/A/CoreAudio (compatibility version 1.0.0)\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0)\n"
OTOOL_LOAD = ("Load command 8\n     cmd LC_UUID\n cmdsize 24\n    uuid 0A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F9\n"
              "Load command 9\n      cmd LC_BUILD_VERSION\n  cmdsize 32\n platform 1\n    minos 14.0\n      sdk 15.5\n")


def defines(on, off=()):
    return "".join(f"#define {key} 1\n" for key in on) + "".join(f"#define {key} 0\n" for key in off)


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def archive(self, name, members):
        path = self.root / f"{name}.tar.gz"
        with tarfile.open(path, "w:gz") as archive:
            for member, content in members:
                entry = tarfile.TarInfo(f"{name}-1/{member}")
                entry.size = len(content)
                archive.addfile(entry, io.BytesIO(content))
        return path

    def item(self, name):
        source = self.archive(name, [("COPYING", f"{name} synthetic license\n".encode()), ("main.c", b"/* fixture */\n")])
        return {"name": name, "version": "1", "url": f"https://example.invalid/{name}.tar.gz",
                "sha256": prepare.digest(source), "license_files": ["COPYING"]}, source

    def selection(self, extra_names=("extra",)):
        """A base lock, a platform selection on top of it, and a filled download cache."""
        cache = self.root / "work/cache"
        cache.mkdir(parents=True)
        items = {}
        for name in ("shared", "windows-only", *extra_names):
            items[name], source = self.item(name)
            (cache / items[name]["sha256"]).write_bytes(source.read_bytes())
        base = self.root / "base.lock.json"
        base.write_text(json.dumps({"schema": 1, "sources": [items["shared"], items["windows-only"]]}))
        selection = self.root / "sources.json"
        selection.write_text(json.dumps({"schema": 1, "base": "base.lock.json", "platforms": {
            "linux": {"shared": ["shared"], "sources": [items[name] for name in extra_names]},
            "macos": {"shared": ["shared"], "sources": []}}}))
        return selection, items


class LockTests(Fixture):
    def test_shared_entries_are_copied_from_the_base_lock(self):
        selection, items = self.selection()
        lock = runtime.resolve_lock("linux", selection)
        self.assertEqual(lock["sources"], [items["shared"], items["extra"]])
        self.assertEqual(runtime.resolve_lock("macos", selection)["sources"], [items["shared"]])

    def test_a_second_pin_for_a_shared_component_is_refused(self):
        selection, items = self.selection()
        data = json.loads(selection.read_text())
        data["platforms"]["linux"]["sources"].append(dict(items["shared"], version="2"))
        selection.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "duplicates a shared pin"):
            runtime.resolve_lock("linux", selection)

    def test_unknown_shared_component_and_unpinned_source_are_refused(self):
        selection, items = self.selection()
        data = json.loads(selection.read_text())
        data["platforms"]["linux"]["shared"].append("missing")
        selection.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "not in the base lock"):
            runtime.resolve_lock("linux", selection)
        data["platforms"]["linux"]["shared"].pop()
        data["platforms"]["linux"]["sources"][0]["sha256"] = "not-a-checksum"
        selection.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "SHA-256"):
            runtime.resolve_lock("linux", selection)

    def test_committed_selection_resolves_for_both_platforms(self):
        for platform in runtime.PLATFORMS:
            names = [item["name"] for item in runtime.resolve_lock(platform)["sources"]]
            self.assertIn("ffmpeg", names)
            self.assertIn("mpv", names)
            self.assertNotIn("llvm-mingw-scripts", names)

    def test_recipe_digest_is_per_platform_and_excludes_tests(self):
        lock = runtime.resolve_lock("linux")
        self.assertEqual(runtime.recipe_digest("linux", lock), runtime.recipe_digest("linux", lock))
        self.assertNotEqual(runtime.recipe_digest("linux", lock), runtime.recipe_digest("macos", lock))
        names = [name for name, _ in runtime.recipe_files("linux")]
        self.assertIn("linux/build.sh", names)
        self.assertIn("base/prepare.py", names)
        self.assertFalse(any(name.startswith(("macos/", "test_")) for name in names))


class EvidenceTests(unittest.TestCase):
    def test_native_guard_requires_its_protocol_and_the_lavf_backend(self):
        configured = runtime.config_defines(defines(MPV_ON, runtime.REQUIRED_OFF["mpv"]))
        self.assertEqual(runtime.check_features("linux", "mpv", configured), [])
        configured["HAVE_LIBCURL"] = "1"
        self.assertEqual(runtime.check_features("linux", "mpv", configured),
                         ["HAVE_LIBCURL: expected 0, got 1"])
        configured = runtime.config_defines(defines(FFMPEG_ON, runtime.REQUIRED_OFF["ffmpeg"]))
        del configured["CONFIG_PATINA_RELAY_GUARD_V1_PROTOCOL"]
        self.assertEqual(runtime.check_features("linux", "ffmpeg", configured),
                         ["CONFIG_PATINA_RELAY_GUARD_V1_PROTOCOL: expected 1, got missing"])

    def test_license_switches_must_be_off(self):
        on = runtime.config_defines(defines(FFMPEG_ON, runtime.REQUIRED_OFF["ffmpeg"]))
        self.assertEqual(runtime.check_features("linux", "ffmpeg", on), [])
        gpl = runtime.config_defines(defines(FFMPEG_ON + ["CONFIG_GPL"], ["CONFIG_VERSION3", "CONFIG_NONFREE"]))
        self.assertEqual(runtime.check_features("linux", "ffmpeg", gpl), ["CONFIG_GPL: expected 0, got 1"])
        missing = runtime.config_defines(defines(FFMPEG_ON[1:], runtime.REQUIRED_OFF["ffmpeg"]))
        self.assertEqual(runtime.check_features("linux", "ffmpeg", missing), [f"{FFMPEG_ON[0]}: expected 1, got missing"])

    def test_elf_parsers(self):
        self.assertEqual(runtime.elf_dynamic(READELF), (["libpulse.so.0", "libc.so.6"], "libmpv.so.2", False))
        self.assertTrue(runtime.elf_dynamic(READELF + " 0x1d (RUNPATH)            Library runpath: [/work]\n")[2])
        self.assertEqual(runtime.elf_exports(NM), ["mpv_create", "mpv_render_context_create"])
        self.assertEqual(runtime.glibc_minimum(OBJDUMP), (2, 34))
        with self.assertRaises(ValueError):
            runtime.glibc_minimum("no versions")

    def test_macho_parsers(self):
        self.assertEqual(runtime.macho_references(OTOOL_L)[0], "@rpath/libmpv.2.dylib")
        self.assertEqual(runtime.macho_minimum(OTOOL_LOAD), (14, 0))
        self.assertEqual(runtime.macho_rpaths("Load command 3\n          cmd LC_RPATH\n      cmdsize 32\n         path /opt/homebrew/lib (offset 12)\n"), ["/opt/homebrew/lib"])


class BuildFixture(Fixture):
    platform = "linux"

    def make_build(self, **overrides):
        build = self.root / "build"
        logs = build / "logs"
        (build / "include/mpv").mkdir(parents=True)
        logs.mkdir()
        library = runtime.PLATFORMS[self.platform]["library"]
        (build / library).write_bytes(b"\x7fELF synthetic libmpv")
        (build / "ffmpeg").write_bytes(b"\x7fELF synthetic ffmpeg")
        for header in runtime.HEADERS:
            (build / "include/mpv" / header).write_text("/* synthetic */\n")
        evidence = {
            "ffmpeg-config.h": defines(runtime.EXPECTED[self.platform]["ffmpeg"], runtime.REQUIRED_OFF["ffmpeg"])
            + '#define FFMPEG_LICENSE "LGPL version 2.1 or later"\n',
            "ffmpeg-config_components.h": "",
            "mpv-config.h": defines(runtime.EXPECTED[self.platform]["mpv"], runtime.REQUIRED_OFF["mpv"]),
            "smoke.txt": "PASS generated.ts: {}\nPASS generated.m3u8: {}\n",
        }
        if self.platform == "linux":
            evidence.update({"libmpv-dynamic.txt": READELF, "ffmpeg-dynamic.txt": READELF.replace("[libmpv.so.2]", "[x]"),
                             "libmpv-exports.txt": NM, "libmpv-symbols.txt": OBJDUMP, "libmpv-notes.txt": NOTES})
        else:
            evidence.update({f"{library}-otool-libraries.txt": OTOOL_L, f"{library}-otool-loads.txt": OTOOL_LOAD,
                             "ffmpeg-otool-libraries.txt": "/build/ffmpeg:\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0)\n",
                             "ffmpeg-otool-loads.txt": OTOOL_LOAD, "libmpv-exports.txt": "0000000000001000 T _mpv_create\n"})
        evidence.update(overrides)
        for name, text in evidence.items():
            (logs / name).write_text(text)
        (logs / "binaries.sha256").write_text("".join(
            f"{prepare.digest(build / name)}  {name}\n" for name in (library, "ffmpeg")))
        return build


class ValidateTests(BuildFixture):
    def test_linux_evidence_passes_and_records_facts(self):
        facts = runtime.validate_build("linux", self.make_build())
        self.assertEqual(facts["glibc_minimum"], "2.34")
        self.assertEqual(facts["library_build_id"], "0123456789abcdef0123456789abcdef01234567")

    def test_linux_violations_fail_loudly(self):
        cases = {
            "undeclared library libx264.so.164": {"libmpv-dynamic.txt": READELF + " 0x1 (NEEDED) Shared library: [libx264.so.164]\n"},
            "beyond the mpv API": {"libmpv-exports.txt": NM + "00000000000a3000 T av_malloc\n"},
            "newer than 2.39": {"libmpv-symbols.txt": OBJDUMP + " (GLIBC_2.41) memcpy\n"},
            "CONFIG_NONFREE: expected 0": {"ffmpeg-config_components.h": "#define CONFIG_NONFREE 1\n"},
            "HAVE_GPL: expected 0": {"mpv-config.h": defines(MPV_ON) + "#define HAVE_GPL 1\n"},
            "playback smoke test": {"smoke.txt": "PASS generated.ts: {}\n"},
            "FFmpeg license is GPL version 2 or later": {"ffmpeg-config.h": defines(FFMPEG_ON, runtime.REQUIRED_OFF["ffmpeg"])
                                                         + '#define FFMPEG_LICENSE "GPL version 2 or later"\n'},
            "RPATH/RUNPATH": {"libmpv-dynamic.txt": READELF + " 0x1d (RUNPATH) Library runpath: [/work/prefix/lib]\n"},
        }
        for message, override in cases.items():
            with self.subTest(message):
                self.tearDown_build()
                with self.assertRaisesRegex(ValueError, message):
                    runtime.validate_build("linux", self.make_build(**override))

    def test_tampered_binary_is_refused(self):
        build = self.make_build()
        (build / "ffmpeg").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "checksum mismatch: ffmpeg"):
            runtime.validate_build("linux", build)

    def tearDown_build(self):
        import shutil
        shutil.rmtree(self.root / "build", ignore_errors=True)


class MacValidateTests(BuildFixture):
    platform = "macos"

    def test_system_only_references_pass(self):
        facts = runtime.validate_build("macos", self.make_build())
        self.assertEqual(facts["macos_minimum"], "14.0")
        self.assertEqual(facts["library_uuid"], "0A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F9")

    def test_homebrew_reference_newer_os_and_rpath_fail(self):
        brew = OTOOL_L + "\t/opt/homebrew/opt/ffmpeg/lib/libavcodec.62.dylib (compatibility version 62.0.0)\n"
        with self.assertRaisesRegex(ValueError, "non-system library: /opt/homebrew"):
            runtime.validate_build("macos", self.make_build(**{"libmpv.2.dylib-otool-libraries.txt": brew}))
        import shutil
        shutil.rmtree(self.root / "build")
        newer = OTOOL_LOAD.replace("minos 14.0", "minos 15.0") + "Load command 10\n          cmd LC_RPATH\n      cmdsize 32\n         path /Users/runner/prefix/lib (offset 12)\n"
        with self.assertRaisesRegex(ValueError, "(?s)macOS 15.0.*LC_RPATH"):
            runtime.validate_build("macos", self.make_build(**{"libmpv.2.dylib-otool-loads.txt": newer}))


class PackageTests(BuildFixture):
    def setUp(self):
        super().setUp()
        self.selection_path, _ = self.selection()
        self.lock = self.root / "work/lock.json"
        runtime.write_lock("linux", self.lock, self.selection_path)
        self.work = self.root / "work"
        with mock.patch.object(prepare.urllib.request, "urlopen", side_effect=AssertionError("No network")):
            prepare.prepare(self.lock, self.work)
        self.build = self.make_build()
        self.output = self.root / "dist"
        with contextlib.redirect_stdout(io.StringIO()):
            self.manifest = runtime.package("linux", self.lock, self.work, self.build, self.output,
                                            "synthetic/project", self.selection_path)
        self.files = runtime.names("linux")

    def test_runtime_sources_and_manifest_agree(self):
        manifest = json.loads((self.output / self.files["manifest"]).read_text())
        self.assertEqual(manifest, self.manifest)
        self.assertRegex(manifest["tag"], r"^mpv-runtime-linux-[0-9a-f]{64}$")
        self.assertEqual(manifest["sha256"], prepare.digest(self.output / self.files["runtime"]))
        self.assertEqual(manifest["source_sha256"], prepare.digest(self.output / self.files["sources"]))
        self.assertTrue(manifest["url"].startswith(f"https://github.com/synthetic/project/releases/download/{manifest['tag']}/"))
        with tarfile.open(self.output / self.files["runtime"]) as archive:
            names = archive.getnames()
            self.assertNotIn("bin/ffmpeg", names)
            self.assertIn("licenses/shared/COPYING", names)
            self.assertIn("licenses/extra/COPYING", names)
            self.assertNotIn("licenses/windows-only/COPYING", names)
            self.assertIn("build-config/ffmpeg-config.h", names)
            self.assertIn(manifest["source_url"], archive.extractfile("SOURCE.txt").read().decode())
        with tarfile.open(self.output / self.files["tools"]) as archive:
            self.assertEqual(archive.getnames(), ["bin/ffmpeg"])
        sums = (self.output / "SHA256SUMS").read_text().splitlines()
        self.assertEqual(len(sums), 3)

    def test_packaging_is_deterministic(self):
        first = prepare.digest(self.output / self.files["runtime"])
        with contextlib.redirect_stdout(io.StringIO()):
            runtime.package("linux", self.lock, self.work, self.build, self.root / "again", "synthetic/project", self.selection_path)
        self.assertEqual(first, prepare.digest(self.root / "again" / self.files["runtime"]))

    def test_source_bundle_restores_offline(self):
        restored = self.root / "restored"
        with mock.patch.object(prepare.urllib.request, "urlopen", side_effect=AssertionError("No network")):
            prepare.prepare(self.lock, restored, self.output / self.files["sources"])
        self.assertTrue((restored / "sources/extra/main.c").is_file())

    def test_fetch_verifies_and_unpacks(self):
        destination = self.root / "runtime"
        with contextlib.redirect_stdout(io.StringIO()), \
                mock.patch.object(prepare.urllib.request, "urlopen", side_effect=AssertionError("No network")):
            runtime.fetch("linux", self.output / self.files["manifest"], destination, self.output / self.files["runtime"])
        self.assertEqual((destination / "lib/libmpv.so.2").read_bytes(), (self.build / "libmpv.so.2").read_bytes())
        self.assertEqual((destination / "lib/libmpv.so").readlink(), Path("libmpv.so.2"))
        self.assertIn(f"prefix={destination.resolve()}", (destination / "lib/pkgconfig/mpv.pc").read_text())

    def test_notices_and_bundle_check(self):
        destination = self.root / "runtime"
        with contextlib.redirect_stdout(io.StringIO()):
            runtime.fetch("linux", self.output / self.files["manifest"], destination, self.output / self.files["runtime"])
        notices = self.root / "package/notices/mpv-runtime"
        runtime.install_notices(destination, notices)
        self.assertIn(self.manifest["source_url"], (notices / "SOURCE.txt").read_text())
        self.assertTrue((notices / "licenses/extra/COPYING").is_file())
        self.assertTrue((notices / "build-config/ffmpeg-config.h").is_file())
        libraries = self.root / "package/lib"
        libraries.mkdir()
        (libraries / "libmpv.so.2").write_bytes(b"patched by a packaging tool")
        (libraries / "libgtk-4.so.1").write_bytes(b"")
        build_id = self.manifest["library_build_id"]
        runtime.check_bundle("linux", destination, libraries, identity=lambda platform, path: build_id)
        with self.assertRaisesRegex(ValueError, "library_build_id differs"):
            runtime.check_bundle("linux", destination, libraries, identity=lambda platform, path: "0" * 40)
        (libraries / "libavcodec.so.61").write_bytes(b"")
        with self.assertRaisesRegex(ValueError, "libavcodec.so.61"):
            runtime.check_bundle("linux", destination, libraries, identity=lambda platform, path: build_id)

    def test_fetch_rejects_a_modified_archive_and_a_missing_pin(self):
        archive = self.root / "modified.tar.gz"
        archive.write_bytes((self.output / self.files["runtime"]).read_bytes() + b"\0")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            runtime.fetch("linux", self.output / self.files["manifest"], self.root / "runtime", archive)
        with self.assertRaisesRegex(SystemExit, "No linux libmpv runtime is pinned"):
            runtime.fetch("linux", self.root / "absent.json", self.root / "runtime")

    def test_fetch_rejects_a_manifest_for_another_platform(self):
        manifest = json.loads((self.output / self.files["manifest"]).read_text())
        manifest["platform"] = "macos"
        path = self.root / "other.json"
        path.write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, "Invalid linux runtime manifest"):
            runtime.fetch("linux", path, self.root / "runtime", self.output / self.files["runtime"])

    def test_modified_lock_rejected(self):
        data = json.loads(self.lock.read_text())
        data["sources"][0]["version"] = "changed"
        self.lock.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "differs"):
            runtime.package("linux", self.lock, self.work, self.build, self.root / "x", "synthetic/project", self.selection_path)


if __name__ == "__main__":
    unittest.main()
