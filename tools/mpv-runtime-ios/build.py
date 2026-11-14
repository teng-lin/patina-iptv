"""Build the guarded iOS Libavformat and test it with MPVKit's pinned player."""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import zipfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
BASE = ROOT / "tools/mpv-runtime"
sys.path.insert(0, str(BASE))
import prepare  # noqa: E402


def run(arguments, **kwargs):
    print("+ " + " ".join(map(str, arguments)), flush=True)
    return subprocess.run(list(map(str, arguments)), check=True, **kwargs)


def output(arguments):
    return subprocess.check_output(list(map(str, arguments)), text=True).strip()


def sdk_path(sdk):
    return output(["xcrun", "--sdk", sdk, "--show-sdk-path"])


def triple(arch, simulator):
    return f"{arch}-apple-ios15.0" + ("-simulator" if simulator else "")


def build_archive(work, arch, simulator):
    sdk = "iphonesimulator" if simulator else "iphoneos"
    directory = work / "build" / (arch + ("-simulator" if simulator else "-device"))
    directory.mkdir(parents=True, exist_ok=True)
    compiler = output(["xcrun", "--sdk", sdk, "--find", "clang"])
    flags = f"-target {triple(arch, simulator)} -isysroot {sdk_path(sdk)} -ffile-prefix-map={work}=/patina-native"
    run(
        [
            work / "sources/ffmpeg/configure",
            "--enable-cross-compile",
            "--target-os=darwin",
            "--arch=" + ("aarch64" if arch == "arm64" else arch),
            "--cc=" + compiler,
            "--ar=" + output(["xcrun", "--sdk", sdk, "--find", "ar"]),
            "--ranlib=" + output(["xcrun", "--sdk", sdk, "--find", "ranlib"]),
            "--extra-cflags=" + flags,
            "--extra-ldflags=" + flags,
            "--disable-autodetect",
            "--disable-x86asm",
            "--disable-gpl",
            "--disable-version3",
            "--disable-nonfree",
            "--disable-shared",
            "--enable-static",
            "--enable-pic",
            "--enable-pthreads",
            "--enable-network",
            "--enable-securetransport",
            "--enable-zlib",
            "--disable-programs",
            "--disable-doc",
            "--disable-debug",
            "--disable-avdevice",
        ],
        cwd=directory,
    )
    configured = (directory / "config_components.h").read_text()
    if "#define CONFIG_PATINA_RELAY_GUARD_V1_PROTOCOL 1" not in configured:
        raise ValueError("The iOS build did not enable the native guard marker")
    run(["make", "-j", os.environ.get("JOBS", "4"), "libavformat/libavformat.a"], cwd=directory)
    return directory / "libavformat/libavformat.a"


def framework(stock, destination, libraries):
    shutil.copytree(stock, destination)
    binary = destination / "Libavformat"
    if len(libraries) == 1:
        shutil.copy2(libraries[0], binary)
    else:
        run(["xcrun", "lipo", "-create", *libraries, "-output", binary])
    run(["xcrun", "lipo", "-info", binary])
    return destination


def constants():
    source = (ROOT / "crates/iptv-player-mpv/build.rs").read_text()
    return {name: re.findall(r'"([A-Za-z0-9_+.-]+)"', re.search(rf"const {name}:.*?= &\[(.*?)\];", source, re.S)[1]) for name in ("FRAMEWORKS", "SYSTEM_FRAMEWORKS", "SYSTEM_LIBS")}


def compile_probe(work, destination, replacement=None):
    slice_name = "ios-arm64_x86_64-simulator"
    include = work / "probe-include"
    include.mkdir(exist_ok=True)
    headers = work / "mpvkit/Libmpv.xcframework" / slice_name / "Libmpv.framework/Headers/mpv"
    if not (headers / "client.h").is_file():
        raise ValueError("The pinned MPVKit client headers are missing")
    link = include / "mpv"
    if not link.exists():
        link.symlink_to(headers, target_is_directory=True)
    values = constants()
    command = ["xcrun", "--sdk", "iphonesimulator", "clang", "-target", triple("arm64", True), "-isysroot", sdk_path("iphonesimulator"), "-I", include, BASE / "relay_guard_probe.c", "-o", destination]
    for name in values["FRAMEWORKS"]:
        bundle = replacement if name == "Libavformat" and replacement else work / "mpvkit" / (name + ".xcframework")
        command += ["-F", bundle / slice_name, "-framework", name]
    command += ["-L", work / "mpvkit/MoltenVK.xcframework" / slice_name, "-lMoltenVK"]
    for name in [*values["SYSTEM_FRAMEWORKS"], "Foundation", "Security"]:
        command += ["-framework", name]
    command += ["-l" + name for name in [*values["SYSTEM_LIBS"], "objc"]]
    run(command)
    run(["codesign", "--force", "--sign", "-", destination])


def launcher(path, udid, executable):
    path.write_text(
        "#!/usr/bin/env python3\nimport os, subprocess, sys\n"
        "environment = dict(os.environ)\n"
        "for key in ('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy', 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY'):\n"
        "    environment.pop('SIMCTL_CHILD_' + key, None)\n"
        "    if key in environment: environment['SIMCTL_CHILD_' + key] = environment[key]\n"
        f"sys.exit(subprocess.call(['xcrun', 'simctl', 'spawn', {udid!r}, {str(executable)!r}, *sys.argv[1:]], env=environment))\n"
    )
    path.chmod(0o755)


def smoke(work, baseline, guarded):
    devices = json.loads(output(["xcrun", "simctl", "list", "devices", "available", "--json"]))
    candidates = [device for runtime, group in devices["devices"].items() if "SimRuntime.iOS-" in runtime for device in group]
    if not candidates:
        raise ValueError("No available iOS simulator runtime")
    device = next((item for item in candidates if item["state"] == "Booted"), candidates[0])
    udid = device["udid"]
    started = device["state"] != "Booted"
    try:
        if started:
            run(["xcrun", "simctl", "boot", udid])
        run(["xcrun", "simctl", "bootstatus", udid, "-b"])
        for executable, extra in [(baseline, ["--baseline"]), (guarded, [])]:
            entry = work / (executable.name + "-launcher.py")
            launcher(entry, udid, executable)
            run([sys.executable, BASE / "relay_guard.py", "--probe", entry, "--ffmpeg", output(["which", "ffmpeg"]), *extra])
    finally:
        if started:
            run(["xcrun", "simctl", "shutdown", udid])


def zip_bundle(directory, destination):
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for path in sorted(directory.rglob("*")):
            if path.is_file():
                info = zipfile.ZipInfo(directory.name + "/" + path.relative_to(directory).as_posix(), (1980, 1, 1, 0, 0, 0))
                info.external_attr = 0o100644 << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(info, path.read_bytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    work, destination = args.work.resolve(), args.output.resolve()
    work.mkdir(parents=True, exist_ok=True)
    destination.mkdir(parents=True, exist_ok=True)
    expected = (ROOT / "tools/mpv-runtime-unix/macos/xcode-version").read_text().strip()
    if output(["xcodebuild", "-version"]).splitlines()[0] != "Xcode " + expected:
        raise ValueError("Select the pinned Xcode before building the iOS runtime")
    os.environ["ZERO_AR_DATE"] = "1"
    prepare.prepare(HERE / "sources.lock.json", work)
    run(["bash", ROOT / "tools/fetch-mpvkit.sh", work / "mpvkit"])
    baseline = work / "baseline-probe"
    compile_probe(work, baseline)
    device = build_archive(work, "arm64", False)
    arm_sim = build_archive(work, "arm64", True)
    intel_sim = build_archive(work, "x86_64", True)
    stage = work / "frameworks"
    if stage.exists():
        shutil.rmtree(stage)
    device_framework = framework(work / "mpvkit/Libavformat.xcframework/ios-arm64/Libavformat.framework", stage / "device/Libavformat.framework", [device])
    sim_framework = framework(work / "mpvkit/Libavformat.xcframework/ios-arm64_x86_64-simulator/Libavformat.framework", stage / "simulator/Libavformat.framework", [arm_sim, intel_sim])
    bundle = destination / "Libavformat.xcframework"
    if bundle.exists():
        shutil.rmtree(bundle)
    run(["xcodebuild", "-create-xcframework", "-framework", device_framework, "-framework", sim_framework, "-output", bundle])
    guarded = work / "guarded-probe"
    compile_probe(work, guarded, bundle)
    smoke(work, baseline, guarded)
    archive = destination / "Libavformat.xcframework.zip"
    zip_bundle(bundle, archive)
    record = {
        "schema": 1,
        "platform": "ios",
        "ffmpeg": "8.1.2",
        "relay_guard": "patina-relay-guard-v1",
        "sha256": prepare.digest(archive),
        "xcode": expected,
        "simulator_tested": "arm64",
        "device_tested": False,
    }
    (destination / "candidate.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()
