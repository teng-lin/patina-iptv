"""Package a native build with its verified sources and complete notices."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import zipfile

import prepare


RUNTIME = "mpv-runtime-windows-x86_64.zip"
SOURCES = "mpv-runtime-sources.tar.gz"

# Explicit Windows system dependencies; never silently ship a runtime that
# requires an undeclared codec, compiler runtime, or third-party DLL.
SYSTEM_DLLS = frozenset("""
advapi32.dll avicap32.dll avrt.dll bcrypt.dll cfgmgr32.dll combase.dll crypt32.dll
d3d11.dll d3d12.dll d3dcompiler_47.dll dbghelp.dll dnsapi.dll dwmapi.dll
dxgi.dll dxva2.dll gdi32.dll imm32.dll iphlpapi.dll kernel32.dll kernelbase.dll
mf.dll mfplat.dll mfreadwrite.dll mfuuid.dll mmdevapi.dll mpr.dll msvcrt.dll
mswsock.dll ncrypt.dll normaliz.dll ntdll.dll ole32.dll oleaut32.dll opengl32.dll
powrprof.dll propsys.dll psapi.dll rpcrt4.dll secur32.dll setupapi.dll shcore.dll shell32.dll
shlwapi.dll strmiids.dll synchronization.dll ucrtbase.dll user32.dll userenv.dll
usp10.dll uuid.dll uxtheme.dll version.dll winhttp.dll wininet.dll winmm.dll
winnsi.dll winspool.drv wintrust.dll ws2_32.dll wtsapi32.dll
""".split())


def validate_build(build):
    logs = build / "logs"
    hashes = {}
    for line in (logs / "binaries.sha256").read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64}) [ *](.+)", line)
        if not match:
            raise ValueError("Malformed build binary checksum evidence")
        name = Path(match[2]).name
        if name in hashes:
            raise ValueError("Duplicate build binary checksum evidence")
        hashes[name] = match[1]
    for binary, report in [("libmpv-2.dll", "libmpv-imports.txt"), ("ffmpeg.exe", "ffmpeg-imports.txt")]:
        if hashes.get(binary) != prepare.digest(build / binary):
            raise ValueError(f"Build binary checksum mismatch: {binary}")
        imports = re.findall(r"^\s*Name:\s*(\S+)\s*$", (logs / report).read_text(encoding="utf-8"), re.MULTILINE)
        if not imports:
            raise ValueError(f"Missing DLL import evidence: {binary}")
        for name in imports:
            normalized = name.lower()
            if normalized not in SYSTEM_DLLS and not re.fullmatch(r"api-ms-win-[a-z0-9-]+-l[0-9]+-[0-9]+-[0-9]+\.dll", normalized):
                raise ValueError(f"Non-system DLL import in {binary}: {name}")


def recipe_digest(lock_path):
    scripts = Path(__file__).resolve().parent
    value = hashlib.sha256()
    inputs = [("sources.lock.json", json.dumps(prepare.read_lock(lock_path)[0], sort_keys=True, separators=(",", ":")).encode())]
    for path in sorted(scripts.rglob("*")):
        if path.is_file() and "__pycache__" not in path.parts and path.name != "sources.lock.json" and (path.suffix in {".py", ".sh", ".ps1", ".cmake", ".patch", ".c", ".h"} or path.name in {"Dockerfile", "Makefile"}):
            inputs.append((path.relative_to(scripts).as_posix(), path.read_bytes().replace(b"\r\n", b"\n")))
    for name, content in sorted(inputs):
        value.update(name.encode() + b"\0" + str(len(content)).encode() + b"\0" + content)
    return value.hexdigest()


def make_zip(path, files):
    names = set()
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, content in sorted(files.items()):
            prepare.relative_path(name)
            if name.casefold() in names:
                raise ValueError(f"Duplicate distribution path: {name}")
            names.add(name.casefold())
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, content)


def package(lock, work, build, output, repository):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("Expected GitHub repository owner/name")
    output.mkdir(parents=True, exist_ok=True)
    # Regenerate the corresponding-source archive from verified cached bytes;
    # bundle also revalidates the lock, component inventory and license files.
    source_archive = output / SOURCES
    prepare.bundle(lock, work, source_archive)
    tag = "mpv-runtime-" + recipe_digest(lock)
    base = f"https://github.com/{repository}/releases/download/{tag}/"
    source_url = base + SOURCES
    dll, ffmpeg = build / "libmpv-2.dll", build / "ffmpeg.exe"
    for binary in (dll, ffmpeg):
        if not binary.is_file():
            raise ValueError(f"Missing Windows binary: {binary}")
        with binary.open("rb") as stream:
            if stream.read(2) != b"MZ":
                raise ValueError(f"Invalid Windows binary: {binary}")
    validate_build(build)
    components = json.loads((work / "components.json").read_text(encoding="utf-8"))
    components["build"] = {"tag": tag, "recipe_sha256": recipe_digest(lock), "dll_sha256": prepare.digest(dll), "ffmpeg_sha256": prepare.digest(ffmpeg), "source_url": source_url, "source_sha256": prepare.digest(source_archive)}
    files = {
        "libmpv-2.dll": dll.read_bytes(),
        "components.json": (json.dumps(components, indent=2) + "\n").encode(),
        "sources.lock.json": (work / "sources.lock.json").read_bytes(),
        "SOURCE.txt": ("Patina Windows libmpv runtime\n\nExact corresponding sources, toolchain inputs, licenses and build scripts:\n" + source_url + "\nSHA-256: " + prepare.digest(source_archive) + "\n\nDependency release: " + tag + "\n").encode(),
    }
    notices = ["Native dependency notices", "", "The accompanying licenses directory contains the exact notices recorded below.", "See SOURCE.txt for the retained corresponding-source archive.", ""]
    for item in components["components"]:
        notices.append(f"{item['name']} {item['version']} ({item['role']})")
        notices.append(f"Source: {item['url']}")
        for license_file in item["licenses"]:
            name = license_file["path"]
            files[f"licenses/{item['name']}/{name}"] = (work / "sources" / item["name"] / name).read_bytes()
            notices.append(f"  licenses/{item['name']}/{name}")
        notices.append("")
    files["NOTICE.txt"] = ("\n".join(notices) + "\n").encode()
    logs = build / "logs"
    required = ["cross.ini", "ffmpeg-config.h", "ffmpeg-config.mak", "mpv-options.json", "libmpv-imports.txt", "ffmpeg-imports.txt"]
    for name in required:
        if not (logs / name).is_file():
            raise ValueError(f"Missing build configuration evidence: {name}")
    for path in sorted(logs.iterdir()):
        if path.is_file():
            files["build-config/" + path.name] = path.read_bytes()
    make_zip(output / RUNTIME, files)
    # ffmpeg is a test executable, intentionally excluded from the app runtime.
    make_zip(output / "test-tools.zip", {"ffmpeg.exe": ffmpeg.read_bytes()})
    manifest = {"schema": 1, "url": base + RUNTIME, "sha256": prepare.digest(output / RUNTIME), "dll_sha256": prepare.digest(dll), "source_url": source_url, "source_sha256": prepare.digest(source_archive), "tag": tag}
    (output / "runtime.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    (output / "SHA256SUMS").write_text("".join(f"{prepare.digest(output / name)}  {name}\n" for name in [RUNTIME, SOURCES, "runtime.json"]), encoding="utf-8")
    print(json.dumps(manifest, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lock", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--build", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repository", required=True)
    args = parser.parse_args()
    package(args.lock, args.work, args.build, args.output, args.repository)


if __name__ == "__main__":
    main()
