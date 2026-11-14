#!/usr/bin/env python3
"""The two external pins, each recorded in exactly one tracked file.

`nsh.rev` names the commit of the NSH fork that must be checked out beside
this repository: NSH is a path dependency, so Cargo.lock cannot record it.
`rust-toolchain.toml` names the Rust release. The composite actions,
tools/check.sh and the Windows build read both here instead of repeating them.

Usage:
  pins.py                  print `nsh=<commit>` and `toolchain=<channel>`
                           (the format of $GITHUB_OUTPUT)
  pins.py check-nsh [DIR]  fail unless the checkout in DIR (default ../nsh)
                           is at the pinned commit; IPTV_ALLOW_NSH_DRIFT=1
                           turns a difference into a warning
"""
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]


def nsh_revision(root=ROOT):
    """The pinned commit: one full lowercase SHA-1 and a newline, nothing else."""
    text = (Path(root) / "nsh.rev").read_text()
    if not re.fullmatch(r"[0-9a-f]{40}\n", text):
        raise ValueError("nsh.rev must hold one full 40-character commit SHA and a newline")
    return text.strip()


def toolchain(root=ROOT):
    channel = tomllib.loads((Path(root) / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", channel):
        raise ValueError("rust-toolchain.toml must pin an exact Rust release")
    return channel


def check_nsh(checkout, root=ROOT, environment=os.environ):
    """None when the checkout is at the pin, otherwise what to tell the developer."""
    expected = nsh_revision(root)
    result = subprocess.run(["git", "-C", str(checkout), "rev-parse", "HEAD"], capture_output=True, text=True)
    actual = result.stdout.strip() if result.returncode == 0 else None
    if actual != expected:
        found = f"is at {actual}" if actual else "is not a Git checkout"
        message = (
            f"{checkout} {found}, but nsh.rev pins {expected}.\n"
            f"Check out the pinned commit:  git -C {checkout} fetch origin && git -C {checkout} checkout --detach {expected}\n"
            "When developing the fork, set IPTV_ALLOW_NSH_DRIFT=1, and update nsh.rev before committing."
        )
        if environment.get("IPTV_ALLOW_NSH_DRIFT") == "1":
            print(f"warning: IPTV_ALLOW_NSH_DRIFT=1: {checkout} {found}; nsh.rev pins {expected}", file=sys.stderr)
            return None
        return message

    status_result = subprocess.run(["git", "-C", str(checkout), "status", "--porcelain"], capture_output=True, text=True)
    if status_result.returncode != 0:
        return f"Could not inspect the NSH checkout at {checkout}: git status failed"
    if status_result.stdout.strip():
        message = (
            f"{checkout} is at {expected} but has uncommitted changes:\n"
            f"{status_result.stdout.strip()}\n"
            "When developing the fork, set IPTV_ALLOW_NSH_DRIFT=1, and update nsh.rev before committing."
        )
        if environment.get("IPTV_ALLOW_NSH_DRIFT") == "1":
            print(f"warning: IPTV_ALLOW_NSH_DRIFT=1: {checkout} has uncommitted changes", file=sys.stderr)
            return None
        return message

    return None


def main(arguments):
    if not arguments:
        print(f"nsh={nsh_revision()}")
        print(f"toolchain={toolchain()}")
        return
    if arguments[0] == "check-nsh" and len(arguments) <= 2:
        problem = check_nsh(Path(arguments[1]) if len(arguments) == 2 else ROOT.parent / "nsh")
        if problem:
            raise SystemExit(problem)
        return
    raise SystemExit(__doc__)


if __name__ == "__main__":
    main(sys.argv[1:])
