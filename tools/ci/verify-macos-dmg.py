#!/usr/bin/env python3
"""Verify a DMG, retrying only the observed transient hdiutil read failure."""
import argparse
import os
import subprocess
import sys
import time

ATTEMPTS = 4


def verify(image):
    transient = [
        f'hdiutil: verify: unable to recognize "{image}" as a disk image. (Resource temporarily unavailable)',
        "hdiutil: verify failed - Resource temporarily unavailable",
    ]
    for attempt in range(1, ATTEMPTS + 1):
        print(f"DMG verification attempt {attempt}/{ATTEMPTS}: {image}", flush=True)
        result = subprocess.run(["hdiutil", "verify", image],
                                env=dict(os.environ, LC_ALL="C"),
                                capture_output=True, text=True, check=False)
        print(result.stdout, end="", flush=True)
        print(result.stderr, end="", file=sys.stderr, flush=True)
        if result.returncode == 0:
            return 0
        # Match the complete known diagnostic: corruption or any other error
        # fails immediately, including failures that also mention this phrase.
        if result.stderr.splitlines() != transient or attempt == ATTEMPTS:
            return result.returncode
        print(f"Transient DMG read failure; retrying verification in {attempt}s.",
              file=sys.stderr, flush=True)
        time.sleep(attempt)
    raise AssertionError("verification attempts exhausted without returning")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image")
    args = parser.parse_args(argv)
    return verify(args.image)


if __name__ == "__main__":
    sys.exit(main())
