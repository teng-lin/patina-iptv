#!/usr/bin/env python3
"""Promote verified release records to the index.json target of the TUF repository.

Supply the complete set of records or update an existing index. The caller
serializes repository writes and authenticates records before invoking this tool.
"""
import argparse
import os
import json
from pathlib import Path
import re
import stat
import tempfile

import update_packages
import version as versioning


def release_version(text):
    if not isinstance(text, str) or not re.fullmatch(r"(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*)){2}", text):
        raise ValueError(f"invalid numeric release version {text!r}")
    return tuple(map(int, text.split(".")))


PACKAGE_SUFFIXES = {(platform, kind): suffix for platform, kind, suffix, _, _ in update_packages.formats()}


def entries(record, repository):
    if type(record["schema"]) is not int or record["schema"] != 1 or record["source"] != "github-release":
        raise ValueError("unsupported release record")
    version = record["version"]
    release_version(version)
    if record["tag"] != f"v{version}" or not re.fullmatch("[0-9a-f]{40}", record["commit"]):
        raise ValueError("inconsistent release identity")
    if not record["packages"]:
        raise ValueError("empty release record")
    base = f"https://github.com/{repository}/releases"
    if record["notes"] != f"{base}/tag/{record['tag']}":
        raise ValueError("release notes belong to a different release")
    seen = set()
    for package in record["packages"]:
        key = (package["platform"], package["format"])
        suffix = PACKAGE_SUFFIXES.get(key)
        expected = f"{base}/download/{record['tag']}/Patina-{version}-{suffix}"
        if (suffix is None or package["url"] != expected or type(package["length"]) is not int
                or package["length"] <= 0 or not re.fullmatch("[0-9a-f]{64}", package["sha256"])):
            raise ValueError(f"invalid release asset for {key}")
        architectures = package.get("architectures")
        minimum = package.get("minimum")
        if (not isinstance(architectures, list) or not architectures
                or not all(isinstance(arch, str) and arch for arch in architectures)
                or len(set(architectures)) != len(architectures)
                or not isinstance(minimum, dict) or not minimum):
            raise ValueError(f"missing or invalid compatibility metadata for {key}")
        if package["platform"] == "android":
            if (type(minimum.get("androidApi")) is not int or minimum["androidApi"] < 1
                    or package.get("signing") != "apk" or type(package.get("versionCode")) is not int
                    or package["versionCode"] != versioning.android_version_code(version)):
                raise ValueError("invalid Android release metadata")
        else:
            required = {key[0]}
            if key[0] == "linux":
                required = {"glibc"} if key[1] == "appimage" else {"glibc", "gtk"}
            if (not required.issubset(minimum)
                    or not all(isinstance(floor, str) and re.fullmatch(r"[0-9]+(?:\.[0-9]+)*", floor)
                               for floor in minimum.values())):
                raise ValueError(f"invalid desktop requirements for {key}")
            if key[0] in ("macos", "windows") and package.get("signing") not in ("signed", "preview"):
                raise ValueError(f"missing desktop signing state for {key}")
        if key in seen:
            raise ValueError(f"duplicate package {key}")
        seen.add(key)
        yield {**package, **{field: record[field] for field in ("version", "tag", "commit", "notes", "source")}}


def promote(current, records, repository):
    if type(current["schema"]) is not int or current["schema"] != 1:
        raise ValueError("unsupported update index")
    latest = {}
    commits = {}

    def check_identity(package):
        identity = package["source"], package["version"]
        if commits.setdefault(identity, package["commit"]) != package["commit"]:
            raise ValueError(f"conflicting commits for immutable release {identity}")
    for package in current["packages"]:
        check_identity(package)
        key = (package["source"], package["platform"], package["format"])
        if key in latest:
            raise ValueError(f"duplicate index entry {key}")
        latest[key] = package
    for record in records:
        for package in entries(record, repository):
            check_identity(package)
            key = (package["source"], package["platform"], package["format"])
            previous = latest.get(key)
            if previous:
                old = release_version(previous["version"])
                new = release_version(package["version"])
                if old == new and previous != package:
                    raise ValueError(f"conflicting bytes or metadata for immutable release {package['tag']} {key}")
                if new <= old:
                    continue
            latest[key] = package
    return {"schema": 1, "packages": [latest[key] for key in sorted(latest)]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("index", type=Path, help="path to the index.json TUF target")
    parser.add_argument("records", type=Path, nargs="+")
    parser.add_argument("--repository", default="teng-lin/patina-iptv", help="GitHub owner/repository of the verified releases")
    args = parser.parse_args()
    current = json.loads(args.index.read_bytes()) if args.index.exists() else {"schema": 1, "packages": []}
    updated = promote(current, [json.loads(path.read_bytes()) for path in args.records], args.repository)
    if updated == current:
        return
    mode = stat.S_IMODE(args.index.stat().st_mode) if args.index.exists() else 0o644
    staged = None
    try:
        with tempfile.NamedTemporaryFile(dir=args.index.parent, delete=False, mode="w",
                                         encoding="utf-8", newline="\n") as tmp:
            staged = Path(tmp.name)
            json.dump(updated, tmp, indent=2, sort_keys=True)
            tmp.write("\n")
        os.chmod(staged, mode)
        staged.replace(args.index)
    finally:
        if staged is not None:
            staged.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
