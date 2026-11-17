#!/usr/bin/env python3
"""Validate target inputs before running the locked fuzz driver without a shell."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess

SPEC = importlib.util.spec_from_file_location("fuzz_targets", Path(__file__).with_name("fuzz-targets.py"))
targets = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(targets)
ROOT = Path(__file__).resolve().parents[2]


def command(environment=os.environ):
    selected = json.loads(environment.get("TARGETS", "[]"))
    known = targets.targets((ROOT / "fuzz/Cargo.toml").read_text())
    if not isinstance(selected, list) or any(not isinstance(target, str) or target not in known for target in selected):
        raise ValueError("fuzz selection must contain only manifest target names")
    if len(set(selected)) != len(selected):
        raise ValueError("fuzz target selection must not repeat targets")
    seconds = int(environment.get("SECONDS_PER_TARGET", "300"))
    if not 1 <= seconds <= 300:
        raise ValueError("fuzz time per target must be between 1 and 300 seconds")
    return ["bash", "tools/ci/fuzz-imports.sh", str(seconds), *(selected or known)]


if __name__ == "__main__":
    raise SystemExit(subprocess.call(command(), cwd=ROOT))
