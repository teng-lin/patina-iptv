#!/usr/bin/env python3
"""Reuse a pinned NSH executable from an immutable bootstrap release."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
from urllib.request import urlopen


REPOSITORY = "teng-lin/patina-iptv"
SIGNER = f"{REPOSITORY}/.github/workflows/nsh-cli-releases.yml"
PLATFORMS = {"Linux-X64": "linux-x64", "macOS-ARM64": "macos-arm64"}
ARTIFACT = re.compile(r"patina-nsh-cli-release-(Linux-X64|macOS-ARM64)-([a-f0-9]{64})")
ASSET_LIMIT = 128 * 1024 * 1024
MANIFEST = "nsh-cli.json"
LICENSE = "LICENSE-APACHE"
ASSETS = {"nsh": ASSET_LIMIT, MANIFEST: 4096, LICENSE: 64 * 1024}
FIELDS = "tagName,isDraft,isImmutable,isPrerelease,assets"


def gh(*args):
    return subprocess.run(["gh", *args, "--repo", REPOSITORY], capture_output=True, text=True, timeout=90)


def lookup(tag):
    result = gh("release", "view", tag, "--json", FIELDS)
    if result.returncode and "release not found" in result.stderr.lower():
        return None
    result.check_returncode()
    return json.loads(result.stdout)


def tag_for(platform, key):
    if not re.fullmatch(r"[a-f0-9]{64}", key):
        raise ValueError("Invalid NSH CLI build key")
    return f"ci-nsh-cli-{PLATFORMS[platform]}-{key}"


def asset_digests(release, tag, *, draft=False):
    if (not release or release["tagName"] != tag or release["isDraft"] is not draft
            or not release["isPrerelease"] or (not draft and not release["isImmutable"])):
        raise ValueError("NSH CLI release metadata does not match the expected immutable release")
    digests = {}
    for asset in release["assets"]:
        name, digest = asset["name"], asset.get("digest") or ""
        if (name not in ASSETS or name in digests or asset["state"] != "uploaded"
                or not 0 < asset["size"] <= ASSETS[name] or not re.fullmatch(r"sha256:[a-f0-9]{64}", digest)):
            raise ValueError("NSH CLI asset metadata is invalid")
        digests[name] = digest
    if not draft and digests.keys() != ASSETS.keys():
        raise ValueError("NSH CLI release is missing an executable or manifest")
    return digests


def digest_file(path):
    with path.open("rb") as data:
        return "sha256:" + hashlib.file_digest(data, "sha256").hexdigest()


def restore(platform, key, path):
    if platform not in PLATFORMS:
        return False
    tag = tag_for(platform, key)
    release = lookup(tag)
    if not release or release["isDraft"]:
        return False
    expected = asset_digests(release, tag)
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=path.parent) as directory:
        gh("release", "download", tag, "--pattern", "nsh", "--pattern", MANIFEST, "--pattern", LICENSE,
           "--dir", directory).check_returncode()
        directory = Path(directory)
        downloaded = directory / "nsh"
        manifest = directory / MANIFEST
        if {name: digest_file(directory / name) for name in ASSETS} != expected:
            raise ValueError("NSH CLI release checksum mismatch")
        if json.loads(manifest.read_text()) != {"tag": tag, "digest": expected["nsh"]}:
            raise ValueError("NSH CLI manifest does not match the expected build key and executable")
        gh("attestation", "verify", str(manifest), "--signer-workflow", SIGNER,
           "--source-ref", "refs/heads/master", "--deny-self-hosted-runners").check_returncode()
        downloaded.chmod(0o755)
        downloaded.replace(path)
    print(f"Restored NSH CLI from immutable release {tag}")
    return True


def publish(tag, directory, commit):
    expected = {name: digest_file(directory / name) for name in ASSETS}
    release = lookup(tag)
    if release and not release["isDraft"]:
        asset_digests(release, tag)
        return
    if not release:
        gh("release", "create", tag, "--draft", "--prerelease", "--latest=false", "--target", commit,
           "--title", tag, "--notes", "Pinned NSH CLI for Patina CI.").check_returncode()
        release = lookup(tag)
    existing = asset_digests(release, tag, draft=True)
    if any(expected[name] != digest for name, digest in existing.items()):
        raise ValueError("Draft NSH CLI release bytes differ from the staged build")
    missing = [str(directory / name) for name in expected if name not in existing]
    if missing:
        gh("release", "upload", tag, *missing).check_returncode()
        release = lookup(tag)
    if asset_digests(release, tag, draft=True) != expected:
        raise ValueError("Draft NSH CLI release bytes differ from the staged build")
    gh("release", "edit", tag, "--draft=false", "--latest=false").check_returncode()
    if asset_digests(lookup(tag), tag) != expected:
        raise ValueError("Published NSH CLI release bytes differ from the staged build")
    print(f"Published immutable NSH CLI release {tag}")


def trusted_commit():
    if (os.environ.get("GITHUB_REPOSITORY") != REPOSITORY
            or os.environ.get("GITHUB_REF") != "refs/heads/master" or os.environ.get("GITHUB_EVENT_NAME") != "workflow_run"):
        raise ValueError("NSH CLI publication requires a trusted master Validation run")
    run = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())["workflow_run"]
    if (run["event"] != "push" or run["head_branch"] != "master" or run["path"] != ".github/workflows/validation.yml"
            or run["head_repository"]["full_name"] != REPOSITORY or not re.fullmatch(r"[a-f0-9]{40}", run["head_sha"])):
        raise ValueError("NSH CLI publication requires a trusted master Validation run")
    return run["head_sha"]


def staged(root):
    for artifact in sorted(root.glob("patina-nsh-cli-release-*")):
        match = ARTIFACT.fullmatch(artifact.name)
        if not match or not artifact.is_dir():
            raise ValueError(f"Invalid staged NSH CLI artifact: {artifact.name}")
        yield tag_for(*match.groups()), artifact


def upstream_license(commit):
    with urlopen(f"https://raw.githubusercontent.com/{REPOSITORY}/{commit}/nsh.rev", timeout=30) as data:
        revision = data.read(42).decode().strip()
    if not re.fullmatch(r"[a-f0-9]{40}", revision):
        raise ValueError("Invalid NSH revision for the staged CLI")
    with urlopen(f"https://raw.githubusercontent.com/teng-lin/nsh/{revision}/{LICENSE}", timeout=30) as data:
        license_data = data.read(ASSETS[LICENSE] + 1)
    if not 0 < len(license_data) <= ASSETS[LICENSE]:
        raise ValueError("Invalid NSH license size")
    return license_data


def prepare(root):
    commit = trusted_commit()
    artifacts = list(staged(root))
    if not artifacts:
        return False
    license_data = upstream_license(commit)
    for tag, directory in artifacts:
        binary = directory / "nsh"
        if not binary.is_file() or not 0 < binary.stat().st_size <= ASSET_LIMIT:
            raise ValueError("Invalid staged NSH executable")
        (directory / LICENSE).write_bytes(license_data)
        (directory / MANIFEST).write_text(json.dumps({"tag": tag, "digest": digest_file(binary)}, sort_keys=True) + "\n")
    return True


def publish_staged(root):
    commit = trusted_commit()
    for tag, directory in staged(root):
        publish(tag, directory, commit)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    download = commands.add_parser("restore")
    download.add_argument("platform")
    download.add_argument("key")
    download.add_argument("path", type=Path)
    for command in ("prepare", "publish"):
        commands.add_parser(command).add_argument("root", type=Path)
    options = parser.parse_args()
    if options.command == "publish":
        publish_staged(options.root)
        return
    if options.command == "prepare":
        available = prepare(options.root)
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            print(f"available={str(available).lower()}", file=output)
        return
    restored = False
    try:
        restored = restore(options.platform, options.key, options.path)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f"::warning::NSH CLI release restore failed ({type(error).__name__}); trying the cache")
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        print(f"restored={str(restored).lower()}", file=output)


if __name__ == "__main__":
    main()
