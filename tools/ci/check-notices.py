#!/usr/bin/env python3
"""Require nonempty generated notices in a package's extracted file tree."""
from pathlib import Path
import sys
root = Path(sys.argv[1])
for name in ("THIRD-PARTY.html", "components.json", "NSH-RUNNERS.html",
             "runner-components.json", "NOTICE-pyatv.md"):
    matches = list(root.rglob(name))
    if not matches or not all(p.stat().st_size > 0 for p in matches):
        raise SystemExit(f"missing or empty package notice: {name}")

if len(sys.argv) > 2:
    import json
    relative = sys.argv[2]
    matches = list(root.rglob(relative))
    if not matches or not all(json.loads(p.read_text()) for p in matches):
        raise SystemExit(f"missing or empty native inventory: {relative}")
