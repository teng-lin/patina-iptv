#!/usr/bin/env python3
"""Retain Android exploratory output without promoting proxies to first render."""
import argparse
import json
from pathlib import Path
import re
import subprocess


def envelope(serial, text, commit):
    cold = []
    selections = []
    for line in text.splitlines():
        launch = re.search(r"run (\d+): activity launch (\d+) ms, guide mount proxy (-?\d+) ms", line)
        selection = re.search(r"zap (\d+): reported size proxy (-?\d+) ms", line)
        if launch:
            cold.append({"run": int(launch[1]), "activity_launch_ms": int(launch[2]), "guide_mount_proxy_ms": int(launch[3])})
        if selection:
            selections.append({"selection": int(selection[1]), "size_proxy_ms": int(selection[2])})
    return {
        "schema": 1, "commit": commit, "platform": "android", "backend": "media3-uncorrelated-proxies",
        "hardware_class": "emulator" if serial.startswith("emulator-") else "physical-tv-unverified",
        "build_mode": "profile-unverified", "fixture_sha256": None,
        "valid": False, "errors": ["missing correlated first-render evidence", "busy writer journey not measured",
                                    "fixture and installed build identity require independent verification"],
        "cold_observations": cold, "selection_observations": selections,
        "sample_count": {"cold": len(cold), "selection": len(selections)},
        "invalid_observations": [line for line in text.splitlines() if line.startswith("INVALID ")],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output")
    parser.add_argument("serial")
    parser.add_argument("observations", nargs="?")
    args = parser.parse_args()
    text = Path(args.observations).read_text(encoding="utf-8") if args.observations else ""
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(envelope(args.serial, text, commit), indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
