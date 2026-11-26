#!/usr/bin/env python3
"""Promote verified release records to the index.json target of the TUF repository.

Supply the complete set of records or update an existing index. The caller
serializes repository writes and authenticates records before invoking this tool.
"""
import argparse
import json
from pathlib import Path
import re
import tempfile

import update_packages


def release_version(text):
    if not isinstance(text, str) or not re.fullmatch(r"(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*)){2}", text):
        raise ValueError(f"invalid numeric release version {text!r}")
    return tuple(map(int, text.split(".")))


PACKAGE_SUFFIXES = {(platform, kind): suffix for platform, kind, suffix, _, _ in update_packages.formats()}


def entries(record, repository):
    if record["schema"] != 1 or record["source"] != "github-release":
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
        if key in seen:
            raise ValueError(f"duplicate package {key}")
        seen.add(key)
        yield {**package, **{field: record[field] for field in ("version", "tag", "commit", "notes", "source")}}


def promote(current, records, repository):
    if current["schema"] != 1:
        raise ValueError("unsupported update index")
    latest = {}
    for package in current["packages"]:
        key = (package["source"], package["platform"], package["format"])
        if key in latest:
            raise ValueError(f"duplicate index entry {key}")
        latest[key] = package
    for record in records:
        for package in entries(record, repository):
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
    with tempfile.NamedTemporaryFile(dir=args.index.parent, delete=False, mode="w", encoding="utf-8") as tmp:
        staged = Path(tmp.name)
        try:
            json.dump(updated, tmp, indent=2, sort_keys=True)
            tmp.write("\n")
        except OSError:
            tmp.close()
            staged.unlink(missing_ok=True)
            raise
    try:
        staged.replace(args.index)
    finally:
        staged.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
