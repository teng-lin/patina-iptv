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


def release_version(text):
    if not isinstance(text, str) or not re.fullmatch(r"(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*)){2}", text):
        raise ValueError(f"invalid numeric release version {text!r}")
    return tuple(map(int, text.split(".")))


def entries(record):
    if record["schema"] != 1 or record["source"] != "github-release":
        raise ValueError("unsupported release record")
    version = record["version"]
    release_version(version)
    if record["tag"] != f"v{version}" or not re.fullmatch("[0-9a-f]{40}", record["commit"]):
        raise ValueError("inconsistent release identity")
    if not record["packages"]:
        raise ValueError("empty release record")
    seen = set()
    for package in record["packages"]:
        key = (package["platform"], package["format"])
        if key in seen:
            raise ValueError(f"duplicate package {key}")
        seen.add(key)
        yield {**package, **{field: record[field] for field in ("version", "tag", "commit", "notes", "source")}}


def promote(current, records):
    if current["schema"] != 1:
        raise ValueError("unsupported update index")
    latest = {}
    for package in current["packages"]:
        key = (package["source"], package["platform"], package["format"])
        if key in latest:
            raise ValueError(f"duplicate index entry {key}")
        latest[key] = package
    for record in records:
        for package in entries(record):
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
    args = parser.parse_args()
    current = json.loads(args.index.read_bytes()) if args.index.exists() else {"schema": 1, "packages": []}
    updated = promote(current, [json.loads(path.read_bytes()) for path in args.records])
    if updated == current:
        return
    with tempfile.NamedTemporaryFile(dir=args.index.parent, delete=False) as tmp:
        staged = Path(tmp.name)
    try:
        staged.write_text(json.dumps(updated, indent=2, sort_keys=True) + "\n")
        staged.replace(args.index)
    finally:
        staged.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
