#!/usr/bin/env python3
"""Restore an exact bootstrap executable from a trusted unsigned master run."""
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile

WORKFLOWS = {
    ".github/workflows/package-check.yml",
    ".github/workflows/unsigned-candidate.yml",
    ".github/workflows/validation.yml",
}
EVENTS = {"push", "schedule", "workflow_dispatch"}
MAX_BYTES = 128 * 1024 * 1024


def api(repository, endpoint, *fields):
    return subprocess.check_output(
        ["gh", "api", "--method", "GET", f"repos/{repository}/actions/{endpoint}", *fields],
        timeout=60,
    )


def restore(repository, repository_id, name, destination):
    records = json.loads(api(repository, "artifacts", "-f", f"name={name}", "-F", "per_page=100"))
    for artifact in records["artifacts"]:
        source = artifact.get("workflow_run") or {}
        if (artifact["expired"] or artifact["name"] != name or not artifact.get("digest")
                or artifact["size_in_bytes"] > MAX_BYTES
                or source.get("head_branch") != "master"
                or source.get("repository_id") != repository_id
                or source.get("head_repository_id") != repository_id):
            continue
        run = json.loads(api(repository, f"runs/{int(source['id'])}"))
        if (run["event"] not in EVENTS or run["path"] not in WORKFLOWS
                or run["head_branch"] != "master" or run["head_sha"] != source["head_sha"]):
            continue
        data = api(repository, f"artifacts/{int(artifact['id'])}/zip")
        if len(data) > MAX_BYTES or "sha256:" + hashlib.sha256(data).hexdigest() != artifact["digest"]:
            raise ValueError("tool archive checksum mismatch")
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            entries = archive.infolist()
            if (len(entries) != 1 or entries[0].filename != destination.name
                    or not 0 < entries[0].file_size <= MAX_BYTES):
                raise ValueError("unexpected files in tool archive")
            executable = archive.read(entries[0])
        destination.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=destination.parent) as staging:
            candidate = Path(staging) / destination.name
            candidate.write_bytes(executable)
            candidate.chmod(0o755)
            candidate.replace(destination)
        print(f"Restored {destination.name} from master run {source['id']}")
        return True
    return False


def main(name, path):
    restored = False
    try:
        restored = restore(
            os.environ["GITHUB_REPOSITORY"], int(os.environ["GITHUB_REPOSITORY_ID"]), name, Path(path),
        )
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, zipfile.BadZipFile) as error:
        print(f"::warning::Could not restore retained macOS tool ({type(error).__name__}); building from source")
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        print(f"restored={str(restored).lower()}", file=output)


if __name__ == "__main__":
    main(*sys.argv[1:])
