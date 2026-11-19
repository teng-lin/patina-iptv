#!/usr/bin/env python3
"""Exact tool identities, deliberately independent of IPTV's Cargo.lock."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


def identity(kind):
    if kind not in ("cli", "android"):
        raise ValueError(f"unknown NSH tool: {kind}")
    action = "nsh-cli" if kind == "cli" else "android"
    inputs = {
        "nsh": subprocess.check_output(
            ["git", "-C", "../nsh-patina", "rev-parse", "HEAD"], text=True
        ).strip(),
        "rustc": subprocess.check_output(
            ["rustc", "-vV"], cwd="../nsh-patina" if kind == "android" else ".", text=True
        ),
        "recipe": Path(f".github/actions/{action}/action.yml").read_text(),
        "key_recipe": Path(__file__).read_text(),
        "environment": {
            key: value for key, value in os.environ.items()
            if key in ("ImageOS", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CC", "CFLAGS", "CXX", "CXXFLAGS", "AR")
            or key.startswith(("CARGO_BUILD_", "CARGO_PROFILE_", "CARGO_TARGET_", "CC_", "CFLAGS_", "CXX_", "CXXFLAGS_", "AR_"))
        },
    }
    # The NSH checkout's configuration is covered by its commit. Cargo also
    # reads global config, and the CLI inherits config from the IPTV cwd even
    # though --manifest-path points at NSH.
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    configs = {f"cargo-home/{name}": cargo_home / name for name in ("config", "config.toml")}
    if kind == "cli":
        configs.update({f"iptv/{name}": Path(".cargo") / name for name in ("config", "config.toml")})
    inputs["cargo_config"] = {
        name: path.read_text() if path.exists() else None for name, path in configs.items()
    }
    if kind == "android":
        # The recipe pins the ABI, NDK, SDK and Rust profile. NSH selects the
        # newest installed build-tools directory, not necessarily the version
        # installed by our action. Hash the inventory rather than duplicating
        # NSH's version ordering; even an extra older version safely invalidates.
        sdk = Path(os.environ.get("ANDROID_HOME") or os.environ["ANDROID_SDK_ROOT"])
        inputs["build_tools"] = {
            path.name: (path / "source.properties").read_text()
            if (path / "source.properties").exists() else None
            for path in (sdk / "build-tools").iterdir() if path.is_dir()
        }
        if not inputs["build_tools"]:
            raise ValueError("Android SDK has no installed build-tools")
        inputs["java"] = subprocess.check_output(
            ["java", "-version"], stderr=subprocess.STDOUT, text=True
        )
    return hashlib.sha256(json.dumps(inputs, sort_keys=True).encode()).hexdigest()


if __name__ == "__main__":
    print(identity(sys.argv[1]))
