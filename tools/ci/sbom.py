#!/usr/bin/env python3
"""Write CycloneDX SBOMs for the Rust dependency graph, media runtime, and Android classpath.

Runs `cargo cyclonedx` (0.5.10) for the app crate with every target enabled,
so one document covers the Android, desktop, Windows and iPad dependency sets
resolved in Cargo.lock: normal and build dependencies, no dev-dependencies.
Build paths are replaced by paths relative to the repository, and the NSH
commit (a path dependency, so it has no registry checksum) is recorded.

Also emits CycloneDX documents for:
- The bundled media runtime (from tools/mpv-runtime/sources.lock.json or components.json).
- The Android release classpath (from hosts/android/app/build/reports/dependency-license/index.json).

Usage:
  sbom.py [rust] OUTPUT.cdx.json
  sbom.py media-runtime INPUT.json OUTPUT.cdx.json [VERSION]
  sbom.py android-classpath INDEX.json OUTPUT.cdx.json [VERSION]
"""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys

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


def media_runtime_sbom(components_path_or_dict, output_path=None, version="0.1.0"):
    if isinstance(components_path_or_dict, (str, Path)):
        data = json.loads(Path(components_path_or_dict).read_text(encoding="utf-8"))
    else:
        data = components_path_or_dict
    raw_components = []
    if isinstance(data, dict):
        if "components" in data:
            raw_components = data["components"]
        elif "sources" in data:
            raw_components = list(data["sources"])
            if data.get("toolchain"):
                raw_components.append(data["toolchain"])
    components = []
    for item in raw_components:
        name = item.get("name", "")
        ver = str(item.get("version", ""))
        comp = {
            "type": "library",
            "name": name,
            "version": ver,
            "purl": f"pkg:generic/{name}@{ver}" if ver else f"pkg:generic/{name}",
        }
        if "sha256" in item:
            comp["hashes"] = [{"alg": "SHA-256", "content": item["sha256"]}]
        if "url" in item:
            comp["externalReferences"] = [{"type": "distribution", "url": item["url"]}]
        licenses = []
        for lic in item.get("licenses", []):
            path = lic.get("path") if isinstance(lic, dict) else str(lic)
            licenses.append({"license": {"name": path}})
        if not licenses:
            for lic_file in item.get("license_files", []):
                licenses.append({"license": {"name": lic_file}})
        if licenses:
            comp["licenses"] = licenses
        components.append(comp)

    doc = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "name": "patina-media-runtime",
                "version": version,
            },
            "properties": [
                {"name": "patina:scope", "value": "bundled media runtime native components"}
            ],
        },
        "components": components,
    }
    result = json.dumps(doc, indent=2) + "\n"
    if output_path is not None:
        out = Path(output_path)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(result, encoding="utf-8")
    return result


def android_classpath_sbom(license_report_path_or_dict, output_path=None, version="0.1.0"):
    if isinstance(license_report_path_or_dict, (str, Path)):
        data = json.loads(Path(license_report_path_or_dict).read_text(encoding="utf-8"))
    else:
        data = license_report_path_or_dict
    raw_deps = data.get("dependencies", []) if isinstance(data, dict) else []
    components = []
    for item in raw_deps:
        module_name = item.get("moduleName", "")
        parts = module_name.split(":", 1)
        group = parts[0] if len(parts) == 2 else ""
        name = parts[1] if len(parts) == 2 else module_name
        mod_version = str(item.get("moduleVersion", ""))
        purl = f"pkg:maven/{group}/{name}@{mod_version}" if group else f"pkg:generic/{name}@{mod_version}"
        comp = {
            "type": "library",
            "name": name,
            "version": mod_version,
            "purl": purl,
        }
        if group:
            comp["group"] = group
        if item.get("moduleUrl"):
            comp["externalReferences"] = [{"type": "website", "url": item["moduleUrl"]}]
        lic_name = item.get("moduleLicense")
        lic_url = item.get("moduleLicenseUrl")
        if lic_name or lic_url:
            lic = {}
            if lic_name:
                lic["name"] = lic_name
            if lic_url:
                lic["url"] = lic_url
            comp["licenses"] = [{"license": lic}]
        components.append(comp)

    doc = {
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
                {"name": "patina:scope", "value": "Android release compile and runtime classpath dependencies"}
            ],
        },
        "components": components,
    }
    result = json.dumps(doc, indent=2) + "\n"
    if output_path is not None:
        out = Path(output_path)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(result, encoding="utf-8")
    return result


def main(arguments):
    if not arguments:
        raise SystemExit(__doc__)
    command = arguments[0]
    if command == "media-runtime":
        if len(arguments) not in (3, 4):
            raise SystemExit("Usage: sbom.py media-runtime INPUT.json OUTPUT.cdx.json [VERSION]")
        version = arguments[3] if len(arguments) == 4 else "0.1.0"
        media_runtime_sbom(arguments[1], arguments[2], version)
        print(arguments[2])
        return
    if command == "android-classpath":
        if len(arguments) not in (3, 4):
            raise SystemExit("Usage: sbom.py android-classpath INDEX.json OUTPUT.cdx.json [VERSION]")
        version = arguments[3] if len(arguments) == 4 else "0.1.0"
        android_classpath_sbom(arguments[1], arguments[2], version)
        print(arguments[2])
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
