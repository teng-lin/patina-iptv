#!/usr/bin/env python3
"""Fetch exact native pins, including tested pre-merge Actions candidates."""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[2]
SPECS = {
    "windows": ("tools/mpv-runtime/runtime.json", "mpv-runtime-windows-x86_64.zip", "mpv-runtime-sources.tar.gz", "runtime.json", ("build", "smoke")),
    "linux": ("tools/mpv-runtime-unix/runtime-linux.json", "mpv-runtime-linux-x86_64.tar.gz", "mpv-runtime-linux-sources.tar.gz", "runtime-linux.json", ("linux-build", "linux-smoke")),
    "macos": ("tools/mpv-runtime-unix/runtime-macos.json", "mpv-runtime-macos-arm64.tar.gz", "mpv-runtime-macos-sources.tar.gz", "runtime-macos.json", ("macos-build", "macos-smoke")),
    "ios": ("tools/mpv-runtime-ios/runtime-ios.json", "Libavformat.xcframework.zip", "mpv-runtime-ios-sources.tar.gz", "runtime-ios.json", ("ios-build",)),
}
SHA = re.compile(r"[a-f0-9]{64}")


def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def load_registry(root=ROOT):
    data = json.loads((root / "tools/ci/native-candidates.json").read_text())
    if data.get("schema") != 1 or not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", data.get("repository", "")):
        raise ValueError("Invalid native candidate registry")
    return data


def load_pin(platform, root=ROOT):
    return json.loads((root / SPECS[platform][0]).read_text())


def validate_pin(platform, pin, repository):
    suffix = "" if platform == "windows" else platform + "-"
    if pin.get("schema") != 1 or not re.fullmatch(r"mpv-runtime-" + suffix + r"[a-f0-9]{64}", pin.get("tag", "")):
        raise ValueError("Invalid native dependency tag")
    base = f"https://github.com/{repository}/releases/download/{pin['tag']}/"
    for field, checksum, filename in [("url", "sha256", SPECS[platform][1]), ("source_url", "source_sha256", SPECS[platform][2])]:
        if pin.get(field) != base + filename or not SHA.fullmatch(pin.get(checksum, "")):
            raise ValueError("Native asset URL or checksum is outside its pinned release")


def gh_json(endpoint):
    result = subprocess.run(["gh", "api", endpoint], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("GitHub native dependency request failed: " + result.stderr.strip())
    return json.loads(result.stdout)


def artifact_file(entry, repository, cache):
    identifier = entry.get("artifact_id")
    checksum = entry.get("artifact_sha256", "")
    if type(identifier) is not int or identifier <= 0 or not SHA.fullmatch(checksum):
        raise ValueError("Invalid native artifact identity")
    path = cache / "artifacts" / f"{identifier}.zip"
    if path.is_file() and digest(path) == checksum:
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            result = subprocess.run(["gh", "api", f"repos/{repository}/actions/artifacts/{identifier}/zip"], stdout=stream, stderr=subprocess.PIPE)
            if result.returncode:
                raise RuntimeError("Could not download the tested native candidate: " + result.stderr.decode(errors="replace"))
        except BaseException:
            stream.close()
            temporary.unlink(missing_ok=True)
            raise
    try:
        if digest(temporary) != checksum:
            raise ValueError("Native candidate artifact checksum mismatch")
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)
    return path


def candidate_entry(platform, pin, registry):
    entry = registry.get("platforms", {}).get(platform)
    if not entry or entry.get("tag") != pin["tag"]:
        raise ValueError("No tested candidate exists for this native pin")
    return entry


def candidate_file(platform, pin, registry, name, checksum, cache):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", name) or not SHA.fullmatch(checksum):
        raise ValueError("Invalid native candidate asset name or checksum")
    entry = candidate_entry(platform, pin, registry)
    archive = artifact_file(entry, registry["repository"], cache)
    destination = cache / pin["tag"] / name
    if destination.is_file() and digest(destination) == checksum:
        return destination
    destination.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(archive) as incoming:
        names = incoming.namelist()
        if len(names) != len(set(names)):
            raise ValueError("Duplicate files in native candidate artifact")
        if json.loads(incoming.read(SPECS[platform][3])) != pin:
            raise ValueError("Candidate manifest differs from the committed native pin")
        with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as outgoing:
            temporary = Path(outgoing.name)
            try:
                with incoming.open(name) as source:
                    shutil.copyfileobj(source, outgoing)
            except BaseException:
                outgoing.close()
                temporary.unlink(missing_ok=True)
                raise
    try:
        if digest(temporary) != checksum:
            raise ValueError("Native candidate asset checksum mismatch")
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)
    return destination


def fetch(platform, kind, cache, root=ROOT):
    registry, pin = load_registry(root), load_pin(platform, root)
    validate_pin(platform, pin, registry["repository"])
    filename = SPECS[platform][1 if kind == "runtime" else 2]
    checksum = pin["sha256" if kind == "runtime" else "source_sha256"]
    url = pin["url" if kind == "runtime" else "source_url"]
    destination = cache / pin["tag"] / filename
    if destination.is_file() and digest(destination) == checksum:
        return destination
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            request = urllib.request.Request(url, headers={"User-Agent": "Patina-native-pins/1"})
            try:
                with urllib.request.urlopen(request, timeout=120) as response:
                    if not response.geturl().startswith("https://"):
                        raise ValueError("Native asset redirected away from HTTPS")
                    shutil.copyfileobj(response, stream)
            except urllib.error.HTTPError as error:
                if error.code != 404:
                    raise
                stream.close()
                temporary.unlink(missing_ok=True)
                return candidate_file(platform, pin, registry, filename, checksum, cache)
        except BaseException:
            stream.close()
            temporary.unlink(missing_ok=True)
            raise
    try:
        if digest(temporary) != checksum:
            raise ValueError("Published native asset checksum mismatch")
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)
    return destination


def expected_tag(platform, root=ROOT):
    if platform == "windows":
        path, name = root / "tools/mpv-runtime/package.py", "native_package"
    elif platform == "ios":
        path, name = root / "tools/mpv-runtime-ios/build.py", "native_ios_build"
    else:
        path, name = root / "tools/mpv-runtime-unix/runtime.py", "native_runtime"
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    saved_path = sys.path[:]
    sys.path.insert(0, str(path.parent))
    try:
        spec.loader.exec_module(module)
        if platform == "windows":
            return "mpv-runtime-" + module.recipe_digest(root / "tools/mpv-runtime/sources.lock.json")
        if platform == "ios":
            return "mpv-runtime-ios-" + module.recipe_digest()
        return f"mpv-runtime-{platform}-" + module.recipe_digest(platform, module.resolve_lock(platform))
    finally:
        sys.path[:] = saved_path


def verify_run(platform, entry, repository):
    run_id = entry.get("run_id")
    commit = entry.get("source_commit", "")
    if type(run_id) is not int or run_id <= 0 or not re.fullmatch(r"[a-f0-9]{40}", commit):
        raise ValueError("Invalid native validation identity")
    run = gh_json(f"repos/{repository}/actions/runs/{run_id}")
    if (
        run.get("conclusion") != "success"
        or run.get("event") != "workflow_dispatch"
        or run.get("head_sha") != commit
        or run.get("head_repository", {}).get("full_name") != repository
        or run.get("path", "").split("@", 1)[0] != ".github/workflows/mpv-runtime.yml"
    ):
        raise ValueError("Native candidate did not pass its expected validation workflow")
    jobs = gh_json(f"repos/{repository}/actions/runs/{run_id}/jobs?per_page=100")["jobs"]
    successful = {job["name"] for job in jobs if job.get("conclusion") == "success"}
    if not set(SPECS[platform][4]) <= successful:
        raise ValueError("Native candidate is missing a successful platform playback check")
    artifact = gh_json(f"repos/{repository}/actions/artifacts/{entry['artifact_id']}")
    owner = artifact.get("workflow_run", {})
    if artifact.get("expired") or artifact.get("digest") != "sha256:" + entry["artifact_sha256"] or owner.get("id") != run_id or owner.get("head_sha") != commit:
        raise ValueError("Native candidate artifact does not belong to the validated run")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("platform", choices=sorted(SPECS))
    parser.add_argument("--kind", choices=("runtime", "sources"), default="runtime")
    parser.add_argument("--cache", type=Path, default=ROOT / "target/native-candidates")
    args = parser.parse_args()
    print(fetch(args.platform, args.kind, args.cache.resolve()))


if __name__ == "__main__":
    main()
