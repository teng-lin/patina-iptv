#!/usr/bin/env python3
"""Prepare and validate a Cargo lockfile candidate with the pinned sibling NSH.

Produces a review artifact, never commits, pushes, publishes or opens a PR. Shipping
runner lock reconciliation and the normal PR/release gates remain required.
"""
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]


def candidate(root=ROOT, runner=subprocess.run):
    commands = [
        [sys.executable, "tools/ci/pins.py", "check-nsh"],
        ["cargo", "update"],
        ["cargo", "deny", "--locked", "check"],
        [sys.executable, "tools/ci/check-plan.py", "run", "--tier", "pr"],
    ]
    for command in commands:
        result = runner(command, cwd=root, check=False)
        if result.returncode:
            return result.returncode
    return 0


def main():
    output = ROOT / "target/dependency-candidate"
    output.mkdir(parents=True, exist_ok=True)
    try:
        code = candidate()
    finally:
        (output / "Cargo.lock").write_bytes((ROOT / "Cargo.lock").read_bytes())
        patch = subprocess.check_output(["git", "diff", "--", "Cargo.lock"], cwd=ROOT)
        (output / "Cargo.lock.patch").write_bytes(patch)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
            summary.write(f"Cargo candidate validation exit: {code}. Review artifact only; no PR or publication.\n")
    return code


if __name__ == "__main__":
    raise SystemExit(main())
