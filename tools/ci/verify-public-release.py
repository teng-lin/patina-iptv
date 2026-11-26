#!/usr/bin/env python3
"""Verify the published bytes and their candidate-bound provenance without building."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

import update_packages

SPEC = importlib.util.spec_from_file_location("publish_release", Path(__file__).with_name("publish-release.py"))
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


# The release build and this verifier use the same numeric version-code policy.
VERSION_SPEC = importlib.util.spec_from_file_location("release_version", Path(__file__).with_name("version.py"))
versioning = importlib.util.module_from_spec(VERSION_SPEC)
VERSION_SPEC.loader.exec_module(versioning)


def release_commit(directory, repository, tag, publisher, ref):
    if ref == f"refs/tags/{tag}":
        return publisher
    # A manual promotion attests with the current master workflow. Its verified
    # manifest names the older tagged commit that supplied the package bytes.
    manifest = json.loads((directory / "candidate-manifest.json").read_text())
    if (ref != "refs/heads/master" or manifest.get("repository") != repository
            or manifest.get("tag") != tag or manifest.get("publisher") != {"sha": publisher, "ref": ref}
            or not re.fullmatch(r"[0-9a-f]{40}", manifest.get("sha", ""))):
        raise ValueError("release identity does not match the verified promotion manifest")
    return manifest["sha"]


def package_record(directory, repository, tag, commit, signing_directory):
    """Describe only supported viewer downloads from a verified public release."""
    version = tag.removeprefix("v")
    if tag != f"v{version}":
        raise ValueError("expected a versioned release tag")
    code = versioning.android_version_code(version)
    base = f"https://github.com/{repository}/releases"
    packages = []
    for platform, kind, suffix, architectures, minimum in update_packages.formats():
        asset = directory / f"Patina-{version}-{suffix}"
        if not asset.is_file():
            continue
        entry = {
            "platform": platform, "format": kind, "architectures": architectures,
            "minimum": minimum, "url": f"{base}/download/{tag}/{asset.name}",
            "length": asset.stat().st_size, "sha256": release.digest(asset),
        }
        if platform == "android":
            entry.update(versionCode=code, signing="apk")
        elif platform in release.SIGNED_PLATFORMS:
            entry["signing"] = "signed" if release.platform_signed(signing_directory, platform) else "preview"
        packages.append(entry)
    if not packages:
        raise ValueError(f"{tag} contains no supported Android or desktop downloads")
    return {"schema": 1, "source": "github-release", "version": version, "tag": tag, "commit": commit,
            "notes": f"{base}/tag/{tag}", "packages": packages}


def verify(directory, repository, commit, ref, runner=subprocess.run):
    release.verify_checksums(directory)
    release.provenance.verify(directory, repository, commit, ref, runner)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    parser.add_argument("commit")
    parser.add_argument("ref")
    parser.add_argument("--update-record", type=Path)
    parser.add_argument("--signing-directory", type=Path)
    args = parser.parse_args()
    if bool(args.update_record) != bool(args.signing_directory):
        parser.error("--update-record and --signing-directory must be used together")
    repository = os.environ["GH_REPO"]
    with tempfile.TemporaryDirectory(prefix="patina-public-assets-") as temporary:
        directory = Path(temporary)
        subprocess.run(["gh", "release", "download", args.tag, "--repo", repository,
                        "--dir", str(directory)], check=True)
        verify(directory, repository, args.commit, args.ref)
        if args.update_record:
            commit = release_commit(directory, repository, args.tag, args.commit, args.ref)
            record = package_record(directory, repository, args.tag, commit, args.signing_directory)
            args.update_record.write_text(json.dumps(record, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
