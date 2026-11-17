#!/usr/bin/env python3
"""Require the reviewed signing mode; never silently downgrade a signed release."""
import argparse
import json
import os
from pathlib import Path

FIELDS = {
    "macos": ("MACOS_CERTIFICATE", "MACOS_CERTIFICATE_PASSWORD", "MACOS_SIGN_IDENTITY",
              "APPLE_ID", "APPLE_TEAM_ID", "APPLE_APP_PASSWORD"),
    # Only booleans, not credential values, reach the Windows configuration step.
    "windows": ("TENANT", "CLIENT", "CLIENT_SECRET", "ENDPOINT", "ACCOUNT", "PROFILE"),
}


def validate(mode, present):
    if mode not in ("signed", "preview"):
        raise ValueError("signing mode must be signed or preview")
    expected = mode == "signed"
    if not present or any(value != expected for value in present):
        raise ValueError(f"{mode} signing mode requires {'all' if expected else 'no'} signing credentials; "
                         "correct configuration or review signing-policy.json")
    return expected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("platform", choices=FIELDS)
    parser.add_argument("--prepare", action="store_true", help="unsigned packaging before the signing job")
    args = parser.parse_args()
    policy = json.loads(Path(__file__).with_name("signing-policy.json").read_text())
    rehearsal = os.environ.get("REHEARSAL", "false")
    if rehearsal not in ("true", "false"):
        raise ValueError("REHEARSAL must be true or false")
    mode = "preview" if args.prepare or rehearsal == "true" else policy[args.platform]
    if args.platform == "windows":
        values = [os.environ.get(field, "false") for field in FIELDS[args.platform]]
        if any(value not in ("true", "false") for value in values):
            raise ValueError("Windows configuration accepts presence booleans only")
        present = [value == "true" for value in values]
    else:
        present = [bool(os.environ.get(field)) for field in FIELDS[args.platform]]
    signed = validate(mode, present)
    if args.platform == "windows":
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write(f"sign={str(signed).lower()}\n")
    print(f"{args.platform}: validated {mode} signing configuration")


if __name__ == "__main__":
    main()
