#!/usr/bin/env python3
"""Verify final artifact bytes against this repository's candidate-bound workflow."""
import re
import subprocess


def verify(directory, repository, commit, ref, runner=subprocess.run):
    if (not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository or "")
            or not re.fullmatch(r"[0-9a-f]{40}", commit or "")
            or not ref or not ref.startswith(("refs/tags/", "refs/heads/"))):
        raise ValueError("provenance requires repository, full candidate SHA and source ref")
    for artifact in sorted(directory.iterdir()):
        if artifact.is_symlink() or not artifact.is_file():
            raise ValueError("provenance subjects must be regular files")
        runner(["gh", "attestation", "verify", str(artifact), "--repo", repository,
                "--predicate-type", "https://slsa.dev/provenance/v1",
                "--signer-workflow", repository + "/.github/workflows/release.yml",
                "--source-digest", commit, "--source-ref", ref], check=True)
