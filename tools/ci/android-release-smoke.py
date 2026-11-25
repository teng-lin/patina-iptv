#!/usr/bin/env python3
"""Launch an unmodified release APK only in the runner's owned fresh emulator.

This deliberately records import, playback, PiP, backup/restore and recording as
pending. Perform those through the visible UI with authorized synthetic media;
neither private app storage nor development fixture entry points are used here.
"""
import json
import os
from pathlib import Path
import re
import runpy
import subprocess
import sys
import time

AVD = runpy.run_path(str(Path(__file__).with_name("disposable-avd.py")))
PACKAGE = AVD["RELEASE_PACKAGE"]


def check_alive(serial, component, foreground=False):
    pid = AVD["adb"](serial, "shell", "pidof", PACKAGE)
    if not re.fullmatch(r"[0-9]+(?: [0-9]+)*", pid):
        raise RuntimeError("release process is not running")
    if foreground:
        activities = AVD["adb"](serial, "shell", "dumpsys", "activity", "activities")
        package, activity = component.split("/", 1)
        expanded = package + "/" + (package + activity if activity.startswith(".") else activity)
        if not any(re.search(r"(?:mResumedActivity|topResumedActivity|ResumedActivity)", line)
                   and (component in line or expanded in line) for line in activities.splitlines()):
            raise RuntimeError("resolved release launcher is not resumed")


def run(serial, apk):
    expected = os.environ.get("IPTV_SMOKE_DISPOSABLE_AVD", "")
    if not re.fullmatch(r"patina-validation-[a-zA-Z0-9_-]+", expected):
        raise ValueError("release smoke requires an owned disposable AVD")
    def adb(*args):
        return AVD["adb"](serial, *args)
    if adb("emu", "avd", "name").splitlines()[0] != expected:
        raise ValueError("serial does not identify the owned disposable AVD")
    package = subprocess.check_output(["apkanalyzer", "manifest", "application-id", apk],
                                      text=True, timeout=30).strip()
    debug = subprocess.check_output(["apkanalyzer", "manifest", "debuggable", apk],
                                    text=True, timeout=30).strip()
    AVD["validate_apk"](package, debug, True)
    AVD["smoke_timeout"](os.environ)
    out = Path(os.environ["IPTV_SMOKE_OUT"])
    out.mkdir(parents=True, exist_ok=True)
    checks = dict.fromkeys(("launch", "playlist_import", "playback", "pip", "backup_restore", "recording"), "pending")
    report = {"schema": 1, "checks": checks, "entry_point": "Android launcher intent"}
    try:
        # No uninstall/clear/run-as: the containing runner already wiped its
        # private copy, and never gives this helper a personal/device profile.
        adb("install", "-r", apk)
        adb("logcat", "-b", "main", "-b", "system", "-b", "crash", "-c")
        # pm reports a valid absent feature with exit status 1 on some images.
        query = subprocess.run(["adb", "-s", serial, "shell", "pm", "has-feature", "android.software.leanback"],
                               check=False, capture_output=True, text=True, timeout=15)
        feature = query.stdout.strip()
        if query.returncode not in (0, 1):
            raise ValueError("cannot query Android form factor")
        if feature not in ("true", "false"):
            raise ValueError("cannot establish Android form factor")
        category = "android.intent.category." + ("LEANBACK_LAUNCHER" if feature == "true" else "LAUNCHER")
        report["launcher_category"] = category
        adb("shell", "input", "keyevent", "KEYCODE_WAKEUP")
        adb("shell", "wm", "dismiss-keyguard")
        resolved = adb("shell", "cmd", "package", "resolve-activity", "--brief", "-a",
                       "android.intent.action.MAIN", "-c", category, "-p", PACKAGE).splitlines()[-1]
        if not re.fullmatch(re.escape(PACKAGE) + r"/[A-Za-z0-9_.$]+", resolved):
            raise RuntimeError("cannot resolve the release launcher")
        report["launcher_component"] = resolved
        result = adb("shell", "am", "start", "-W", "-n", resolved, "-a", "android.intent.action.MAIN",
                     "-c", category)
        if "Error:" in result or "Status: ok" not in result:
            raise RuntimeError("release launcher did not start")
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            log = adb("logcat", "-d", "-s", "nsh:I")
            if "iptv startup: fonts registered" in log and "iptv pip: supported" in log:
                check_alive(serial, resolved, foreground=True)
                checks["launch"] = "passed"
                break
            time.sleep(1)
        else:
            raise RuntimeError("release did not finish normal startup")
        (out / "device.json").write_text(json.dumps({
            "api": adb("shell", "getprop", "ro.build.version.sdk"),
            "abi": adb("shell", "getprop", "ro.product.cpu.abi"),
            "leanback": feature == "true",
        }, indent=2) + "\n")
        with (out / "01-release-launch.png").open("wb") as screenshot:
            subprocess.run(["adb", "-s", serial, "exec-out", "screencap", "-p"],
                           check=True, stdout=screenshot, timeout=30)
        print("Release launch passed; remaining acceptance journeys pending. Evidence: " + str(out), flush=True)
        hold = int(os.environ.get("IPTV_SMOKE_HOLD", "0"))
        until = time.monotonic() + hold
        while time.monotonic() < until:
            time.sleep(min(5, until - time.monotonic()))
        crashes = adb("logcat", "-d", "-b", "crash")
        (out / "crash.log").write_text(crashes)
        (out / "app.log").write_text(adb("logcat", "-d", "-s", "nsh:I"))
        if "FATAL EXCEPTION" in crashes or "Fatal signal" in crashes:
            raise RuntimeError("release crashed; inspect retained evidence")
        check_alive(serial, resolved)
    except subprocess.CalledProcessError as error:
        (out / "command-failure.log").write_text(str(error.stdout or "") + str(error.stderr or ""))
        raise
    finally:
        (out / "release-checklist.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("supply the runner's explicit emulator serial and release APK")
    run(*sys.argv[1:])
