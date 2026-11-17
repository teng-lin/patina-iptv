#!/usr/bin/env python3
"""Record native build inputs beside release SBOMs, without dumping environment."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def inventory(root=ROOT, runner=subprocess.check_output, which=shutil.which):
    files = ("Cargo.lock", "rust-toolchain.toml", "nsh.rev", "tools/ci/package.Cargo.lock")
    result = {"schema": 1, "os": platform.platform(), "files": {}, "tools": {}, "native_packages": ""}
    for name in files:
        result["files"][name] = hashlib.sha256((root / name).read_bytes()).hexdigest()
    result["commit"] = runner(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    for name in ("rustc", "cargo", "clang", "cmake", "ninja", "java", "xcodebuild"):
        if which(name):
            args = [name, "-version" if name in ("java", "xcodebuild") else "--version"]
            result["tools"][name] = runner(args, text=True, stderr=subprocess.STDOUT, timeout=20).strip()
    if which("dpkg-query"):
        result["native_packages"] = runner(["dpkg-query", "-W", "-f=${Package}\\t${Version}\\n"], text=True, timeout=30)
    elif which("brew"):
        result["native_packages"] = runner(["brew", "list", "--versions"], text=True, timeout=60)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(inventory(), indent=2) + "\n")


if __name__ == "__main__":
    main()
