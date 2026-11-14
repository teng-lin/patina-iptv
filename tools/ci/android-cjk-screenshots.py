#!/usr/bin/env python3
"""Capture the four CJK scenarios on a disposable synthetic Android emulator.

Requires a debuggable APK built with the synthetic development playlist. This
clears app data, so use only a dedicated test emulator, never a personal device.
Run the mock fixture server before this script. Screenshots need visual review.
"""

import argparse
import json
import os
import pathlib
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("serial", help="dedicated emulator serial")
    parser.add_argument("--adb", default="adb")
    parser.add_argument("--out", type=pathlib.Path, default=pathlib.Path("target/cjk"))
    parser.add_argument("--wait", type=float, default=12)
    args = parser.parse_args()
    if not args.serial.startswith("emulator-"):
        parser.error("use a disposable emulator, not a physical device")
    if args.wait <= 0:
        parser.error("--wait must be positive")
    pkg = "com.hantekllc.iptvrs"

    def adb(*command, data=None):
        return subprocess.run(
            [args.adb, "-s", args.serial, *command],
            input=data, capture_output=True, check=True,
        ).stdout

    # The smoke workflow clears app data and imports only the compiled-in
    # synthetic fixture. A failed import must not produce a "guide" capture
    # of the empty welcome screen.
    args.out.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["PATH"] = str(pathlib.Path(args.adb).resolve().parent) + os.pathsep + env["PATH"]
    env["IPTV_SMOKE_OUT"] = str(args.out / "smoke")
    env["IPTV_SMOKE_SETTLE"] = "1"
    subprocess.run(
        ["bash", str(pathlib.Path(__file__).with_name("android-smoke.sh")), args.serial],
        env=env, check=True,
    )
    for locale in ("zh", "zh-Hant", "ja", "ko"):
        adb("shell", "am", "force-stop", pkg)
        settings = {
            "format": "iptv-rs.settings", "version": 2,
            "global": {"general.language": locale, "general.resume_last_channel": False},
            "playlists": {}, "groups": {},
        }
        adb("shell", "run-as", pkg, "mkdir", "-p", "files")
        adb(
            "shell", "run-as", pkg, "sh", "-c", "'cat > files/settings.json'",
            data=json.dumps(settings).encode(),
        )
        adb("shell", "am", "start", "-W", "-n", pkg + "/.host.TvActivity")
        time.sleep(args.wait)
        (args.out / (locale + ".png")).write_bytes(adb("exec-out", "screencap", "-p"))
    print(f"Captured four scenarios in {args.out}; review glyphs and clipping manually.")


if __name__ == "__main__":
    main()
