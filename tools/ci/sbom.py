#!/usr/bin/env python3
"""Write CycloneDX SBOMs for the Rust dependency graph, a media runtime, and the Android classpath.

rust: runs `cargo cyclonedx` (0.5.10) for the app crate with every target
enabled, so one document covers the Android, desktop, Windows and iPad
dependency sets resolved in Cargo.lock: normal and build dependencies, no
dev-dependencies. Build paths are replaced by paths relative to the
repository, and the NSH commit (a path dependency, so it has no registry
checksum) is recorded.

media-runtime: one document per platform, from the components.json packaged
inside that platform's libmpv runtime archive (Windows: tools/mpv-runtime,
Linux and macOS: tools/mpv-runtime-unix). The built library is the only
component with a binary hash; each source keeps the checksum of its pinned
archive on its download reference, and the toolchain is listed as a build
input under `formulation`. A source lock is refused: it names what a build may
download, not what a package bundles.

android-classpath: from the Gradle dependency licence report
(hosts/android/app/build/reports/dependency-license/index.json).

Both fail without writing a document when the input is empty, malformed or
lacks a name, version, checksum or build provenance.

Usage:
  sbom.py [rust] OUTPUT.cdx.json
  sbom.py media-runtime COMPONENTS.json OUTPUT.cdx.json [VERSION]
          [--platform windows|linux|macos] [--manifest RUNTIME.json]
  sbom.py android-classpath INDEX.json OUTPUT.cdx.json [VERSION]
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
from urllib.parse import quote, urlsplit

ROOT = Path(__file__).resolve().parents[2]


def portable(text, root):
    """The document without the build machine's directories."""
    root = Path(root)
    for directory, relative in ((root, "."), (root.parent, "..")):
        text = text.replace(f"file://{directory}", f"file://{relative}")
    for directory in (root, root.parent):
        if str(directory) in text:
            raise ValueError("SBOM still names a build directory")
    return text


def finish(text, root, nsh_revision):
    document = json.loads(portable(text, root))
    if document.get("bomFormat") != "CycloneDX" or not document.get("components"):
        raise ValueError("cargo cyclonedx produced no component list")
    names = {component["name"] for component in document["components"]}
    workspace = {"iptv-core", "iptv-net", "iptv-store-sqlite", "nsh-core"}
    if not workspace <= names:
        raise ValueError(f"SBOM lacks workspace or NSH crates: {sorted(workspace - names)}")
    properties = document["metadata"].setdefault("properties", [])
    properties.append({"name": "patina:nsh-revision", "value": nsh_revision})
    properties.append({"name": "patina:scope", "value":
                       "Cargo.lock graph of iptv-app for all targets; normal and build dependencies"})
    return json.dumps(document, indent=2) + "\n"


# ----------------------------------------------------------- shared checks

SHA256 = re.compile(r"[0-9a-f]{64}")


def unique_keys(pairs):
    """A JSON object may not name a key twice: the later one would win unseen."""
    record = {}
    for key, value in pairs:
        if key in record:
            raise ValueError(f"duplicate key {key!r}")
        record[key] = value
    return record


def load(source, what):
    """The parsed document of a path, or an already parsed one; always an object."""
    if isinstance(source, (str, Path)):
        try:
            source = json.loads(Path(source).read_text(encoding="utf-8"), object_pairs_hook=unique_keys)
        except ValueError as error:
            raise ValueError(f"{what} {source} is not valid JSON: {error}") from error
    if not isinstance(source, dict):
        raise ValueError(f"{what} is not a JSON object")
    return source


def string(record, key, where):
    value = record.get(key)
    if not isinstance(value, str) or not value or value != value.strip() or not value.isprintable():
        raise ValueError(f"{where}: missing or malformed {key}")
    return value


def sha256(record, key, where):
    value = record.get(key)
    if not isinstance(value, str) or not SHA256.fullmatch(value):
        raise ValueError(f"{where}: missing or malformed SHA-256 {key}")
    return value


def https(record, key, where):
    value = string(record, key, where)
    if not value.startswith("https://") or not urlsplit(value).hostname or any(c.isspace() for c in value):
        raise ValueError(f"{where}: {key} is not an HTTPS URL")
    return value


def release_version(version):
    if not isinstance(version, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+-]*", version):
        raise ValueError(f"malformed release version: {version!r}")
    return version


def digest(value):
    return [{"alg": "SHA-256", "content": value}]


def write(document, output_path):
    """Serialize a finished document. Nothing is written when validation failed."""
    result = json.dumps(document, indent=2) + "\n"
    if output_path is not None:
        out = Path(output_path)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(result, encoding="utf-8")
    return result


# ----------------------------------------------------------- media runtime

# The `build` record each packager adds to the components.json inside its
# runtime archive: tools/mpv-runtime/package.py for Windows (no platform
# field, `dll_sha256`) and tools/mpv-runtime-unix/runtime.py for Linux and
# macOS (`platform`, `arch`, `library`, `library_sha256` and the linker
# identity that survives app packaging). test-sbom.py runs both packagers on
# synthetic inputs, so a change to either schema fails there.
RUNTIMES = {
    "windows": {"arch": "x86_64", "library": "libmpv-2.dll", "hash": "dll_sha256", "identity": None},
    "linux": {"arch": "x86_64", "library": "libmpv.so.2", "hash": "library_sha256",
              "identity": ("library_build_id", r"[0-9a-f]{16,}")},
    "macos": {"arch": "arm64", "library": "libmpv.2.dylib", "hash": "library_sha256",
              "identity": ("library_uuid", r"[0-9A-F]{8}(-[0-9A-F]{4}){3}-[0-9A-F]{12}")},
}
# Facts the Unix packager records about the library; kept as properties.
RUNTIME_FACTS = ("library_build_id", "library_uuid", "glibc_minimum", "macos_minimum")
COMPONENT_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")


def runtime_build(inventory, platform=None):
    """The validated `build` record of a packaged inventory and the platform it describes."""
    if platform is not None and platform not in RUNTIMES:
        raise ValueError(f"unknown runtime platform: {platform!r}")
    if "sources" in inventory and "components" not in inventory:
        raise ValueError("this is a source lock, not a packaged runtime inventory: it names what a build "
                         "may download, not what a package bundles. Pass the components.json from the "
                         "runtime archive of the platform being packaged")
    if inventory.get("schema") != 1:
        raise ValueError("expected runtime component inventory schema 1")
    build = inventory.get("build")
    if not isinstance(build, dict):
        raise ValueError("inventory has no build record: expected the components.json packaged in a "
                         "runtime archive, which names the built library and its source release")
    windows, unix = "dll_sha256" in build, "library_sha256" in build
    if windows == unix:
        raise ValueError("build record must name exactly one bundled library hash: "
                         "dll_sha256 (Windows) or library_sha256 (Linux, macOS)")
    if windows:
        if "platform" in build or "library" in build:
            raise ValueError("build record mixes the Windows and the Linux/macOS schema")
        described = "windows"
    else:
        described = build.get("platform")
        if described not in ("linux", "macos"):
            raise ValueError(f"build record names an unknown platform: {described!r}")
    if platform is not None and platform != described:
        raise ValueError(f"inventory describes the {described} runtime, not the {platform} one")
    runtime, where = RUNTIMES[described], f"{described} build record"
    if not windows and (build.get("arch") != runtime["arch"] or build.get("library") != runtime["library"]):
        raise ValueError(f"{where}: expected {runtime['library']} for {runtime['arch']}, "
                         f"got {build.get('library')!r} for {build.get('arch')!r}")
    recipe = sha256(build, "recipe_sha256", where)
    prefix = "mpv-runtime-" if windows else f"mpv-runtime-{described}-"
    if build.get("tag") != prefix + recipe:
        raise ValueError(f"{where}: tag is not the dependency release of its recipe ({prefix}<recipe_sha256>)")
    sha256(build, runtime["hash"], where)
    sha256(build, "source_sha256", where)
    https(build, "source_url", where)
    if "ffmpeg_sha256" in build:  # the test executable; never bundled, so not described
        sha256(build, "ffmpeg_sha256", where)
    if runtime["identity"]:
        key, pattern = runtime["identity"]
        if not isinstance(build.get(key), str) or not re.fullmatch(pattern, build[key]):
            raise ValueError(f"{where}: missing or malformed {key}")
    for key in RUNTIME_FACTS:
        if key in build:
            string(build, key, where)
    return described, build


def runtime_pin(manifest, platform, build):
    """Check the inventory against a runtime manifest; return the pinned archive's reference."""
    manifest = load(manifest, "runtime manifest")
    runtime, where = RUNTIMES[platform], f"{platform} runtime manifest"
    if manifest.get("schema") != 1:
        raise ValueError(f"{where}: expected schema 1")
    expected = {key: build[key] for key in ("tag", "source_url", "source_sha256", runtime["hash"])}
    if platform != "windows":
        expected.update(platform=platform, arch=runtime["arch"], library=runtime["library"])
        expected[runtime["identity"][0]] = build[runtime["identity"][0]]
    differing = sorted(key for key, value in expected.items() if manifest.get(key) != value)
    if differing:
        raise ValueError(f"{where} does not pin this inventory's runtime: {', '.join(differing)} differ")
    return {"type": "distribution", "url": https(manifest, "url", where),
            "hashes": digest(sha256(manifest, "sha256", where)),
            "comment": "pinned runtime archive that carries this library"}


def runtime_inputs(inventory):
    """The validated source and toolchain rows of an inventory, in its order."""
    rows = inventory.get("components")
    if not isinstance(rows, list) or not rows:
        raise ValueError("inventory lists no components")
    names, inputs = set(), []
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ValueError(f"component {index} is not an object")
        name = row.get("name")
        if not isinstance(name, str) or not COMPONENT_NAME.fullmatch(name) or name.casefold() in names:
            raise ValueError(f"component {index}: missing, malformed or duplicate name {name!r}")
        names.add(name.casefold())
        where = f"component {name}"
        if row.get("role") not in ("source", "toolchain"):
            raise ValueError(f"{where}: role must be source or toolchain, got {row.get('role')!r}")
        licenses = row.get("licenses")
        if not isinstance(licenses, list) or not licenses or not all(isinstance(item, dict) for item in licenses):
            raise ValueError(f"{where}: missing license files")
        files = []
        for item in licenses:
            path = string(item, "path", f"{where} license")
            parts = PurePosixPath(path).parts
            if "\\" in path or PurePosixPath(path).is_absolute() or ".." in parts:
                raise ValueError(f"{where}: unsafe license path {path!r}")
            files.append((path, sha256(item, "sha256", f"{where} license {path}")))
        if row.get("license_files") != [path for path, _ in files]:
            raise ValueError(f"{where}: license files differ from the locked license_files")
        inputs.append({"name": name, "role": row["role"], "version": string(row, "version", where),
                       "url": https(row, "url", where), "sha256": sha256(row, "sha256", where), "licenses": files})
    if sum(item["role"] == "toolchain" for item in inputs) > 1:
        raise ValueError("inventory names more than one toolchain")
    if not any(item["role"] == "source" for item in inputs):
        raise ValueError("inventory lists no source the library was built from")
    return inputs


def runtime_input(item, component_type):
    """A pinned build input. Its checksum is of the downloaded archive, never of a bundled
    file, so it is a hash of the download reference and not the component's own `hashes`."""
    qualifiers = f"download_url={quote(item['url'], safe=':/')}&checksum=sha256:{item['sha256']}"
    return {
        "type": component_type,
        "bom-ref": f"{item['role']}:{item['name']}",
        "name": item["name"],
        "version": item["version"],
        "purl": f"pkg:generic/{quote(item['name'], safe='')}@{quote(item['version'], safe='')}?{qualifiers}",
        "licenses": [{"license": {"name": path}} for path, _ in item["licenses"]],
        "externalReferences": [{"type": "distribution", "url": item["url"], "hashes": digest(item["sha256"]),
                                "comment": f"pinned {item['role']} archive; the hash is of this archive"}],
        "properties": [{"name": "patina:role", "value": item["role"]},
                       {"name": f"patina:{item['role']}-archive-sha256", "value": item["sha256"]}]
        + [{"name": "patina:license-file", "value": f"{path} sha256:{value}"} for path, value in item["licenses"]],
    }


def media_runtime_sbom(inventory, output_path=None, version="0.1.0", platform=None, manifest=None):
    """The CycloneDX document of one platform's bundled libmpv runtime.

    `inventory` is the components.json packaged inside that platform's runtime
    archive (a path or its parsed content). `platform`, when given, must be the
    one the inventory describes; `manifest` (runtime.json or
    runtime-<platform>.json) must pin the same build.

    The one bundled file is the libmpv library: it alone carries a binary
    hash. Sources carry their archive's hash on the download reference, and
    the toolchain is a build input under `formulation`, not a component.
    """
    release_version(version)
    inventory = load(inventory, "runtime inventory")
    platform, build = runtime_build(inventory, platform)
    inputs = runtime_inputs(inventory)
    runtime = RUNTIMES[platform]
    sources = [runtime_input(item, "library") for item in inputs if item["role"] == "source"]
    toolchain = [runtime_input(item, "application") for item in inputs if item["role"] == "toolchain"]
    library_hash = build[runtime["hash"]]
    references = [{"type": "distribution", "url": build["source_url"], "hashes": digest(build["source_sha256"]),
                   "comment": "corresponding source archive: sources, licenses, lock and build scripts"}]
    if manifest is not None:
        references.insert(0, runtime_pin(manifest, platform, build))
    library = {
        "type": "library",
        "bom-ref": "library",
        "name": runtime["library"],
        "version": build["tag"],
        "description": f"libmpv built for {platform} {runtime['arch']} from the listed sources. The hash is of "
                       "the library in the runtime archive; app packaging may re-sign it or rewrite load paths.",
        "hashes": digest(library_hash),
        "purl": f"pkg:generic/{quote(runtime['library'], safe='')}@{build['tag']}?checksum=sha256:{library_hash}",
        "externalReferences": references,
        "properties": [{"name": "patina:role", "value": "bundled-binary"},
                       {"name": "patina:source-archive-sha256", "value": build["source_sha256"]}]
        + [{"name": "patina:" + key.replace("_", "-"), "value": build[key]} for key in RUNTIME_FACTS if key in build],
    }
    document = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "bom-ref": "runtime",
                "name": "patina-media-runtime",
                "version": version,
            },
            "properties": [
                {"name": "patina:scope", "value": f"libmpv runtime bundled in the {platform} {runtime['arch']} "
                 "package: the built library, the pinned sources it was built from and, under formulation, "
                 "the build toolchain"},
                {"name": "patina:runtime-platform", "value": platform},
                {"name": "patina:runtime-arch", "value": runtime["arch"]},
                {"name": "patina:runtime-tag", "value": build["tag"]},
                {"name": "patina:runtime-recipe-sha256", "value": build["recipe_sha256"]},
            ],
        },
        "components": [library] + sources,
        "dependencies": [{"ref": "runtime", "dependsOn": ["library"]},
                         {"ref": "library", "dependsOn": [source["bom-ref"] for source in sources]}],
    }
    if toolchain:
        document["formulation"] = [{"bom-ref": "build", "components": toolchain}]
    return write(document, output_path)


# ------------------------------------------------------- Android classpath

MAVEN_NAME = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_.-]*")
MAVEN_VERSION = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.+-]*")


def maven_version(value, where):
    """A resolved release version: not Gradle's placeholder, a selector or a snapshot."""
    if not isinstance(value, str) or not MAVEN_VERSION.fullmatch(value):
        raise ValueError(f"{where}: missing or malformed moduleVersion {value!r}")
    if value == "unspecified" or value.endswith("+") or value.startswith("latest.") or value.endswith("-SNAPSHOT"):
        raise ValueError(f"{where}: moduleVersion {value!r} is not a fixed release version")
    return value


def optional_text(record, key, where):
    value = record.get(key)
    if value is None or value == "":
        return None
    if not isinstance(value, str) or not value.strip() or not value.isprintable():
        raise ValueError(f"{where}: malformed {key}")
    return value.strip()


def module_licenses(item, where):
    """The licence the report names: one per module, or a `moduleLicenses` list."""
    rows = [item]
    if "moduleLicenses" in item:
        rows = item["moduleLicenses"]
        if not isinstance(rows, list) or not all(isinstance(row, dict) for row in rows):
            raise ValueError(f"{where}: malformed moduleLicenses")
    licenses = []
    for row in rows:
        name, url = optional_text(row, "moduleLicense", where), optional_text(row, "moduleLicenseUrl", where)
        if name or url:
            licenses.append({"license": {key: value for key, value in (("name", name), ("url", url)) if value}})
    return licenses


def android_classpath_sbom(report, output_path=None, version="0.1.0"):
    """The CycloneDX document of the Android release classpath.

    `report` is the JSON of the Gradle dependency licence report (a path or
    its parsed content). Every dependency must be a `group:name` Maven
    coordinate at a fixed version; anything else fails instead of being
    written down as a guess.
    """
    release_version(version)
    report = load(report, "dependency report")
    dependencies = report.get("dependencies")
    if not isinstance(dependencies, list) or not dependencies:
        raise ValueError("dependency report lists no dependencies")
    components, seen = [], set()
    for index, item in enumerate(dependencies):
        if not isinstance(item, dict):
            raise ValueError(f"dependency {index} is not an object")
        module = item.get("moduleName")
        parts = module.split(":") if isinstance(module, str) else []
        if len(parts) != 2 or not all(MAVEN_NAME.fullmatch(part) for part in parts):
            raise ValueError(f"dependency {index}: moduleName {module!r} is not a group:name Maven coordinate")
        group, name = parts
        where = f"dependency {module}"
        module_version = maven_version(item.get("moduleVersion"), where)
        purl = f"pkg:maven/{quote(group, safe='')}/{quote(name, safe='')}@{quote(module_version, safe='')}"
        if purl in seen:
            raise ValueError(f"{where}: listed twice at {module_version}")
        seen.add(purl)
        component = {
            "type": "library",
            "bom-ref": purl,
            "group": group,
            "name": name,
            "version": module_version,
            "purl": purl,
        }
        website = optional_text(item, "moduleUrl", where)
        if website:
            component["externalReferences"] = [{"type": "website", "url": website}]
        licenses = module_licenses(item, where)
        if licenses:
            component["licenses"] = licenses
        components.append(component)
    document = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "name": "patina-android-classpath",
                "version": version,
            },
            "properties": [
                {"name": "patina:scope", "value": "Android release runtime classpath dependencies"}
            ],
        },
        "components": components,
    }
    return write(document, output_path)


# --------------------------------------------------------------------- CLI

def options(command, arguments):
    parser = argparse.ArgumentParser(prog=f"sbom.py {command}", description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("version", nargs="?", default="0.1.0")
    if command == "media-runtime":
        parser.add_argument("--platform", choices=sorted(RUNTIMES),
                            help="fail unless the inventory describes this platform's runtime")
        parser.add_argument("--manifest", type=Path,
                            help="runtime.json / runtime-<platform>.json that must pin the inventory's build")
    return parser.parse_args(arguments)


def main(arguments):
    if not arguments:
        raise SystemExit(__doc__)
    command = arguments[0]
    if command in ("media-runtime", "android-classpath"):
        args = options(command, arguments[1:])
        try:
            if command == "media-runtime":
                media_runtime_sbom(args.input, args.output, args.version, args.platform, args.manifest)
            else:
                android_classpath_sbom(args.input, args.output, args.version)
        except (OSError, ValueError) as error:
            # No document is written: a release must not carry an SBOM of a guess.
            raise SystemExit(f"sbom.py {command}: {error}") from error
        print(args.output)
        return
    if command == "rust":
        arguments = arguments[1:]
    if len(arguments) != 1:
        raise SystemExit(__doc__)
    output = Path(arguments[0]).resolve()
    spec = importlib.util.spec_from_file_location("pins", Path(__file__).with_name("pins.py"))
    pins = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(pins)
    lock = (ROOT / "Cargo.lock").read_bytes()
    commit_time = subprocess.check_output(["git", "-C", str(ROOT), "log", "-1", "--format=%ct"], text=True).strip()
    generated = ROOT / "crates/iptv-app/iptv-app.cdx.json"
    try:
        # It writes one file beside every workspace manifest; only the app's is kept.
        subprocess.run(["cargo", "cyclonedx", "--manifest-path", str(ROOT / "crates/iptv-app/Cargo.toml"),
                        "--format", "json", "--spec-version", "1.5", "--target", "all"],
                       cwd=ROOT, check=True, env=dict(os.environ, SOURCE_DATE_EPOCH=commit_time))
        text = generated.read_text()
    finally:
        for leftover in ROOT.glob("crates/*/*.cdx.json"):
            leftover.unlink()
    # cargo cyclonedx has no --locked: make sure it described the committed lock.
    if (ROOT / "Cargo.lock").read_bytes() != lock:
        raise SystemExit("cargo cyclonedx changed Cargo.lock; the SBOM would not describe the release")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(finish(text, ROOT, pins.nsh_revision()))
    print(output)


if __name__ == "__main__":
    main(sys.argv[1:])
