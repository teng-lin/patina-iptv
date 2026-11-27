#!/usr/bin/env python3
"""Run synthetic Android smoke on a complete owned copy of an existing AVD.

Never resets, deletes images from, or starts the source profile. Failed runs retain
logs and the operation manifest; only the copied profile is removed. Build the APK
with IPTV_VALIDATION_APP_ID=com.hantekllc.iptvrs.validation first.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import uuid
from xml.etree import ElementTree

PACKAGE = "com.hantekllc.iptvrs.validation"
RELEASE_PACKAGE = "com.hantekllc.iptvrs"
CAST_TEST_PACKAGE = PACKAGE + ".test"
CAST_TEST_RUNNER = "com.hantekllc.iptvrs.host.CastPlatformInstrumentation"
ANDROID_ATTRIBUTE = "{http://schemas.android.com/apk/res/android}"
MARKER = ".patina-owned-avd.json"
ROOT = Path(__file__).resolve().parents[2]


def plain_path(path):
    path = Path(os.path.abspath(path))
    for part in (path, *path.parents):
        if part.is_symlink():
            raise ValueError("symlink paths are not valid disposable profile roots")
    return path


def read_config(path):
    return dict(line.split("=", 1) for line in path.read_text().splitlines() if "=" in line)


def flatten_images(profile, source, sdk, qemu_img, runner=subprocess.run):
    """Flatten copied qcow2 images so no backing path can reach the source AVD."""
    sdk = plain_path(sdk) if sdk is not None else None
    for image in profile.rglob("*"):
        if not image.is_file():
            continue
        with image.open("rb") as stream:
            if stream.read(4) != b"QFI\xfb":
                continue
        if not qemu_img:
            raise ValueError("qemu-img is required to isolate qcow2 backing chains")
        info = runner([qemu_img, "info", "--output=json", "--backing-chain", str(image)],
                      check=True, capture_output=True, text=True, timeout=30)
        chain = json.loads(info.stdout)
        if not isinstance(chain, list) or not chain:
            raise ValueError("cannot establish qcow2 backing chain")
        for entry in chain:
            backing = plain_path(entry["filename"])
            owned = backing.is_relative_to(profile) or backing.is_relative_to(source)
            readonly_sdk = (sdk is not None and backing.is_relative_to(sdk / "system-images")
                            and backing.is_file() and backing.stat().st_mode & 0o222 == 0)
            if not owned and not readonly_sdk:
                raise ValueError("qcow2 backing image is outside the copied/source profile or read-only SDK")
        # Conversion reads the original backing chain but writes only a new copied
        # image. No unsafe rebase and no mutation of shared backing files.
        temporary = image.with_name(image.name + ".patina-flat")
        runner([qemu_img, "convert", "-f", "qcow2", "-O", "qcow2", str(image), str(temporary)],
               check=True, capture_output=True, text=True, timeout=300)
        flat = runner([qemu_img, "info", "--output=json", "--backing-chain", str(temporary)],
                      check=True, capture_output=True, text=True, timeout=30)
        entries = json.loads(flat.stdout)
        if len(entries) != 1 or entries[0].get("backing-filename"):
            raise ValueError("flattened qcow2 still references a backing image")
        temporary.replace(image)


def prepare(source, scratch, copier=shutil.copytree, sdk=None, qemu_img=None, image_runner=subprocess.run):
    source, scratch = plain_path(source), plain_path(scratch)
    if not source.is_dir() or source.suffix != ".avd" or not (source / "config.ini").is_file():
        raise ValueError("source must be an existing .avd profile with config.ini")
    if (source == scratch or source in scratch.parents or scratch in source.parents
            or ".android" in scratch.parts or scratch.suffix == ".avd"):
        raise ValueError("scratch must be separate from original AVD and Android profile roots")
    if any(path.is_symlink() for path in source.rglob("*")):
        raise ValueError("source profile contains symlinks; copy only self-contained profiles")
    scratch.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="patina-avd-", dir=scratch))
    token = uuid.uuid4().hex
    record = {"schema": 1, "token": token, "scratch": str(scratch), "source": str(source)}
    (root / MARKER).write_text(json.dumps(record))
    try:
        name = "patina-validation-" + token
        avd_home = root / "avd"
        avd_home.mkdir()
        profile = avd_home / (name + ".avd")
        copier(source, profile)
        flatten_images(profile, source, sdk, qemu_img, image_runner)
        # Locks and the generated qemu config describe the original running VM.
        # Only copies are removed; preserve userdata, cache, encryption and snapshots.
        for path in profile.iterdir():
            if path.name.endswith(".lock") or path.name == "hardware-qemu.ini":
                shutil.rmtree(path) if path.is_dir() else path.unlink()
        config = read_config(profile / "config.ini")
        for key, value in list(config.items()):
            if value.startswith(str(source) + os.sep):
                config[key] = str(profile) + value[len(str(source)):]
            if (key == "sdcard.path" or key.startswith("disk.") and key.endswith(".path")) and value:
                candidate = Path(config[key])
                candidate = plain_path(candidate if candidate.is_absolute() else profile / candidate)
                if not candidate.is_relative_to(profile):
                    raise ValueError(f"external writable AVD path refused: {key}")
        config.update(AvdId=name, **{"avd.ini.displayname": name})
        (profile / "config.ini").write_text("".join(f"{key}={value}\n" for key, value in config.items()))
        (avd_home / (name + ".ini")).write_text(f"avd.ini.encoding=UTF-8\npath={profile}\ntarget={config.get('target', 'android-36')}\n")
        return root, token, name
    except BaseException:
        cleanup(root, scratch, token)
        raise


def validate_owned(root, scratch, token):
    root, scratch = plain_path(root), plain_path(scratch)
    if root.parent != scratch or not re.fullmatch(r"patina-avd-[a-zA-Z0-9_-]+", root.name):
        raise ValueError("refusing cleanup outside the owned scratch directory")
    marker = root / MARKER
    if marker.is_symlink() or not marker.is_file():
        raise ValueError("refusing cleanup without an ownership marker")
    record = json.loads(marker.read_text())
    if (record.get("schema") != 1 or record.get("token") != token or record.get("scratch") != str(scratch)
            or Path(record.get("source", "/")) == root or not token):
        raise ValueError("refusing cleanup with a mismatched ownership marker")
    return root


def boot_config(root, scratch, token, name, emulator, port, gpu="swiftshader"):
    root = validate_owned(root, scratch, token)
    if name != "patina-validation-" + token or not (root / "avd" / (name + ".avd")).is_dir():
        raise ValueError("refusing boot/reset of an unowned AVD")
    env = dict(os.environ, ANDROID_AVD_HOME=str(root / "avd"),
               ANDROID_USER_HOME=str(root / "android"), ANDROID_EMULATOR_HOME=str(root / "android"))
    # Reset only the verified private copy. Copied apps/accounts must never start.
    command = [emulator, "-avd", name, "-port", str(port), "-wipe-data",
               "-no-snapshot", "-no-window", "-no-audio", "-no-boot-anim",
               "-gpu", gpu]
    return command, env


def cleanup(root, scratch, token):
    shutil.rmtree(validate_owned(root, scratch, token))


def adb(serial, *arguments, **kwargs):
    if not re.fullmatch(r"emulator-\d+", serial):
        raise ValueError("validation requires an explicit emulator serial")
    timeout = 120 if arguments and arguments[0] == "install" else 15
    if arguments[:3] == ("shell", "am", "start"):
        timeout = 60
    return subprocess.run(["adb", "-s", serial, *arguments], check=True, timeout=timeout,
                          capture_output=True, text=True, **kwargs).stdout.strip()


def stop(process):
    if process is None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        process.wait()
        return
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        pass
    # A shell may exit before its adb/probe children. Reap the entire owned group.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait(timeout=10)


# Seconds an operator may hold the smoke for manual checks (IPTV_SMOKE_HOLD);
# the runner adds the hold to the smoke's own budget.
SMOKE_BUDGET = 1200
HOLD_LIMIT = 1800


def smoke_timeout(env):
    hold = env.get("IPTV_SMOKE_HOLD", "")
    if not hold:
        return SMOKE_BUDGET
    if not re.fullmatch(r"[1-9][0-9]{0,3}", hold) or int(hold) > HOLD_LIMIT:
        raise ValueError(f"IPTV_SMOKE_HOLD must be whole seconds from 1 through {HOLD_LIMIT}")
    return SMOKE_BUDGET + int(hold)


def validate_apk(package, debuggable, release):
    expected = RELEASE_PACKAGE if release else PACKAGE
    if package != expected:
        raise ValueError(f"APK must use {expected}")
    if release and debuggable != "false":
        raise ValueError("release smoke requires a non-debuggable APK")



def validate_cast_test(manifest):
    root = ElementTree.fromstring(manifest)
    entries = root.findall("instrumentation")
    if (root.get("package") != CAST_TEST_PACKAGE or len(entries) != 1
            or entries[0].get(ANDROID_ATTRIBUTE + "targetPackage") != PACKAGE
            or entries[0].get(ANDROID_ATTRIBUTE + "name") != CAST_TEST_RUNNER):
        raise ValueError("expected the disposable Patina Cast instrumentation APK")


def apk_manifest(apk, attribute):
    return subprocess.check_output(
        ["apkanalyzer", "manifest", attribute, str(apk)], text=True, timeout=30).strip()


def stage_cast_apks(root, apk, cast_test):
    # The build outputs may change while the emulator boots. Validate and install
    # the same copies from the private disposable run.
    app_copy = root / "app.apk"
    test_copy = root / "cast-platform-test.apk"
    for source, copy in ((apk, app_copy), (cast_test, test_copy)):
        shutil.copyfile(source, copy)
        copy.chmod(0o400)
    package = apk_manifest(app_copy, "application-id")
    debuggable = apk_manifest(app_copy, "debuggable")
    validate_apk(package, debuggable, release=False)
    if debuggable != "true":
        raise ValueError("Cast platform tests require a debuggable validation APK")
    validate_cast_test(apk_manifest(test_copy, "print"))
    return app_copy, test_copy


def run_cast_test(serial, apk, test_apk, output, collect):
    adb(serial, "install", "-r", str(apk))
    adb(serial, "install", "-t", "-r", str(test_apk))
    adb(serial, "shell", "input", "keyevent", "KEYCODE_WAKEUP")
    adb(serial, "shell", "wm", "dismiss-keyguard")
    adb(serial, "shell", "settings", "put", "secure", "immersive_mode_confirmations", "confirmed")
    command = ["adb", "-s", serial, "shell", "am", "instrument", "-w", "-r"]
    if collect:
        command += ["-e", "coverage", "true"]
    command.append(f"{CAST_TEST_PACKAGE}/{CAST_TEST_RUNNER}")
    trace = output / "instrumentation.txt"
    with trace.open("w") as log:
        status = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=120).returncode
    result = trace.read_text()
    print(result)
    if status or "INSTRUMENTATION_CODE: -1" not in result or "INSTRUMENTATION_STATUS_CODE: 0" not in result:
        raise RuntimeError(f"Android Cast platform test failed: {trace}")
    if collect:
        data = subprocess.check_output([
            "adb", "-s", serial, "exec-out", "run-as", PACKAGE,
            "cat", "files/cast-platform.exec",
        ], timeout=15)
        if not data:
            raise RuntimeError("empty Android Cast platform coverage data")
        (output / "cast-platform.exec").write_bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="existing source .avd directory; never modified")
    parser.add_argument("apk", type=Path, help="synthetic validation-ID debug APK")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--release", action="store_true",
                      help="launch a non-debuggable production-ID APK; remaining journeys stay pending")
    mode.add_argument("--cast-platform-test", type=Path,
                      help="run the synthetic Cast platform test APK instead of the standard smoke")
    parser.add_argument("--cast-platform-coverage", action="store_true",
                        help="collect JaCoCo data from an instrumented Cast platform test run")
    parser.add_argument("--artifact-commit", help="verified compiled source SHA; separate from harness checkout")
    parser.add_argument("--artifact-nsh", help="verified compiled NSH SHA; separate from harness checkout")
    parser.add_argument("--scratch", type=Path, default=ROOT / "target/disposable-avds")
    parser.add_argument("--out", type=Path, default=ROOT / "target/android-validation")
    parser.add_argument("--port", type=int, default=5580)
    parser.add_argument("--emulator", default="emulator")
    parser.add_argument("--gpu", choices=("auto", "host", "software", "lavapipe", "swiftshader", "swangle", "swiftshader_indirect"),
                        default="swiftshader", help="renderer to record and compare during emulator diagnostics")
    parser.add_argument("--qemu-img", default=shutil.which("qemu-img"), help="SDK qemu-img or a compatible host tool")
    args = parser.parse_args()
    if args.cast_platform_coverage and not args.cast_platform_test:
        parser.error("--cast-platform-coverage requires --cast-platform-test")
    for revision in (args.artifact_commit, args.artifact_nsh):
        if revision is not None and not re.fullmatch(r"[0-9a-f]{40}", revision):
            parser.error("artifact source revisions must be full lowercase Git SHAs")
    try:
        budget = smoke_timeout(os.environ)
    except ValueError as error:
        parser.error(str(error))
    if args.port % 2 or not 5554 <= args.port <= 5682:
        parser.error("--port must be an even emulator console port from 5554 through 5682")
    output = plain_path(args.out)
    source = plain_path(args.source)
    if output == source or source in output.parents or ".android" in output.parts:
        parser.error("evidence output must be outside the original Android profile")
    apk = args.apk.resolve(strict=True)
    cast_test = args.cast_platform_test.resolve(strict=True) if args.cast_platform_test else None
    package = PACKAGE
    if not cast_test:
        # Refuse a normal app APK before any device action, including uninstall/install.
        package = apk_manifest(apk, "application-id")
        debuggable = apk_manifest(apk, "debuggable")
        try:
            validate_apk(package, debuggable, args.release)
        except ValueError as error:
            parser.error(str(error))
    for port in (args.port, args.port + 1):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", port))
    sdk = Path(os.environ.get("ANDROID_HOME", str(Path.home() / "Android/Sdk")))
    qemu_img = args.qemu_img
    if not qemu_img:
        candidates = [sdk / "emulator/qemu-img", sdk / "emulator/qemu/linux-x86_64/qemu-img",
                      sdk / "emulator/qemu/darwin-aarch64/qemu-img"]
        qemu_img = next((str(path) for path in candidates if path.is_file()), None)
    root, token, name = prepare(args.source, args.scratch, sdk=sdk, qemu_img=qemu_img)
    process = None
    log = None
    smoke = None
    try:
        if cast_test:
            try:
                apk, cast_test = stage_cast_apks(root, apk, cast_test)
            except (ValueError, ElementTree.ParseError) as error:
                parser.error(str(error))
        output.mkdir(parents=True, exist_ok=True)
        output = Path(tempfile.mkdtemp(prefix="run-", dir=output))
        serial = f"emulator-{args.port}"
        operation = {"schema": 1, "avd": name, "serial": serial, "package": package,
                     "source": str(args.source.resolve()), "copy": str(root), "audio_enabled": False,
                     "fresh_userdata": True,
                     "gpu": args.gpu,
                     "mode": "release-launch" if args.release else "fixture-smoke",
                     "checkout_nsh_revision": (ROOT / "nsh.rev").read_text().strip(),
                     "artifact_commit": args.artifact_commit,
                     "artifact_nsh_revision": args.artifact_nsh,
                     "apk_sha256": hashlib.sha256(apk.read_bytes()).hexdigest(),
                     "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                     "result": "incomplete"}
        if cast_test:
            operation["mode"] = "cast-platform-test"
            operation["test_apk_sha256"] = hashlib.sha256(cast_test.read_bytes()).hexdigest()
        manifest = output / "operation.json"
        manifest.write_text(json.dumps(operation, indent=2) + "\n")
        command, env = boot_config(root, args.scratch, token, name, args.emulator, args.port, args.gpu)
        env.update(ANDROID_SERIAL=serial,
                   IPTV_SMOKE_OUT=str(output / "smoke"), IPTV_SMOKE_DISPOSABLE_AVD=name)
        (root / "android").mkdir()
        log = (output / "emulator.log").open("w")
        process = subprocess.Popen(command, env=env, stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError("owned emulator exited before boot")
            try:
                actual = adb(serial, "emu", "avd", "name").splitlines()[0]
                if actual != name:
                    raise RuntimeError("serial belongs to another profile; refusing device actions")
                if adb(serial, "shell", "getprop", "sys.boot_completed") == "1":
                    break
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                pass
            time.sleep(1)
        else:
            raise RuntimeError("owned emulator boot timed out")
        if cast_test:
            run_cast_test(serial, apk, cast_test, output, args.cast_platform_coverage)
        else:
            command = (["python3", str(ROOT / "tools/ci/android-release-smoke.py")] if args.release else
                       ["bash", str(ROOT / "tools/ci/android-smoke.sh")])
            smoke = subprocess.Popen([*command, serial, str(apk)],
                                     env=env, start_new_session=True)
            if smoke.wait(timeout=budget):
                raise RuntimeError("synthetic smoke failed; evidence retained")
        operation["result"] = "launch-passed-journeys-pending" if args.release else "passed"
        manifest.write_text(json.dumps(operation, indent=2) + "\n")
        print(f"Evidence: {output}")
    finally:
        # Reap only the process group we started, never an adb-selected unknown VM.
        stop(smoke)
        stop(process)
        if log is not None:
            log.close()
        cleanup(root, args.scratch, token)


if __name__ == "__main__":
    def interrupted(signum, frame):
        raise KeyboardInterrupt("validation interrupted")
    signal.signal(signal.SIGTERM, interrupted)
    main()
