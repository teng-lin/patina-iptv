#!/usr/bin/env python3
"""Run production broker tests in a fresh D-Bus session and disposable Secret Service.

Compilation precedes isolation, retaining the caller's Cargo/toolchain cache. Only
our test binary and a foreground keyring daemon see the empty temporary home. No
login-session bus or keyring is accessed. Requires gnome-keyring and dbus-run-session.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
TEST = "host::native::credential_key::tests::isolated_secret_service_persists_key_and_reopens_catalog"


def environment(root, inherited):
    env = {key: value for key, value in inherited.items()
           if not key.startswith(("DBUS_", "GNOME_KEYRING_")) and key not in
           ("DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "SESSION_MANAGER", "XDG_CURRENT_DESKTOP",
            "DESKTOP_SESSION", "XDG_SESSION_TYPE", "XDG_SESSION_ID", "SSH_ASKPASS", "GPG_TTY")}
    env.update(IPTV_DISPOSABLE_BROKER_ROOT=str(root), HOME=str(root), XDG_CONFIG_HOME=str(root / "config"),
               XDG_DATA_HOME=str(root / "data"), XDG_CACHE_HOME=str(root / "cache"),
               XDG_RUNTIME_DIR=str(root / "run"), IPTV_TEST_ISOLATED_BROKER="1")
    return env


def inside(binary):
    # Only dbus-run-session creates this address; the parent removes inherited ones.
    root = Path(os.environ.get("IPTV_DISPOSABLE_BROKER_ROOT", "/"))
    if (not (root / ".patina-broker").is_file() or root.is_symlink()
            or Path.home() != root or root == Path("/")
            or os.environ.get("XDG_RUNTIME_DIR") != str(root / "run")):
        raise RuntimeError("refusing a broker outside the disposable profile")
    if not os.environ.get("DBUS_SESSION_BUS_ADDRESS"):
        raise RuntimeError("an isolated D-Bus session is required")
    daemon = subprocess.Popen(["gnome-keyring-daemon", "--foreground", "--unlock", "--components=secrets"],
                              stdin=subprocess.PIPE, start_new_session=True)
    try:
        daemon.stdin.write(b"synthetic-disposable-broker\n")
        daemon.stdin.close()
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if daemon.poll() is not None:
                raise RuntimeError("disposable Secret Service exited")
            result = subprocess.run(["gdbus", "call", "--session", "--dest", "org.freedesktop.DBus",
                                     "--object-path", "/org/freedesktop/DBus", "--method",
                                     "org.freedesktop.DBus.NameHasOwner", "org.freedesktop.secrets"],
                                    capture_output=True, text=True, timeout=5, check=True)
            if "true" in result.stdout:
                break
            time.sleep(0.1)
        else:
            raise RuntimeError("disposable Secret Service did not register")
        subprocess.run([binary, "--ignored", "--exact", TEST, "--nocapture"], check=True, timeout=180)
    finally:
        if daemon.poll() is None:
            os.killpg(daemon.pid, signal.SIGTERM)
            try:
                daemon.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(daemon.pid, signal.SIGKILL)
                daemon.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inside", metavar="TEST_BINARY", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.inside:
        inside(args.inside)
        return
    result = subprocess.run(["cargo", "test", "--locked", "-p", "iptv-app", "--lib",
                             "--no-default-features", "--no-run", "--message-format=json"],
                            cwd=ROOT, capture_output=True, text=True, check=False)
    if result.returncode:
        print(result.stderr, file=sys.stderr, end="")
        for line in result.stdout.splitlines():
            message = json.loads(line)
            if message.get("reason") == "compiler-message":
                print(message.get("message", {}).get("rendered", ""), file=sys.stderr, end="")
        raise SystemExit(result.returncode)
    executables = {item["executable"] for line in result.stdout.splitlines()
                   if (item := json.loads(line)).get("reason") == "compiler-artifact"
                   and item.get("executable") and item["target"]["name"] == "iptv_app"}
    if len(executables) != 1:
        raise RuntimeError("expected exactly one production application test executable")
    binary = executables.pop()
    listing = subprocess.check_output([binary, "--list", "--ignored", "--format=terse"], text=True)
    if listing.splitlines().count(TEST + ": test") != 1:
        raise RuntimeError("production credential test absent (test-support must be disabled)")
    with tempfile.TemporaryDirectory(prefix="patina-credentials-") as temporary:
        root = Path(temporary)
        (root / "run").mkdir(mode=0o700)
        (root / ".patina-broker").write_text("owned disposable credential test\n")
        subprocess.run(["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()),
                        "--inside", binary], env=environment(root, os.environ), check=True, timeout=210)


if __name__ == "__main__":
    main()
