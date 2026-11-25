#!/usr/bin/env python3
"""Disposable profile ownership, path boundaries and interruption cleanup."""
import importlib.util
import json
from pathlib import Path
import tempfile
import subprocess
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("avd", Path(__file__).with_name("disposable-avd.py"))
a = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(a)


class Profiles(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        # macOS's default temporary directory passes through /var -> /private/var.
        self.root = Path(temporary.name).resolve()
        self.source = self.root / ".android/avd/tab36.avd"
        self.source.mkdir(parents=True)
        (self.source / "config.ini").write_text(f"AvdId=tab36\ndisk.dataPartition.path={self.source}/userdata-qemu.img\n")
        for name in ("userdata-qemu.img", "cache.img", "encryptionkey.img"):
            (self.source / name).write_bytes(b"synthetic preserved image")
        self.before = {p.name: p.read_bytes() for p in self.source.iterdir()}
        self.scratch = self.root / "scratch"

    def assert_original(self):
        self.assertEqual({p.name: p.read_bytes() for p in self.source.iterdir()}, self.before)

    def test_smoke_hold_is_bounded_and_extends_the_budget(self):
        self.assertEqual(a.smoke_timeout({}), a.SMOKE_BUDGET)
        self.assertEqual(a.smoke_timeout({"IPTV_SMOKE_HOLD": "480"}), a.SMOKE_BUDGET + 480)
        self.assertEqual(a.smoke_timeout({"IPTV_SMOKE_HOLD": "1800"}), a.SMOKE_BUDGET + 1800)
        for bad in ("0", "1801", "18000", "-5", "1e3", "99999999999999999999", " 60", "60s"):
            with self.assertRaises(ValueError, msg=bad):
                a.smoke_timeout({"IPTV_SMOKE_HOLD": bad})

    def test_production_apk_only_allowed_explicitly_and_without_debugging(self):
        a.validate_apk(a.PACKAGE, "true", False)
        a.validate_apk(a.RELEASE_PACKAGE, "false", True)
        for package, debug, release in ((a.RELEASE_PACKAGE, "false", False),
                                         (a.PACKAGE, "false", True),
                                         (a.RELEASE_PACKAGE, "true", True)):
            with self.assertRaises(ValueError):
                a.validate_apk(package, debug, release)

    def test_complete_copy_rewrites_only_copy_and_cleanup_preserves_original(self):
        root, token, name = a.prepare(self.source, self.scratch)
        profile = root / "avd" / (name + ".avd")
        for image in ("userdata-qemu.img", "cache.img", "encryptionkey.img"):
            self.assertEqual((profile / image).read_bytes(), self.before[image])
        self.assertIn(str(profile), (profile / "config.ini").read_text())
        self.assertNotIn(str(self.source), (profile / "config.ini").read_text())
        self.assert_original()
        a.cleanup(root, self.scratch, token)
        self.assertFalse(root.exists())
        self.assert_original()

    def test_original_profile_and_parent_cannot_be_used_as_scratch(self):
        for scratch in (self.source, self.source.parent, self.source / "scratch", self.root):
            with self.subTest(scratch=scratch), self.assertRaises(ValueError):
                a.prepare(self.source, scratch)
        self.assert_original()

    def test_cleanup_rejects_original_markerless_wrong_token_and_escape(self):
        with self.assertRaises(ValueError):
            a.cleanup(self.source, self.source.parent, "anything")
        owned, token, _ = a.prepare(self.source, self.scratch)
        for root, scratch, supplied in ((owned, self.scratch, "wrong"),
                                        (owned, self.root, token), (self.source, self.scratch, token)):
            with self.assertRaises(ValueError):
                a.cleanup(root, scratch, supplied)
        marker = owned / a.MARKER
        saved = marker.read_text()
        marker.unlink()
        with self.assertRaises(ValueError):
            a.cleanup(owned, self.scratch, token)
        marker.write_text(saved)
        a.cleanup(owned, self.scratch, token)
        self.assert_original()

    def test_symlink_roots_sources_and_markers_are_rejected(self):
        link = self.root / "linked-scratch"
        link.symlink_to(self.root, target_is_directory=True)
        with self.assertRaises(ValueError):
            a.prepare(self.source, link / "scratch")
        outside = self.root / "outside"
        outside.write_text("synthetic")
        source_link = self.source / "external.img"
        source_link.symlink_to(outside)
        with self.assertRaises(ValueError):
            a.prepare(self.source, self.scratch)
        source_link.unlink()
        owned, token, _ = a.prepare(self.source, self.scratch)
        marker = owned / a.MARKER
        saved = marker.read_text()
        marker.unlink()
        marker.symlink_to(outside)
        with self.assertRaises(ValueError):
            a.cleanup(owned, self.scratch, token)
        marker.unlink()
        marker.write_text(saved)
        a.cleanup(owned, self.scratch, token)
        self.assert_original()

    def test_interrupted_copy_removes_only_owned_partial_copy(self):
        def interrupted(source, target):
            target.mkdir()
            (target / "partial").write_text("generated")
            raise KeyboardInterrupt()
        with self.assertRaises(KeyboardInterrupt):
            a.prepare(self.source, self.scratch, copier=interrupted)
        self.assertEqual(list(self.scratch.iterdir()), [])
        self.assert_original()

    def test_external_writable_path_fails_before_emulator_start(self):
        (self.source / "config.ini").write_text("disk.cachePartition.path=/synthetic/original/cache.img\n")
        with self.assertRaisesRegex(ValueError, "external writable"):
            a.prepare(self.source, self.scratch)
        self.assertEqual(list(self.scratch.iterdir()), [])

    def test_source_prefixed_parent_escape_is_not_remapped_outside_copy(self):
        (self.source / "config.ini").write_text(f"disk.cachePartition.path={self.source}/../../../personal.img\n")
        with self.assertRaisesRegex(ValueError, "external writable"):
            a.prepare(self.source, self.scratch)
        self.assertEqual(list(self.scratch.iterdir()), [])

    def test_marker_cannot_authorize_another_scratch_root(self):
        root, token, _ = a.prepare(self.source, self.scratch)
        marker = root / a.MARKER
        data = json.loads(marker.read_text())
        data["scratch"] = str(self.root)
        marker.write_text(json.dumps(data))
        with self.assertRaises(ValueError):
            a.cleanup(root, self.scratch, token)
        data["scratch"] = str(self.scratch)
        marker.write_text(json.dumps(data))
        a.cleanup(root, self.scratch, token)

    def test_qcow2_is_flattened_without_mutating_source_backing(self):
        profile = self.root / "copy"
        profile.mkdir()
        image = profile / "userdata.img"
        image.write_bytes(b"QFI\xfbsynthetic")
        original = self.source / "userdata-qemu.img"
        def runner(args, **kwargs):
            if args[1] == "convert":
                Path(args[-1]).write_bytes(b"QFI\xfbstandalone")
                return subprocess.CompletedProcess(args, 0, "")
            filename = Path(args[-1])
            chain = [{"filename": str(filename)}]
            if filename == image:
                chain[0]["backing-filename"] = str(original)
                chain.append({"filename": str(original)})
            return subprocess.CompletedProcess(args, 0, json.dumps(chain))
        a.flatten_images(profile, self.source, None, "synthetic-qemu-img", runner)
        self.assertEqual(image.read_bytes(), b"QFI\xfbstandalone")
        self.assert_original()

    def test_external_qcow2_backing_is_rejected_without_conversion(self):
        profile = self.root / "copy"
        profile.mkdir()
        (profile / "disk.qcow2").write_bytes(b"QFI\xfbsynthetic")
        calls = []
        def runner(args, **kwargs):
            calls.append(args)
            return subprocess.CompletedProcess(args, 0, json.dumps([
                {"filename": str(profile / "disk.qcow2")}, {"filename": "/synthetic/personal/disk.img"}]))
        with self.assertRaisesRegex(ValueError, "backing image"):
            a.flatten_images(profile, self.source, None, "synthetic-qemu-img", runner)
        self.assertEqual(len(calls), 1)
        self.assert_original()

    def test_adb_requires_serial_and_never_implicitly_selects_a_device(self):
        with patch.object(a.subprocess, "run") as run:
            run.return_value.stdout = "1\n"
            self.assertEqual(a.adb("emulator-5580", "shell", "getprop", "sys.boot_completed"), "1")
            self.assertEqual(run.call_args.args[0][:3], ["adb", "-s", "emulator-5580"])
            for serial in ("", "physical-serial", "emulator-5580;anything"):
                with self.assertRaises(ValueError):
                    a.adb(serial, "shell", "true")

    def test_fresh_muted_boot_is_confined_to_marker_owned_copy(self):
        root, token, name = a.prepare(self.source, self.scratch)
        command, env = a.boot_config(root, self.scratch, token, name, "emulator", 5580)
        self.assertEqual(command[command.index("-avd") + 1], name)
        for flag in ("-wipe-data", "-no-snapshot", "-no-audio"):
            self.assertIn(flag, command)
        modern, _ = a.boot_config(root, self.scratch, token, name, "emulator", 5580, "swiftshader")
        self.assertEqual(modern[modern.index("-gpu") + 1], "swiftshader")
        for key in ("ANDROID_AVD_HOME", "ANDROID_USER_HOME", "ANDROID_EMULATOR_HOME"):
            self.assertTrue(Path(env[key]).is_relative_to(root))
        for boot_root, boot_token, boot_name in ((self.source, token, "tab36"),
                                                  (root, "wrong", name), (root, token, "tab36")):
            with self.assertRaises(ValueError):
                a.boot_config(boot_root, self.scratch, boot_token, boot_name, "emulator", 5580)
        self.assert_original()
        a.cleanup(root, self.scratch, token)


if __name__ == "__main__":
    unittest.main()
