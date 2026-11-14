#!/usr/bin/env python3
"""Publish the exact tested dependency bytes after they land on master."""

import argparse
import hashlib
import json
import shutil
import os
from pathlib import Path
import subprocess
import zipfile

import native_candidates as native


def release_assets(platform, pin, registry, cache):
    entry = native.candidate_entry(platform, pin, registry)
    native.verify_run(platform, entry, registry["repository"])
    if native.expected_tag(platform) != pin["tag"]:
        raise ValueError("The native recipe changed after candidate validation")
    source = native.artifact_file(entry, registry["repository"], cache)
    names = [*native.SPECS[platform][1:4], "SHA256SUMS"]
    checksums = {names[0]: pin["sha256"], names[1]: pin["source_sha256"]}
    directory = cache / "promotion" / pin["tag"]
    directory.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(source) as archive:
        if len(archive.namelist()) != len(set(archive.namelist())):
            raise ValueError("Duplicate native release asset")
        manifest = archive.read(names[2])
        if json.loads(manifest) != pin:
            raise ValueError("Candidate manifest differs from the committed pin")
        checksums[names[2]] = hashlib.sha256(manifest).hexdigest()
        expected_sums = "".join(f"{checksums[name]}  {name}\n" for name in names[:3]).encode()
        if archive.read("SHA256SUMS") != expected_sums:
            raise ValueError("Candidate SHA256SUMS differs from the verified assets")
        for name in names:
            destination = directory / name
            with archive.open(name) as incoming, destination.open("wb") as outgoing:
                shutil.copyfileobj(incoming, outgoing)
            if name in checksums and native.digest(destination) != checksums[name]:
                raise ValueError("Candidate release asset checksum mismatch")
    return [directory / name for name in names]


def existing_release(repository, tag):
    try:
        return native.gh_json(f"repos/{repository}/releases/tags/{tag}")
    except RuntimeError as error:
        if "HTTP 404" in str(error):
            return None
        raise


def unchanged_release(release, assets):
    expected = {path.name: "sha256:" + native.digest(path) for path in assets}
    actual = {asset["name"]: asset.get("digest") for asset in release["assets"]}
    if actual != expected or release.get("draft"):
        raise ValueError("Existing dependency release differs; refusing to overwrite")


def promote(platform, registry, cache, publish):
    repository = registry["repository"]
    pin = native.load_pin(platform)
    native.validate_pin(platform, pin, repository)
    release = existing_release(repository, pin["tag"])
    # Published releases survive Actions artifact expiry. The immutable runtime
    # and source pins, and the committed manifest bytes, remain authoritative.
    if release and not release.get("draft"):
        expected = {native.SPECS[platform][1]: "sha256:" + pin["sha256"], native.SPECS[platform][2]: "sha256:" + pin["source_sha256"],
                    native.SPECS[platform][3]: "sha256:" + native.digest(native.ROOT / native.SPECS[platform][0])}
        sums = "".join(f"{checksum.removeprefix('sha256:')}  {name}\n" for name, checksum in expected.items())
        expected["SHA256SUMS"] = "sha256:" + hashlib.sha256(sums.encode()).hexdigest()
        actual = {asset["name"]: asset.get("digest") for asset in release["assets"]}
        if actual != expected:
            raise ValueError("Existing dependency release differs; refusing to overwrite")
        print(f"{platform}: pinned dependency already published")
        return
    assets = release_assets(platform, pin, registry, cache)
    if release:
        raise ValueError("An unfinished dependency release exists; refusing to overwrite")
    if not publish:
        print(f"{platform}: candidate provenance, recipe, manifest and all release bytes verified")
        return
    notes = assets[0].parent / "release-notes.txt"
    notes.write_text(f"Pinned {platform} native dependency with the per-stream relay capability guard. The exact corresponding sources, patches, licenses and build recipe accompany the binary. Native synthetic playback and forbidden-connection checks passed before publication. iOS validation uses the arm64 simulator; the device slice is built.\n")
    subprocess.run(["gh", "release", "create", pin["tag"], "--repo", repository, "--draft", "--target", os.environ["GITHUB_SHA"],
                    "--title", pin["tag"], "--notes-file", str(notes), "--latest=false", *map(str, assets)], check=True)
    # Compare every uploaded digest before making this dependency public.
    uploaded = existing_release(repository, pin["tag"])
    if not uploaded:
        raise ValueError("Uploaded dependency release is missing")
    unchanged_release({**uploaded, "draft": False}, assets)
    subprocess.run(["gh", "release", "edit", pin["tag"], "--repo", repository, "--draft=false", "--latest=false"], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--publish", action="store_true")
    parser.add_argument("--platform", choices=native.SPECS)
    parser.add_argument("--cache", type=Path, default=native.ROOT / "target/native-candidates")
    args = parser.parse_args()
    registry = native.load_registry()
    if args.publish and (os.environ.get("GITHUB_REF") != "refs/heads/master" or os.environ.get("GITHUB_EVENT_NAME") != "push"
                         or os.environ.get("GITHUB_REPOSITORY") != registry["repository"]):
        raise ValueError("Native publication requires a trusted master push")
    for platform in ([args.platform] if args.platform else native.SPECS):
        promote(platform, registry, args.cache.resolve(), args.publish)


if __name__ == "__main__":
    main()
