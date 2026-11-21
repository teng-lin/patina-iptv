"""Pinned libmpv runtimes for Linux and macOS (stdlib only).

The Windows runtime (`tools/mpv-runtime`) is the model: pinned sources from
its lock, an offline build, a check of the build's license configuration and
binary dependencies, and a dependency release holding the runtime with its
exact corresponding sources. This module adds the Linux and macOS variants
without touching any Windows input, so the Windows recipe digest and runtime
stay byte-for-byte unchanged.

Commands:
  lock     write the platform's source lock (shared entries of the Windows
           lock plus the platform's own, all SHA-256 pinned)
  verify   check a build's feature/license configuration headers
  package  validate a build and write the runtime, source and test-tool
           archives, runtime-<platform>.json and SHA256SUMS
  fetch    download (or take) a runtime archive, verify it against a
           runtime-<platform>.json manifest and unpack it for app packaging
  notices  copy an unpacked runtime's notices into an app package
  check-bundle
           check that a package bundles exactly the pinned libmpv and no
           system libmpv/FFmpeg
"""

import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent
BASE = HERE.parent / "mpv-runtime"
sys.path.insert(0, str(BASE))
import prepare  # noqa: E402  (the Windows pipeline's verified download/extract)

PLATFORMS = {
    "linux": {"arch": "x86_64", "library": "libmpv.so.2", "link": "libmpv.so"},
    "macos": {"arch": "arm64", "library": "libmpv.2.dylib", "link": "libmpv.dylib"},
}
HEADERS = ("client.h", "render.h", "render_gl.h", "stream_cb.h")

# The app's Linux packages target Ubuntu 24.04 (glibc 2.39); the macOS app
# declares macOS 14.0 (NSH's LSMinimumSystemVersion). A runtime may need less,
# never more.
GLIBC_LIMIT = (2, 39)
MACOS_LIMIT = (14, 0)

# Shared libraries the Linux runtime may load from the host (or that the
# AppImage deploys from the build system with their own notices and sources).
# Anything else means an undeclared dependency slipped into the build.
LINUX_SYSTEM_LIBRARIES = frozenset("""
libc.so.6 libm.so.6 libdl.so.2 libpthread.so.0 librt.so.1 libgcc_s.so.1
libstdc++.so.6 ld-linux-x86-64.so.2
libpulse.so.0 libasound.so.2 libpipewire-0.3.so.0
libva.so.2 libva-drm.so.2 libdrm.so.2
libssl.so.3 libcrypto.so.3 libfontconfig.so.1
""".split())
MACOS_SYSTEM_PREFIXES = ("/usr/lib/", "/System/Library/Frameworks/")

# Configuration evidence. FFmpeg's GPL, version 3 and nonfree switches must be
# off; mpv's GPL switch must be off. The rest are the playback features the
# app relies on: HLS/MPEG-TS over HTTP(S), the codecs of common IPTV streams,
# hardware decoding in copy mode, OpenGL rendering and the platform's audio.
_FFMPEG_COMMON = """
    CONFIG_NETWORK CONFIG_ZLIB CONFIG_LIBDAV1D
    CONFIG_AVCODEC CONFIG_AVFORMAT CONFIG_AVFILTER CONFIG_AVUTIL CONFIG_SWRESAMPLE CONFIG_SWSCALE
    CONFIG_HLS_DEMUXER CONFIG_MPEGTS_DEMUXER CONFIG_MOV_DEMUXER CONFIG_MATROSKA_DEMUXER CONFIG_RTSP_DEMUXER
    CONFIG_HTTP_PROTOCOL CONFIG_HTTPS_PROTOCOL CONFIG_TCP_PROTOCOL CONFIG_TLS_PROTOCOL CONFIG_CRYPTO_PROTOCOL
    CONFIG_H264_DECODER CONFIG_HEVC_DECODER CONFIG_MPEG2VIDEO_DECODER CONFIG_LIBDAV1D_DECODER
    CONFIG_AAC_DECODER CONFIG_MP2_DECODER CONFIG_AC3_DECODER CONFIG_EAC3_DECODER
    CONFIG_SUBRIP_DECODER CONFIG_WEBVTT_DECODER CONFIG_ASS_DECODER
    CONFIG_MPEG2VIDEO_ENCODER CONFIG_MP2_ENCODER CONFIG_RAWVIDEO_DEMUXER CONFIG_PCM_S16LE_DEMUXER
    CONFIG_HLS_MUXER CONFIG_MPEGTS_MUXER HAVE_PTHREADS CONFIG_PATINA_RELAY_GUARD_V1_PROTOCOL
""".split()
_MPV_COMMON = "HAVE_FFMPEG HAVE_GL HAVE_LIBASS HAVE_LIBPLACEBO HAVE_ZLIB HAVE_ICONV HAVE_LIBAVDEVICE".split()
REQUIRED_OFF = {"ffmpeg": ["CONFIG_GPL", "CONFIG_VERSION3", "CONFIG_NONFREE"], "mpv": ["HAVE_GPL", "HAVE_LIBCURL"]}
EXPECTED = {
    "linux": {
        "ffmpeg": _FFMPEG_COMMON + "CONFIG_OPENSSL CONFIG_VAAPI HAVE_VAAPI_DRM CONFIG_FFNVCODEC CONFIG_NVDEC".split(),
        "mpv": _MPV_COMMON + "HAVE_PULSE HAVE_ALSA HAVE_PIPEWIRE".split(),
    },
    "macos": {
        "ffmpeg": _FFMPEG_COMMON + "CONFIG_SECURETRANSPORT CONFIG_VIDEOTOOLBOX".split(),
        "mpv": _MPV_COMMON + "HAVE_COREAUDIO".split(),
    },
}


def names(platform):
    arch = PLATFORMS[platform]["arch"]
    return {
        "runtime": f"mpv-runtime-{platform}-{arch}.tar.gz",
        "sources": f"mpv-runtime-{platform}-sources.tar.gz",
        "tools": f"test-tools-{platform}-{arch}.tar.gz",
        "manifest": f"runtime-{platform}.json",
    }


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


# ------------------------------------------------------------------ lock

def resolve_lock(platform, selection_path=HERE / "sources.json"):
    """The platform's schema-1 source lock.

    Shared components are copied verbatim from the Windows lock, so a version
    or checksum can only change in one place. Platform-only components may not
    reuse a shared name: two pins for one component would drift apart.
    """
    selection_path = Path(selection_path)
    selection = json.loads(selection_path.read_text(encoding="utf-8"))
    if selection.get("schema") != 1 or not isinstance(selection.get("platforms"), dict):
        raise ValueError("Expected platform source selection schema 1")
    if platform not in PLATFORMS or platform not in selection["platforms"]:
        raise ValueError(f"Unknown runtime platform: {platform}")
    base, _ = prepare.read_lock(selection_path.parent / selection["base"])
    shared = {item["name"].casefold(): item for item in base["sources"]}
    entry = selection["platforms"][platform]
    chosen = []
    for name in entry.get("shared", []):
        if not isinstance(name, str) or name.casefold() not in shared:
            raise ValueError(f"Shared component is not in the base lock: {name!r}")
        chosen.append(shared[name.casefold()])
    extra = entry.get("sources", [])
    for item in extra:
        if not isinstance(item, dict) or str(item.get("name", "")).casefold() in shared:
            raise ValueError(f"Platform component duplicates a shared pin: {item.get('name')!r}")
    lock = {"schema": 1, "sources": chosen + list(extra)}
    # The same validation as every other lock: names, HTTPS, SHA-256, licences.
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "lock.json"
        path.write_text(json.dumps(lock), encoding="utf-8")
        prepare.read_lock(path)
    return lock


def write_lock(platform, output, selection_path=HERE / "sources.json"):
    lock = resolve_lock(platform, selection_path)
    Path(output).parent.mkdir(parents=True, exist_ok=True)
    Path(output).write_text(json.dumps(lock, indent=2) + "\n", encoding="utf-8")
    return lock


def recipe_files(platform):
    """Every script that shapes a platform's runtime, by its name in the recipe."""
    files = [("base/" + name, BASE / name) for name in
             ("prepare.py", "smoke.py", "relay_guard.py", "relay_guard_probe.c")]
    for path in sorted((BASE / "patches").glob("*.patch")):
        files.append(("base/patches/" + path.name, path))
    for path in sorted(HERE.glob("*.py")):
        if not path.name.startswith("test_"):
            files.append((path.name, path))
    for path in sorted((HERE / platform).rglob("*")):
        if path.is_file() and "__pycache__" not in path.parts:
            files.append((path.relative_to(HERE).as_posix(), path))
    return files


def recipe_digest(platform, lock):
    value = hashlib.sha256()
    inputs = [("sources.lock.json", canonical(lock)), ("platform", platform.encode())]
    inputs += [(name, path.read_bytes().replace(b"\r\n", b"\n")) for name, path in recipe_files(platform)]
    for name, content in sorted(inputs):
        value.update(name.encode() + b"\0" + str(len(content)).encode() + b"\0" + content)
    return value.hexdigest()


# ------------------------------------------------------- source archive

def _tar(output, files):
    """A gzip tar with stable metadata; `files` maps archive name -> (path or bytes, mode)."""
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    seen = set()
    with output.open("wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0) as compressed, \
            tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for name in sorted(files):
            prepare.relative_path(name)
            if name.casefold() in seen:
                raise ValueError(f"Duplicate archive path: {name}")
            seen.add(name.casefold())
            content, mode = files[name]
            data = content if isinstance(content, bytes) else Path(content).read_bytes()
            info = tarfile.TarInfo(name)
            info.size, info.mode, info.mtime = len(data), mode, 0
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            archive.addfile(info, io.BytesIO(data))


def bundle(platform, lock_path, work, output):
    """The exact corresponding source: archives, licenses, lock and build scripts.

    Same layout as the Windows bundle, so `prepare.py prepare --source-bundle`
    restores it offline.
    """
    lock, components = prepare.read_lock(lock_path)
    work = Path(work)
    if json.loads((work / "sources.lock.json").read_text(encoding="utf-8")) != lock:
        raise ValueError("Prepared source lock differs from requested lock")
    inventory = json.loads((work / "components.json").read_text(encoding="utf-8"))["components"]
    if sorted(row["name"] for row in inventory) != sorted(item["name"] for item, _ in components):
        raise ValueError("Component inventory differs from lock")
    files = {"sources.lock.json": (work / "sources.lock.json", 0o644),
             "components.json": (work / "components.json", 0o644)}
    for item, role in components:
        archive = work / "cache" / item["sha256"]
        if not archive.is_file() or prepare.digest(archive) != item["sha256"]:
            raise ValueError(f"Missing or modified source archive: {item['name']}")
        files[f"archives/{item['name']}-{item['sha256']}.archive"] = (archive, 0o644)
        record = next(row for row in inventory if row["name"] == item["name"])
        if record.get("role") != role or any(record.get(key) != value for key, value in item.items()):
            raise ValueError(f"Component inventory changed: {item['name']}")
        if [entry["path"] for entry in record["licenses"]] != item["license_files"]:
            raise ValueError(f"License inventory differs from lock: {item['name']}")
        root = (work / "sources" / item["name"]).resolve()
        for license_info in record["licenses"]:
            path = work / "sources" / item["name"] / license_info["path"]
            if not path.resolve().is_relative_to(root) or prepare.digest(path) != license_info["sha256"]:
                raise ValueError(f"License changed or unsafe: {path}")
            files[f"licenses/{item['name']}/{license_info['path']}"] = (path, 0o644)
    for name, path in recipe_files(platform) + [("README.md", HERE / "README.md"), ("sources.json", HERE / "sources.json")]:
        files["build-scripts/" + name] = (path, 0o755 if path.suffix == ".sh" else 0o644)
    _tar(output, files)


# ------------------------------------------------------------- evidence

def config_defines(*texts):
    defines = {}
    for text in texts:
        defines.update(re.findall(r"^#define\s+(\w+)\s+([01])\s*$", text, re.MULTILINE))
    return defines


def check_features(platform, component, defines):
    """Errors for features that must be on (the app needs them) or off (license)."""
    errors = []
    for expected, keys in (("1", EXPECTED[platform][component]), ("0", REQUIRED_OFF[component])):
        for key in keys:
            if defines.get(key) != expected:
                errors.append(f"{key}: expected {expected}, got {defines.get(key, 'missing')}")
    return errors


def elf_dynamic(text):
    """(NEEDED names, SONAME, has RPATH/RUNPATH) from `readelf -d`."""
    needed = re.findall(r"\(NEEDED\)\s+Shared library: \[([^\]]+)\]", text)
    soname = re.findall(r"\(SONAME\)\s+Library soname: \[([^\]]+)\]", text)
    paths = re.findall(r"\((?:RPATH|RUNPATH)\)", text)
    return needed, (soname[0] if soname else None), bool(paths)


def elf_exports(text):
    """Defined dynamic symbols from `nm -D --defined-only`."""
    exports = []
    for line in text.splitlines():
        fields = line.split()
        if len(fields) == 3 and fields[1] in "TtDdBbRrVvWwui" and len(fields[1]) == 1:
            exports.append(fields[2].split("@")[0])
    return exports


def glibc_minimum(text):
    """The newest GLIBC_x.y symbol version `objdump -T` lists as required."""
    versions = {tuple(int(part) for part in found.split("."))
                for found in re.findall(r"\bGLIBC_(\d+(?:\.\d+)+)\b", text)}
    if not versions:
        raise ValueError("No glibc symbol versions found")
    return max(versions)


def macho_references(text):
    """Load commands of `otool -L` (the first line names the file)."""
    return [line.strip().split(" (compatibility")[0] for line in text.splitlines()[1:] if line.strip()]


def macho_minimum(text):
    """LC_BUILD_VERSION's minos from `otool -l`."""
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if line.strip() == "cmd LC_BUILD_VERSION":
            for follow in lines[index + 1:index + 6]:
                fields = follow.split()
                if fields and fields[0] == "minos":
                    return tuple(int(part) for part in fields[1].split("."))
    raise ValueError("No LC_BUILD_VERSION minimum OS found")


def macho_uuid(text):
    """LC_UUID from `otool -l`; it survives install_name_tool and re-signing."""
    found = re.search(r"^\s*uuid ([0-9A-Fa-f-]{36})\s*$", text, re.MULTILINE)
    return found[1].upper() if found else None


def macho_rpaths(text):
    lines = text.splitlines()
    return [lines[index + 2].strip().split(" ")[1] for index, line in enumerate(lines)
            if line.strip() == "cmd LC_RPATH" and index + 2 < len(lines)]


def version_text(version):
    return ".".join(str(part) for part in version)


def binary_hashes(logs):
    hashes = {}
    for line in (logs / "binaries.sha256").read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64}) [ *](.+)", line)
        if not match or Path(match[2]).name in hashes:
            raise ValueError("Malformed build binary checksum evidence")
        hashes[Path(match[2]).name] = match[1]
    return hashes


def validate_build(platform, build):
    """Check the build's evidence; return facts recorded in the manifest."""
    build = Path(build)
    logs = build / "logs"
    library = PLATFORMS[platform]["library"]
    hashes = binary_hashes(logs)
    for binary in (library, "ffmpeg"):
        if hashes.get(binary) != prepare.digest(build / binary):
            raise ValueError(f"Build binary checksum mismatch: {binary}")
    errors = []
    smoke = logs / "smoke.txt"
    if not smoke.is_file() or not all(f"PASS {name}" in smoke.read_text(encoding="utf-8")
                                       for name in ("generated.ts", "generated.m3u8")):
        errors.append("No passing build-time playback smoke test (logs/smoke.txt)")
    license = re.search(r'^#define FFMPEG_LICENSE "([^"]*)"', (logs / "ffmpeg-config.h").read_text(encoding="utf-8"), re.MULTILINE)
    if not license or license[1] != "LGPL version 2.1 or later":
        errors.append(f"FFmpeg license is {license[1] if license else 'missing'}, expected LGPL version 2.1 or later")
    for component, headers in (("ffmpeg", ["ffmpeg-config.h", "ffmpeg-config_components.h"]), ("mpv", ["mpv-config.h"])):
        defines = config_defines(*((logs / name).read_text(encoding="utf-8") for name in headers))
        errors += [f"{component}: {error}" for error in check_features(platform, component, defines)]
    facts = {}
    if platform == "linux":
        needed, soname, has_paths = elf_dynamic((logs / "libmpv-dynamic.txt").read_text(encoding="utf-8"))
        if soname != library:
            errors.append(f"libmpv SONAME is {soname}, expected {library}")
        if has_paths:
            errors.append("libmpv carries an RPATH/RUNPATH from the build")
        tool_needed, _, _ = elf_dynamic((logs / "ffmpeg-dynamic.txt").read_text(encoding="utf-8"))
        for binary, dependencies in ((library, needed), ("ffmpeg", tool_needed)):
            errors += [f"{binary} needs undeclared library {name}" for name in dependencies
                       if name not in LINUX_SYSTEM_LIBRARIES]
        exports = elf_exports((logs / "libmpv-exports.txt").read_text(encoding="utf-8"))
        foreign = sorted(name for name in exports if not name.startswith("mpv_") and name not in ("_init", "_fini"))
        if not exports or foreign:
            errors.append(f"libmpv exports symbols beyond the mpv API: {foreign[:10]}")
        minimum = glibc_minimum((logs / "libmpv-symbols.txt").read_text(encoding="utf-8"))
        if minimum > GLIBC_LIMIT:
            errors.append(f"libmpv needs glibc {version_text(minimum)}, newer than {version_text(GLIBC_LIMIT)}")
        build_id = re.search(r"Build ID: ([0-9a-f]{16,})", (logs / "libmpv-notes.txt").read_text(encoding="utf-8"))
        if not build_id:
            errors.append("libmpv has no GNU build ID")
        else:
            facts["library_build_id"] = build_id[1]
        facts["glibc_minimum"] = version_text(minimum)
        facts["needed"] = sorted(needed)
    else:
        for binary in (library, "ffmpeg"):
            references = macho_references((logs / f"{binary}-otool-libraries.txt").read_text(encoding="utf-8"))
            if not references:
                errors.append(f"No load command evidence for {binary}")
            for reference in references:
                own = binary == library and reference == "@rpath/" + library
                if not own and not reference.startswith(MACOS_SYSTEM_PREFIXES):
                    errors.append(f"{binary} references a non-system library: {reference}")
            commands = (logs / f"{binary}-otool-loads.txt").read_text(encoding="utf-8")
            minimum = macho_minimum(commands)
            if minimum > MACOS_LIMIT:
                errors.append(f"{binary} needs macOS {version_text(minimum)}, newer than {version_text(MACOS_LIMIT)}")
            if macho_rpaths(commands):
                errors.append(f"{binary} carries an LC_RPATH from the build")
            if binary == library:
                facts["macos_minimum"] = version_text(minimum)
                facts["library_uuid"] = macho_uuid(commands)
                if not facts["library_uuid"]:
                    errors.append("libmpv has no LC_UUID")
        exports = [line.split()[-1] for line in (logs / "libmpv-exports.txt").read_text(encoding="utf-8").splitlines()
                   if line.strip()]
        foreign = sorted(name for name in exports if not name.startswith("_mpv_"))
        if not exports or foreign:
            errors.append(f"libmpv exports symbols beyond the mpv API: {foreign[:10]}")
    if errors:
        raise ValueError("Runtime build check failed:\n" + "\n".join(errors))
    return facts


# -------------------------------------------------------------- package

def package(platform, lock_path, work, build, output, repository, selection_path=HERE / "sources.json"):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("Expected GitHub repository owner/name")
    work, build, output = Path(work), Path(build), Path(output)
    output.mkdir(parents=True, exist_ok=True)
    lock, _ = prepare.read_lock(lock_path)
    if lock != resolve_lock(platform, selection_path):
        raise ValueError("Lock differs from the committed platform source selection")
    files = names(platform)
    library = PLATFORMS[platform]["library"]
    source_archive = output / files["sources"]
    bundle(platform, lock_path, work, source_archive)
    facts = validate_build(platform, build)
    recipe = recipe_digest(platform, lock)
    tag = f"mpv-runtime-{platform}-{recipe}"
    base = f"https://github.com/{repository}/releases/download/{tag}/"
    source_sha = prepare.digest(source_archive)
    components = json.loads((work / "components.json").read_text(encoding="utf-8"))
    components["build"] = {"platform": platform, "arch": PLATFORMS[platform]["arch"], "tag": tag,
                           "recipe_sha256": recipe, "library": library,
                           "library_sha256": prepare.digest(build / library),
                           "ffmpeg_sha256": prepare.digest(build / "ffmpeg"),
                           "source_url": base + files["sources"], "source_sha256": source_sha, **facts}
    content = {
        f"lib/{library}": (build / library, 0o755),
        "components.json": ((json.dumps(components, indent=2) + "\n").encode(), 0o644),
        "sources.lock.json": (work / "sources.lock.json", 0o644),
        "SOURCE.txt": ((f"Patina {platform} libmpv runtime\n\n"
                        "libmpv with FFmpeg, libass, libplacebo and the libraries listed in NOTICE.txt\n"
                        "linked into it. Exact corresponding sources, licenses and build scripts:\n"
                        f"{base + files['sources']}\nSHA-256: {source_sha}\n\nDependency release: {tag}\n").encode(), 0o644),
    }
    for header in HEADERS:
        content[f"include/mpv/{header}"] = (build / "include/mpv" / header, 0o644)
    notices = ["Native dependency notices", "",
               f"{library} is LGPL software built from the sources below; FFmpeg was configured",
               "with --disable-gpl --disable-version3 --disable-nonfree (see build-config).",
               "You may replace it with a build of your own; SOURCE.txt names the retained sources.", ""]
    for item in components["components"]:
        notices += [f"{item['name']} {item['version']} ({item['role']})", f"Source: {item['url']}"]
        for license_file in item["licenses"]:
            name = license_file["path"]
            content[f"licenses/{item['name']}/{name}"] = (work / "sources" / item["name"] / name, 0o644)
            notices.append(f"  licenses/{item['name']}/{name}")
        notices.append("")
    content["NOTICE.txt"] = (("\n".join(notices) + "\n").encode(), 0o644)
    for path in sorted((build / "logs").iterdir()):
        if path.is_file():
            content["build-config/" + path.name] = (path, 0o644)
    _tar(output / files["runtime"], content)
    # ffmpeg generates the smoke tests' media; it never enters an app package.
    _tar(output / files["tools"], {"bin/ffmpeg": (build / "ffmpeg", 0o755)})
    manifest = {"schema": 1, "platform": platform, "arch": PLATFORMS[platform]["arch"],
                "url": base + files["runtime"], "sha256": prepare.digest(output / files["runtime"]),
                "library": library, "library_sha256": prepare.digest(build / library),
                "source_url": base + files["sources"], "source_sha256": source_sha, "tag": tag,
                **{key: value for key, value in facts.items() if key != "needed"}}
    (output / files["manifest"]).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    (output / "SHA256SUMS").write_text("".join(
        f"{prepare.digest(output / name)}  {name}\n" for name in (files["runtime"], files["sources"], files["manifest"])),
        encoding="utf-8")
    print(json.dumps(manifest, indent=2))
    return manifest


# ---------------------------------------------------------------- fetch

def read_manifest(platform, path):
    manifest = json.loads(Path(path).read_text(encoding="utf-8"))
    hexes = ("sha256", "library_sha256", "source_sha256")
    if (manifest.get("schema") != 1 or manifest.get("platform") != platform
            or manifest.get("library") != PLATFORMS[platform]["library"]
            or any(not re.fullmatch(r"[0-9a-f]{64}", str(manifest.get(key, ""))) for key in hexes)
            or not str(manifest.get("url", "")).startswith("https://")
            or not str(manifest.get("source_url", "")).startswith("https://")
            or not re.fullmatch(rf"mpv-runtime-{platform}-[0-9a-f]{{64}}", str(manifest.get("tag", "")))):
        raise ValueError(f"Invalid {platform} runtime manifest: {path}")
    return manifest


def fetch(platform, manifest_path, destination, archive=None):
    """Unpack the pinned runtime into `destination`; every byte is hash-checked."""
    manifest_path = Path(manifest_path)
    if not manifest_path.is_file():
        raise SystemExit(
            f"No {platform} libmpv runtime is pinned ({manifest_path} is missing). Dispatch "
            f"'Build retained libmpv dependencies' (mpv-runtime.yml) for {platform}, review the "
            f"release and commit its {names(platform)['manifest']} to tools/mpv-runtime-unix/.")
    manifest = read_manifest(platform, manifest_path)
    destination = Path(destination).resolve()
    cache = destination.parent / (destination.name + "-cache")
    if archive is None:
        archive = prepare.download({"name": "runtime", "url": manifest["url"], "sha256": manifest["sha256"]}, cache)
    elif prepare.digest(archive) != manifest["sha256"]:
        raise ValueError(f"Runtime archive checksum mismatch: {archive}")
    if destination.is_symlink():
        raise ValueError(f"Refusing a symlinked runtime destination: {destination}")
    if destination.exists():
        shutil.rmtree(destination)
    prepare.extract(archive, destination)
    library = destination / "lib" / manifest["library"]
    if not library.is_file() or prepare.digest(library) != manifest["library_sha256"]:
        raise ValueError(f"Runtime library checksum mismatch: {library}")
    components = json.loads((destination / "components.json").read_text(encoding="utf-8"))
    if components.get("build", {}).get("library_sha256") != manifest["library_sha256"] \
            or components["build"].get("source_sha256") != manifest["source_sha256"]:
        raise ValueError("Runtime component inventory does not match the manifest")
    # What the app's link step and build script look for.
    (destination / "lib" / PLATFORMS[platform]["link"]).symlink_to(manifest["library"])
    pkgconfig = destination / "lib/pkgconfig"
    pkgconfig.mkdir()
    (pkgconfig / "mpv.pc").write_text(
        f"prefix={destination}\nlibdir=${{prefix}}/lib\nincludedir=${{prefix}}/include\n\n"
        "Name: mpv\nDescription: Patina's pinned libmpv runtime\nVersion: 2\n"
        "Libs: -L${libdir} -lmpv\nCflags: -I${includedir}\n", encoding="utf-8")
    (destination / "runtime.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(destination)
    return manifest


# ------------------------------------------------- app package helpers

NOTICE_FILES = ("NOTICE.txt", "SOURCE.txt", "components.json", "sources.lock.json")
# Libraries that only a system libmpv/FFmpeg would bring into a package.
SYSTEM_MEDIA = re.compile(r"^lib(av(codec|device|filter|format|util)|sw(resample|scale)|postproc|placebo|ass|dav1d|mpv)\b")


def install_notices(runtime_dir, output):
    """Copy the runtime's notices, licenses, source pointer and build configuration."""
    runtime_dir, output = Path(runtime_dir), Path(output)
    if output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True)
    for name in NOTICE_FILES:
        shutil.copyfile(runtime_dir / name, output / name)
    for directory in ("licenses", "build-config"):
        shutil.copytree(runtime_dir / directory, output / directory)


def binary_identity(platform, path):
    """The linker's identity of a library: GNU build ID or Mach-O UUID.

    Packaging rewrites load paths and signatures, so file hashes change; these
    identities do not.
    """
    if platform == "linux":
        notes = subprocess.run(["readelf", "--notes", str(path)], check=True, capture_output=True, text=True).stdout
        found = re.search(r"Build ID: ([0-9a-f]{16,})", notes)
        return found[1] if found else None
    return macho_uuid(subprocess.run(["otool", "-l", str(path)], check=True, capture_output=True, text=True).stdout)


IDENTITY = {"linux": "library_build_id", "macos": "library_uuid"}


def check_bundle(platform, runtime_dir, libraries, identity=binary_identity):
    """A package's library directory holds this runtime's libmpv and no other media library."""
    manifest = read_manifest(platform, Path(runtime_dir) / "runtime.json")
    libraries = Path(libraries)
    bundled = libraries / manifest["library"]
    if not bundled.is_file():
        raise ValueError(f"The package does not bundle {manifest['library']}")
    expected = manifest.get(IDENTITY[platform])
    if not expected or identity(platform, bundled) != expected:
        raise ValueError(f"{bundled} is not the pinned runtime's library ({IDENTITY[platform]} differs)")
    foreign = sorted(path.name for path in libraries.iterdir()
                     if SYSTEM_MEDIA.match(path.name) and path.name != manifest["library"])
    if foreign:
        raise ValueError(f"System media libraries were bundled beside the pinned runtime: {foreign}")
    return manifest


# ------------------------------------------------------------------ CLI

def main(arguments=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    lock = commands.add_parser("lock")
    lock.add_argument("--platform", choices=PLATFORMS, required=True)
    lock.add_argument("--output", type=Path, required=True)
    verify = commands.add_parser("verify")
    verify.add_argument("--platform", choices=PLATFORMS, required=True)
    verify.add_argument("--component", choices=sorted(REQUIRED_OFF), required=True)
    verify.add_argument("headers", type=Path, nargs="+")
    pack = commands.add_parser("package")
    pack.add_argument("--platform", choices=PLATFORMS, required=True)
    for name in ("lock", "work", "build", "output"):
        pack.add_argument("--" + name, type=Path, required=True)
    pack.add_argument("--repository", required=True)
    get = commands.add_parser("fetch")
    get.add_argument("--platform", choices=PLATFORMS, required=True)
    get.add_argument("--destination", type=Path, required=True)
    get.add_argument("--manifest", type=Path, help="Default: the committed runtime-<platform>.json")
    get.add_argument("--archive", type=Path, help="A local runtime archive instead of the manifest's URL")
    notices = commands.add_parser("notices")
    notices.add_argument("--runtime", type=Path, required=True)
    notices.add_argument("--output", type=Path, required=True)
    check = commands.add_parser("check-bundle")
    check.add_argument("--platform", choices=PLATFORMS, required=True)
    check.add_argument("--runtime", type=Path, required=True)
    check.add_argument("--libraries", type=Path, required=True)
    args = parser.parse_args(arguments)
    if args.command == "lock":
        write_lock(args.platform, args.output)
    elif args.command == "verify":
        defines = config_defines(*(path.read_text(encoding="utf-8") for path in args.headers))
        errors = check_features(args.platform, args.component, defines)
        if errors:
            raise SystemExit(f"{args.component} required feature check failed:\n" + "\n".join(errors))
        print(f"{args.component}: verified required playback features and LGPL build configuration")
    elif args.command == "package":
        package(args.platform, args.lock, args.work, args.build, args.output, args.repository)
    elif args.command == "fetch":
        fetch(args.platform, args.manifest or HERE / names(args.platform)["manifest"], args.destination, args.archive)
    elif args.command == "notices":
        install_notices(args.runtime, args.output)
    else:
        manifest = check_bundle(args.platform, args.runtime, args.libraries)
        print(f"{args.libraries}/{manifest['library']} is the pinned runtime {manifest['tag']}")


if __name__ == "__main__":
    main()
