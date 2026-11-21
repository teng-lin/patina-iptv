#!/usr/bin/env python3
"""Makes a macOS .app self-contained: copies the dylibs its executable
links outside the system into Contents/Frameworks, points every reference at
them (@rpath/<name>, the executable's rpath @executable_path/../Frameworks,
each dylib's @loader_path), drops the rpaths that lead back to the build
machine, and signs it all again ad hoc (`codesign -s -`).

libmpv comes from Patina's pinned runtime (`--runtime`, unpacked by
`tools/mpv-runtime-unix/runtime.py fetch`): one libmpv.2.dylib with FFmpeg and
the rest linked in, which references only the system. Its notices and the
address of its exact sources go to Contents/Resources/notices/mpv-runtime,
and the bundled copy is checked to be that runtime's (by Mach-O UUID). Any
other dylib the executable links (normally none) is taken from Homebrew as
before, with the notices and source records described below.

Usage (after `nsh package --target macos`):
  python3 -I tools/bundle-macos.py --runtime target/mpv-runtime-macos target/package/Patina.app
  python3 -I tools/bundle-macos.py target/package/Patina.app   (Homebrew libmpv, development only)

Needs Xcode's command line tools (otool, install_name_tool, codesign). It
fails when a dependency cannot be found, and checks at the end that no
reference outside the system and the bundle is left. Homebrew builds its
bottles for the macOS it runs on: the app's LSMinimumSystemVersion is raised
to the newest minimum of what it bundles, so an older Mac says the app needs
a newer macOS rather than failing at launch (a release builds libmpv for an
older deployment target instead).

Beside each library's license texts it records where its source comes from
(notices/native/sources.json, SOURCE.txt and the installed formulae), and
fails when a bundled Homebrew keg does not name a checksummed source.
"""

import os
import json
import hashlib
import importlib.util
import re
import tarfile
import tempfile
import urllib.request
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys

SYSTEM = ("/usr/lib/", "/System/")

# Shared with the release job that downloads what this script records.
_spec = importlib.util.spec_from_file_location("native_sources", Path(__file__).parent / "ci/native-sources.py")
native_sources = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(native_sources)
# The pinned runtime's manifest checks and notices.
_spec = importlib.util.spec_from_file_location("media_runtime", Path(__file__).parent / "mpv-runtime-unix/runtime.py")
media_runtime = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(media_runtime)

# The unpacked pinned runtime, when given: its lib directory serves @rpath/
# references the executable cannot resolve itself (it has no rpath yet).
RUNTIME = None


def run(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout


def refs(path):
    """The dylibs `path` links (otool -L), its own id aside."""
    lines = run("otool", "-L", path).splitlines()[1:]
    out = [line.strip().split(" (compatibility")[0] for line in lines]
    own = dylib_id(path)
    return [r for r in out if r != own]


def dylib_id(path):
    lines = run("otool", "-D", path).splitlines()[1:]
    return lines[0].strip() if lines else None


def rpaths(path):
    lines = run("otool", "-l", path).splitlines()
    return [
        lines[i + 2].strip().split(" ")[1]
        for i, line in enumerate(lines)
        if line.strip() == "cmd LC_RPATH"
    ]


def resolve(ref, loader, exe_dir):
    """The file `ref` names for `loader`, or None."""
    here = os.path.dirname(loader)
    if ref.startswith("@loader_path/"):
        candidates = [os.path.join(here, ref[len("@loader_path/"):])]
    elif ref.startswith("@executable_path/"):
        candidates = [os.path.join(exe_dir, ref[len("@executable_path/"):])]
    elif ref.startswith("@rpath/"):
        name = ref[len("@rpath/"):]
        candidates = [
            os.path.join(rp.replace("@loader_path", here).replace("@executable_path", exe_dir), name)
            for rp in rpaths(loader)
        ]
    else:
        candidates = [ref]
    for c in candidates:
        if os.path.exists(c):
            return os.path.realpath(c)
    if RUNTIME is not None and ref.startswith("@rpath/"):
        pinned = RUNTIME / "lib" / ref[len("@rpath/"):]
        if pinned.is_file():
            return os.path.realpath(pinned)
    return None


def from_runtime(real):
    return RUNTIME is not None and Path(real).is_relative_to(RUNTIME.resolve())


def minimum_os(path):
    """The macOS a Mach-O file needs (LC_BUILD_VERSION minos), as a tuple."""
    lines = run("otool", "-l", path).splitlines()
    for i, line in enumerate(lines):
        if line.strip() == "cmd LC_BUILD_VERSION":
            for follow in lines[i + 1 : i + 6]:
                parts = follow.split()
                if parts and parts[0] == "minos":
                    return tuple(int(x) for x in parts[1].split("."))
    return (0,)


def external(ref):
    return not ref.startswith(SYSTEM)


def glib_source_notices(prefix, destination):
    """Use the installed formula's exact source checksum when its bottle omits texts."""
    formula = prefix / ".brew/glib.rb"
    content = formula.read_text()
    url_match = re.search(r'^\s*url "([^"\n]+)"\s*$', content, re.M)
    sha_match = re.search(r'^\s*sha256 "([0-9a-f]{64})"\s*$', content, re.M)
    version = prefix.name.split("_", 1)[0]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("Unsupported installed GLib version")
    expected = f"https://download.gnome.org/sources/glib/{'.'.join(version.split('.')[:2])}/glib-{version}.tar.xz"
    if not url_match or url_match[1] != expected or not sha_match:
        raise ValueError("Installed GLib formula lacks matching source URL and checksum")
    with tempfile.TemporaryFile() as archive:
        digest = hashlib.sha256()
        total = 0
        with urllib.request.urlopen(expected, timeout=60) as response:
            while chunk := response.read(1024 * 1024):
                total += len(chunk)
                if total > 64 * 1024 * 1024:
                    raise ValueError("GLib source archive exceeds limit")
                digest.update(chunk)
                archive.write(chunk)
        if digest.hexdigest() != sha_match[1]:
            raise ValueError("GLib source checksum mismatch")
        archive.seek(0)
        copied = 0
        with tarfile.open(fileobj=archive, mode="r:xz") as source:
            for member in source:
                path = Path(member.name)
                if not member.isfile() or path.is_absolute() or ".." in path.parts:
                    continue
                if not path.parts or path.parts[0] != f"glib-{version}":
                    continue
                if not (path.name.upper().startswith(("COPYING", "LICENSE", "LICENCE", "NOTICE", "COPYRIGHT"))
                        or "LICENSES" in path.parts):
                    continue
                if member.size > 1024 * 1024 or copied >= 200:
                    raise ValueError("GLib source notices exceed limit")
                target = destination / "source" / Path(*path.parts[1:])
                target.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(member) as stream:
                    target.write_bytes(stream.read())
                copied += 1
        if not copied:
            raise ValueError("GLib source has no license notices")
    shutil.copyfile(formula, destination / "source-formula.rb")
    (destination / "source.json").write_text(json.dumps({"url": expected, "sha256": sha_match[1]}, indent=2) + "\n")


def record_sources(prefixes, notices, version):
    """What each bundled keg was built from, and where a user finds that source.

    Written before signing: the notices are sealed by the app's signature.
    The release job downloads exactly this list (tools/ci/native-sources.py).
    """
    components = []
    for prefix in sorted(prefixes):
        component, formula = native_sources.homebrew_component(prefix)
        destination = notices / component["recipe"]
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(formula)
        components.append(component)
    description = native_sources.manifest("macos", version, components)
    (notices / "sources.json").write_text(json.dumps(description, indent=2) + "\n")
    (notices / "SOURCE.txt").write_text(native_sources.source_note("macos", version))
    return description


def main():
    global RUNTIME
    arguments = sys.argv[1:]
    if len(arguments) == 3 and arguments[0] == "--runtime":
        RUNTIME = Path(arguments[1]).resolve()
        media_runtime.read_manifest("macos", RUNTIME / "runtime.json")
        arguments = arguments[2:]
    if len(arguments) != 1 or not arguments[0].rstrip("/").endswith(".app"):
        sys.exit(__doc__)
    app = os.path.abspath(arguments[0].rstrip("/"))
    info = plistlib.load(open(os.path.join(app, "Contents", "Info.plist"), "rb"))
    exe = os.path.join(app, "Contents", "MacOS", info["CFBundleExecutable"])
    exe_dir = os.path.dirname(exe)
    frameworks = os.path.join(app, "Contents", "Frameworks")
    bundled = os.path.realpath(frameworks) + "/"

    # Every external dylib, by real path, with the name it gets in the bundle
    # (its install name's: libavcodec.62.dylib, not the versioned file).
    names = {}
    todo = [exe]
    seen = set()
    while todo:
        f = todo.pop()
        if f in seen:
            continue
        seen.add(f)
        for ref in refs(f):
            if not external(ref):
                continue
            real = resolve(ref, f, exe_dir)
            if real is None:
                sys.exit(f"cannot find {ref} (linked by {f})")
            if real.startswith(bundled):
                continue
            if real not in names:
                names[real] = os.path.basename(dylib_id(real) or real)
                todo.append(real)
    taken = {}
    for real, name in names.items():
        if taken.setdefault(name, real) != real:
            sys.exit(f"two dylibs named {name}: {taken[name]} and {real}")

    # Homebrew installs upstream notices in each versioned formula prefix.
    # Keep those texts and the bottle receipt alongside the bundled libraries.
    notices = Path(app) / "Contents/Resources/notices/native"
    inventory = []
    prefixes = set()
    for real in names:
        if from_runtime(real):
            continue
        notices.mkdir(parents=True, exist_ok=True)
        library = Path(real)
        prefix = next((p for p in library.parents if (p / "INSTALL_RECEIPT.json").exists()), None)
        if prefix is None:
            sys.exit(f"no component receipt for bundled library {library.name}")
        inventory.append({"library": names[real], "component": prefix.parent.name, "version": prefix.name})
        if prefix in prefixes:
            continue
        prefixes.add(prefix)
        destination = notices / (prefix.parent.name + "-" + prefix.name)
        destination.mkdir(exist_ok=True)
        shutil.copyfile(prefix / "INSTALL_RECEIPT.json", destination / "receipt.json")
        texts = [p for p in prefix.rglob("*") if p.is_file() and
                 p.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT"))]
        if not texts:
            if prefix.parent.name == "glib":
                glib_source_notices(prefix, destination)
            else:
                sys.exit(f"no upstream license text installed for {prefix.parent.name} {prefix.name}")
        for text in texts:
            target = destination / text.relative_to(prefix)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(text, target)
    if inventory:
        (notices / "components.json").write_text(json.dumps(inventory, indent=2) + "\n")
        try:
            record_sources(prefixes, notices, info["CFBundleShortVersionString"])
        except (OSError, ValueError) as error:
            sys.exit(f"cannot record the source of a bundled library: {error}")
    if RUNTIME is not None:
        if not any(from_runtime(real) for real in names):
            sys.exit(f"the executable does not link the pinned runtime's libmpv in {RUNTIME}")
        media_runtime.install_notices(RUNTIME, notices.parent / "mpv-runtime")

    os.makedirs(frameworks, exist_ok=True)
    copies = {}
    for real, name in names.items():
        dest = os.path.join(frameworks, name)
        shutil.copyfile(real, dest)
        os.chmod(dest, 0o644)
        copies[real] = dest

    def rewrite(path, is_exe):
        args = []
        for ref in refs(path):
            if not external(ref):
                continue
            real = resolve(ref, path if is_exe else [k for k, v in copies.items() if v == path][0], exe_dir)
            if real in names:
                args += ["-change", ref, "@rpath/" + names[real]]
        for rp in rpaths(path):
            if rp not in ("@executable_path/../Frameworks", "@loader_path"):
                args += ["-delete_rpath", rp]
        if not is_exe:
            args += ["-id", "@rpath/" + os.path.basename(path)]
        if args:
            run("install_name_tool", *args, path)
        want = "@executable_path/../Frameworks" if is_exe else "@loader_path"
        if want not in rpaths(path):
            run("install_name_tool", "-add_rpath", want, path)

    # The references are resolved against the originals (their rpaths and
    # loader paths), before those are rewritten.
    for dest in copies.values():
        rewrite(dest, False)
    rewrite(exe, True)

    # The macOS the bundle needs, in its Info.plist.
    plist_path = os.path.join(app, "Contents", "Info.plist")
    declared = tuple(int(x) for x in info.get("LSMinimumSystemVersion", "0").split("."))
    needed = max(minimum_os(f) for f in [exe, *copies.values()])
    if needed > declared:
        info["LSMinimumSystemVersion"] = ".".join(str(x) for x in needed)
        with open(plist_path, "wb") as f:
            plistlib.dump(info, f)

    # The bundled libmpv is the pinned runtime's, and no Homebrew FFmpeg/libmpv
    # came along beside it.
    if RUNTIME is not None:
        try:
            media_runtime.check_bundle("macos", RUNTIME, Path(frameworks))
        except ValueError as error:
            sys.exit(str(error))

    # Signatures: the dylibs, then the app around them.
    for dest in sorted(copies.values()):
        run("codesign", "--force", "--sign", "-", dest)
    run("codesign", "--force", "--sign", "-", app)

    # Nothing outside the system and the bundle is left.
    for f in [exe, *copies.values()]:
        for ref in refs(f):
            if external(ref):
                real = resolve(ref, f, exe_dir)
                if not ref.startswith("@rpath/") or real is None or not real.startswith(bundled):
                    sys.exit(f"{f} has an unbundled dependency: {ref}")
        for rp in rpaths(f):
            if not rp.startswith(("@executable_path", "@loader_path")):
                sys.exit(f"{f} still has rpath {rp}")
    size = sum(os.path.getsize(p) for p in copies.values())
    print(f"bundled {len(copies)} dylibs ({size / 2**20:.0f} MB) into {os.path.relpath(frameworks)}")
    print(f"needs macOS {info['LSMinimumSystemVersion']} (LSMinimumSystemVersion)")


if __name__ == "__main__":
    main()
