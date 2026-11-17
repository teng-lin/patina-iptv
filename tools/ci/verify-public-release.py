#!/usr/bin/env python3
"""Verify the published bytes and their candidate-bound provenance without building."""
import argparse
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile

SPEC = importlib.util.spec_from_file_location("publish_release", Path(__file__).with_name("publish-release.py"))
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


def verify(directory, repository, commit, ref, runner=subprocess.run):
    release.verify_checksums(directory)
    for artifact in sorted(directory.iterdir()):
        runner(["gh", "attestation", "verify", str(artifact), "--repo", repository,
                "--signer-workflow", repository + "/.github/workflows/release.yml",
                "--source-digest", commit, "--source-ref", ref], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    parser.add_argument("commit")
    parser.add_argument("ref")
    args = parser.parse_args()
    repository = os.environ["GH_REPO"]
    with tempfile.TemporaryDirectory(prefix="patina-public-assets-") as temporary:
        directory = Path(temporary)
        subprocess.run(["gh", "release", "download", args.tag, "--repo", repository,
                        "--dir", str(directory)], check=True)
        verify(directory, repository, args.commit, args.ref)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
