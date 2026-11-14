#!/usr/bin/env python3
"""Decide which Android jobs a change needs (.github/workflows/android.yml).

package  the full job: native library, notices, Gradle tests, lint, APK and AAB
compile  the cheap job: `cargo check` of the app library for Android

Pushes and manual runs do both; the compile job runs there to keep its build
cache warm for pull requests. A pull request packages only when a packaging
input changed. Any other pull request that touches Rust sources still compiles
for Android: the host checks never build `cfg(target_os = "android")` code.

Reads EVENT, BASE_SHA and HEAD_SHA; prints `package=` and `compile=` lines
in the format of $GITHUB_OUTPUT.
"""
import fnmatch
import os
import subprocess

# fnmatch's `*` also crosses `/`, so `crates/**/*.rs` is every Rust file below crates/.
PACKAGE = (
    ".github/workflows/android.yml", ".github/actions/android/**",
    ".github/actions/setup/**", ".github/actions/nsh-cli/**",
    ".cargo/config", ".cargo/config.toml", "hosts/android/**", "Cargo.toml",
    "Cargo.lock", "crates/**/Cargo.toml", "crates/iptv-app/build.rs",
    "crates/iptv-app/nsh.json", "crates/iptv-app/icon.png",
    "crates/iptv-app/fonts/**", "tools/sync-host.sh", "tools/ci/**",
    "crates/iptv-player-mpv/fixtures/**",
    "rust-toolchain.toml", "nsh.rev", "about.toml",
)
RUST = ("crates/**/*.rs", "crates/**/Cargo.toml", "Cargo.toml", "Cargo.lock")


def matches(paths, patterns):
    return any(fnmatch.fnmatchcase(path, pattern) for path in paths for pattern in patterns)


def select(event, paths):
    if event != "pull_request":
        return {"package": True, "compile": True}
    package = matches(paths, PACKAGE)
    # Packaging compiles the same sources; do not build them twice.
    return {"package": package, "compile": not package and matches(paths, RUST)}


def main():
    event = os.environ["EVENT"]
    paths = []
    if event == "pull_request":
        paths = subprocess.check_output([
            "git", "diff", "--name-only", "-z",
            os.environ["BASE_SHA"] + "..." + os.environ["HEAD_SHA"],
        ]).decode().split("\0")
    for name, value in select(event, paths).items():
        print(f"{name}={str(value).lower()}")


if __name__ == "__main__":
    main()
