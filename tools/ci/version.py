"""Validate tag/version agreement and stamp generated package metadata."""
import json
import os
import re
from pathlib import Path
import tomllib

def android_version_code(version):
    # Reserve a higher range than the historical workflow-run counters. A
    # version has identical upgrade metadata in master, tag and manual builds.
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
        raise ValueError("Android distribution requires a numeric workspace version")
    major, minor, patch = map(int, version.split('.'))
    code = 1_000_000 + major * 1_000_000 + minor * 1_000 + patch
    if minor > 999 or patch > 999 or code > 2_100_000_000:
        raise ValueError("workspace version exceeds Android version-code allocation")
    return code


def main():
    version = tomllib.loads(Path("Cargo.toml").read_text())["workspace"]["package"]["version"]
    ref = os.environ["GITHUB_REF"]
    if ref.startswith("refs/tags/") and ref != f"refs/tags/v{version}":
        raise SystemExit("Release tag must equal v + workspace package version")
    code = android_version_code(version)
    path = Path("crates/iptv-app/nsh.json")
    config = json.loads(path.read_text())
    config["version"] = version
    path.write_text(json.dumps(config, indent=2) + "\n")
    with open(os.environ["GITHUB_ENV"], "a") as env:
        env.write(f"VERSION={version}\nIPTV_ANDROID_VERSION_CODE={code}\n")


if __name__ == "__main__":
    main()
