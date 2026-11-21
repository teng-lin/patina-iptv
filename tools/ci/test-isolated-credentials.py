#!/usr/bin/env python3
"""The credential harness never inherits the login session's bus or keyring."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("credentials", Path(__file__).with_name("isolated-credentials.py"))
c = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(c)


class Isolation(unittest.TestCase):
    def test_the_named_test_is_the_brokers_test_at_its_module_path(self):
        *modules, tests, name = c.TEST.split("::")
        self.assertEqual(tests, "tests")
        source = c.ROOT / "crates/iptv-app/src" / ("/".join(modules) + ".rs")
        text = source.read_text(encoding="utf-8")
        self.assertIn("mod tests", text)
        self.assertIn(f"fn {name}()", text)

    def test_environment_preserves_toolchain_but_replaces_profile_and_bus(self):
        original = {"HOME": "/synthetic/login", "DBUS_SESSION_BUS_ADDRESS": "synthetic-login-bus",
                    "GNOME_KEYRING_CONTROL": "/synthetic/login-keyring", "CARGO_HOME": "/synthetic/cache",
                    "DISPLAY": ":42", "WAYLAND_DISPLAY": "wayland-42", "XAUTHORITY": "/synthetic/authority",
                    "PATH": "/synthetic/bin", "XDG_RUNTIME_DIR": "/synthetic/session"}
        env = c.environment(Path("/synthetic/disposable"), original)
        self.assertNotIn("DBUS_SESSION_BUS_ADDRESS", env)
        self.assertNotIn("GNOME_KEYRING_CONTROL", env)
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY"):
            self.assertNotIn(key, env)
        self.assertEqual(env["HOME"], "/synthetic/disposable")
        self.assertEqual(env["XDG_RUNTIME_DIR"], "/synthetic/disposable/run")
        self.assertEqual(env["CARGO_HOME"], original["CARGO_HOME"])
        self.assertEqual(original["HOME"], "/synthetic/login")

    def test_direct_inner_invocation_refuses_login_profile_before_spawning(self):
        with patch.dict(c.os.environ, {}, clear=True), patch.object(c.subprocess, "Popen") as spawn:
            with self.assertRaisesRegex(RuntimeError, "disposable"):
                c.inside("/synthetic/test")
            spawn.assert_not_called()


if __name__ == "__main__":
    unittest.main()
