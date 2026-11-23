#!/usr/bin/env python3
"""Stage the two Android native artifacts from this candidate run for Gradle."""
import argparse
import hashlib
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
ABIS = {"arm64-v8a": (2, 183, 64), "armeabi-v7a": (1, 40, 52)}


def assets(path):
    if not (path / "app.json").is_file():
        raise ValueError(f"missing app.json in {path}")
    checksums = {}
    for file in sorted(path.rglob("*")):
        if file.is_file():
            with file.open("rb") as content:
                checksums[file.relative_to(path)] = hashlib.file_digest(content, "sha256").digest()
    return checksums


def verify_abi(path, abi):
    elf_class, machine, size = ABIS[abi]
    with path.open("rb") as content:
        header = content.read(size)
    if (len(header) != size or header[:7] != b"\x7fELF" + bytes([elf_class, 1, 1])
            or struct.unpack_from("<HH", header, 16) != (3, machine)):
        raise ValueError(f"{path} is not a shared library for {abi}")


def merge(inputs, output, commit):
    shards = {abi: inputs / f"android-native-{abi}" for abi in ABIS}
    for abi, shard in shards.items():
        if (shard / "commit.txt").read_text().strip() != commit:
            raise ValueError(f"{abi} was built from a different commit")
        verify_abi(shard / "jniLibs" / abi / "libnsh.so", abi)

    arm64_assets = shards["arm64-v8a"] / "assets/nsh"
    if assets(arm64_assets) != assets(shards["armeabi-v7a"] / "assets/nsh"):
        raise ValueError("the ARM64 and ARMv7 builds produced different NSH assets")

    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        staged = Path(temporary) / "nsh"
        shutil.copytree(arm64_assets, staged / "assets/nsh")
        for abi, shard in shards.items():
            shutil.copytree(shard / "jniLibs" / abi, staged / "jniLibs" / abi)
        staged.rename(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inputs", type=Path)
    args = parser.parse_args()
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    try:
        merge(args.inputs, ROOT / "hosts/android/app/build/nsh", commit)
    except (OSError, ValueError) as error:
        parser.exit(1, f"cannot assemble Android native artifacts: {error}\n")


if __name__ == "__main__":
    main()
