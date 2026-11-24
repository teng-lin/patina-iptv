#!/usr/bin/env python3
"""Start a fresh CI emulator using the hosted runner's installed Android tools."""

import argparse
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import uuid

from adb import stop

PORT = 5554
SERIAL = f"emulator-{PORT}"
ADB_GUARD = Path(__file__).with_name("adb.py")
SMOKE = Path(__file__).resolve().parents[1] / "android-smoke.sh"


def prepare_sdk(sdk, target, env):
    tools = sdk / "cmdline-tools/latest/bin"
    image = f"system-images;android-36;{target};x86_64"
    packages = []
    if not (sdk / f"system-images/android-36/{target}/x86_64/source.properties").is_file():
        packages.append(image)
    for package, executable in (("emulator", "emulator/emulator"), ("platform-tools", "platform-tools/adb")):
        if not os.access(sdk / executable, os.X_OK):
            packages.append(package)
    if not any(os.access(d8, os.X_OK) for d8 in sdk.glob("build-tools/*/d8")):
        packages.append("build-tools;35.0.1")
    if packages:
        print(f"Installing missing Android SDK packages: {', '.join(packages)}", flush=True)
        subprocess.run([tools / "sdkmanager", "--install", *packages],
                       env=env, input="y\n" * 64, text=True, check=True, timeout=600)
    return tools, image


def adb(*args, env, phase, timeout=15, capture=False):
    return subprocess.run([sys.executable, ADB_GUARD, "--phase", phase, "--category", args[0],
                           "--timeout", str(timeout), "--", "adb", "-s", SERIAL, *args],
                          env=env, check=True, capture_output=capture, text=True).stdout


def wait_for_boot(process, name, env):
    start = time.monotonic()
    while time.monotonic() - start < 180:
        if process.poll() is not None:
            raise RuntimeError("emulator exited before boot")
        try:
            owned = adb("emu", "avd", "name", phase="boot", env=env, timeout=5, capture=True).splitlines()
            if owned and owned[0] != name:
                raise RuntimeError("ADB serial belongs to a different AVD")
            if owned and adb("shell", "getprop", "sys.boot_completed", phase="boot", env=env,
                             timeout=5, capture=True).strip() == "1":
                print(f"Emulator booted in {time.monotonic() - start:.1f}s", flush=True)
                return
        except subprocess.CalledProcessError:
            pass
        time.sleep(2)
    raise RuntimeError("emulator boot timed out")


def run_smoke(apk, env, output):
    for attempt in range(2):
        process = subprocess.Popen(["bash", SMOKE, SERIAL, apk], env=env, start_new_session=True)
        try:
            status = process.wait()
        finally:
            stop(process, 2)
        crash = output / "crash.log"
        if status == 0 or attempt or not crash.is_file() or "DeadSystemException" not in crash.read_text(errors="replace"):
            return status
        print("emulator system died; rebooting for one more run", flush=True)
        adb("reboot", phase="system recovery", env=env, timeout=30)
        adb("wait-for-device", phase="system recovery", env=env, timeout=120)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=("google_apis", "android-tv"))
    parser.add_argument("profile", choices=("pixel_tablet", "tv_1080p"))
    parser.add_argument("apk", type=Path)
    args = parser.parse_args()

    sdk = Path(os.environ["ANDROID_HOME"])
    env = dict(os.environ, PATH=os.pathsep.join((str(sdk / "platform-tools"), os.environ["PATH"])))
    tools, image = prepare_sdk(sdk, args.target, env)
    for port in (PORT, PORT + 1):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", port))
    output = Path(env.get("IPTV_SMOKE_OUT", "target/android-smoke"))
    output.mkdir(parents=True, exist_ok=True)
    name = "patina-validation-" + uuid.uuid4().hex
    with tempfile.TemporaryDirectory(prefix="patina-ci-avd-", dir=env.get("RUNNER_TEMP")) as directory:
        env.update(ANDROID_AVD_HOME=directory, IPTV_SMOKE_DISPOSABLE_AVD=name, ANDROID_SERIAL=SERIAL)
        subprocess.run([tools / "avdmanager", "create", "avd", "-n", name, "--package", image,
                        "--device", args.profile], env=env, input="no\n", text=True, check=True, timeout=60)
        config = Path(directory) / f"{name}.avd/config.ini"
        with config.open("a") as stream:
            stream.write("\nhw.cpu.ncore=2\nhw.ramSize=4096M\ndisk.dataPartition.size=4096M\n")

        with (output / "emulator.log").open("w") as log:
            emulator = subprocess.Popen([sdk / "emulator/emulator", "-avd", name, "-port", str(PORT), "-no-window",
                                         "-no-snapshot", "-no-audio", "-no-boot-anim", "-gpu", "swiftshader"],
                                        stdout=log, stderr=subprocess.STDOUT, env=env, start_new_session=True)
            try:
                wait_for_boot(emulator, name, env)
                adb("shell", "input", "keyevent", "82", phase="boot", env=env)
                for setting in ("window_animation_scale", "transition_animation_scale", "animator_duration_scale"):
                    adb("shell", "settings", "put", "global", setting, "0.0", phase="boot", env=env)
                return run_smoke(args.apk, env, output)
            finally:
                stop(emulator, 3)


if __name__ == "__main__":
    def interrupted(signum, frame):
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    sys.exit(main())
