#!/usr/bin/env python3
"""Reuse a pinned CI tool from an immutable bootstrap release."""
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
PLATFORMS = {"Linux-X64": "linux-x64", "macOS-ARM64": "macos-arm64", "Windows-X64": "windows-x64"}
KINDS = {'cargo-about', 'nsh-cli', 'nsh-android-arm64-v8a', 'nsh-android-armeabi-v7a', 'nsh-android-x86_64'}
ARTIFACT = re.compile(
    r"patina-(cargo-about|nsh-cli|nsh-android-(?:arm64-v8a|armeabi-v7a|x86_64))-release-"
    r"(Linux-X64|macOS-ARM64|Windows-X64)-([a-f0-9]{64})")
CARGO_ABOUT_LICENSE = "https://raw.githubusercontent.com/EmbarkStudios/cargo-about/8dd595a270e21c52badb21adcf0f95dce3f0c808/LICENSE-APACHE"
ASSET_LIMIT = 128 * 1024 * 1024
MANIFEST = "nsh-cli.json"
LICENSE = "LICENSE-APACHE"
ASSETS = {"nsh": ASSET_LIMIT, MANIFEST: 4096, LICENSE: 64 * 1024}
RUNTIME = "runtime.tar.gz"
FIELDS = "tagName,isDraft,isImmutable,isPrerelease,assets"


def gh(*args):
    return subprocess.run(["gh", *args, "--repo", REPOSITORY], capture_output=True, text=True, timeout=90)


def lookup(tag):
    result = gh("release", "view", tag, "--json", FIELDS)
    if result.returncode and "release not found" in result.stderr.lower():
        return None
    result.check_returncode()
    return json.loads(result.stdout)


def tag_for(platform, key, kind='nsh-cli'):
    if kind not in KINDS or not re.fullmatch(r"[a-f0-9]{64}", key):
        raise ValueError("Invalid CI tool build key")
    if ((kind == "cargo-about" and platform != "Windows-X64")
            or (kind.startswith("nsh-android-") and platform != "Linux-X64")
            or (kind == "nsh-cli" and platform == "Windows-X64")):
        raise ValueError("Unsupported platform for this CI tool")
    return f"ci-{kind}-{PLATFORMS[platform]}-{key}"


def payload(tag):
    if tag.startswith("ci-nsh-android-"):
        return RUNTIME
    return "cargo-about.exe" if tag.startswith("ci-cargo-about-") else "nsh"


def asset_limits(tag):
    return {payload(tag): ASSET_LIMIT, MANIFEST: ASSETS[MANIFEST], LICENSE: ASSETS[LICENSE]}


def asset_digests(release, tag, *, draft=False):
    if (not release or release["tagName"] != tag or release["isDraft"] is not draft
            or not release["isPrerelease"] or (not draft and not release["isImmutable"])):
        raise ValueError("CI tool release metadata does not match the expected immutable release")
    digests = {}
    limits = asset_limits(tag)
    for asset in release["assets"]:
        name, digest = asset["name"], asset.get("digest") or ""
        if name not in limits or name in digests:
            raise ValueError("CI tool asset metadata is invalid")
        if draft and asset["state"] == "starter":
            digests[name] = None
            continue
        if (asset["state"] != "uploaded" or not 0 < asset["size"] <= limits[name]
                or not re.fullmatch(r"sha256:[a-f0-9]{64}", digest)):
            raise ValueError("CI tool asset metadata is invalid")
        digests[name] = digest
    if not draft and digests.keys() != limits.keys():
        raise ValueError("CI tool release is missing an executable or manifest")
    return digests


def digest_file(path):
    with path.open("rb") as data:
        return "sha256:" + hashlib.file_digest(data, "sha256").hexdigest()


def restore(platform, key, path, kind='nsh-cli'):
    if platform not in PLATFORMS or (kind == "nsh-cli" and platform == "Windows-X64"):
        return False
    tag = tag_for(platform, key, kind)
    executable = payload(tag)
    release = lookup(tag)
    if not release or release["isDraft"]:
        return False
    expected = asset_digests(release, tag)
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=path.parent) as directory:
        gh("release", "download", tag, "--pattern", executable, "--pattern", MANIFEST, "--pattern", LICENSE,
           "--dir", directory).check_returncode()
        directory = Path(directory)
        downloaded = directory / executable
        manifest = directory / MANIFEST
        if {name: digest_file(directory / name) for name in expected} != expected:
            raise ValueError("CI tool release checksum mismatch")
        if json.loads(manifest.read_text()) != {"tag": tag, "digest": expected[executable]}:
            raise ValueError("CI tool manifest does not match the expected build key and executable")
        gh("attestation", "verify", str(manifest), "--signer-workflow", SIGNER,
           "--source-ref", "refs/heads/master", "--deny-self-hosted-runners").check_returncode()
        downloaded.chmod(0o755)
        downloaded.replace(path)
    print(f"Restored CI tool from immutable release {tag}")
    return True


def publish(tag, directory, commit):
    expected = {name: digest_file(directory / name) for name in asset_limits(tag)}
    release = lookup(tag)
    if release and not release["isDraft"]:
        asset_digests(release, tag)
        return
    if not release:
        gh("release", "create", tag, "--draft", "--prerelease", "--latest=false", "--target", commit,
           "--title", tag, "--notes", "Pinned bootstrap tool for Patina CI.").check_returncode()
        release = lookup(tag)
    existing = asset_digests(release, tag, draft=True)
    changed = [name for name in expected if existing.get(name) != expected[name]]
    if changed:
        options = ["--clobber"] if any(name in existing for name in changed) else []
        gh("release", "upload", tag, *options, *(str(directory / name) for name in changed)).check_returncode()
        release = lookup(tag)
    if asset_digests(release, tag, draft=True) != expected:
        raise ValueError("Draft CI tool release bytes differ from the staged build")
    gh("release", "edit", tag, "--draft=false", "--latest=false").check_returncode()
    if asset_digests(lookup(tag), tag) != expected:
        raise ValueError("Published CI tool release bytes differ from the staged build")
    print(f"Published immutable CI tool release {tag}")


def trusted_commit():
    if (os.environ.get("GITHUB_REPOSITORY") != REPOSITORY
            or os.environ.get("GITHUB_REF") != "refs/heads/master" or os.environ.get("GITHUB_EVENT_NAME") != "workflow_run"):
        raise ValueError("CI tool publication requires a trusted master Validation run")
    run = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())["workflow_run"]
    if (run["event"] != "push" or run["head_branch"] != "master" or run["path"] != ".github/workflows/validation.yml"
            or run["conclusion"] != "success" or run["head_repository"]["full_name"] != REPOSITORY
            or not re.fullmatch(r"[a-f0-9]{40}", run["head_sha"])):
        raise ValueError("CI tool publication requires a trusted master Validation run")
    return run["head_sha"]


def staged(root):
    for artifact in sorted((*root.glob("patina-nsh-*-release-*"), *root.glob("patina-cargo-about-release-*"))):
        match = ARTIFACT.fullmatch(artifact.name)
        if not match or not artifact.is_dir():
            raise ValueError(f"Invalid staged CI tool artifact: {artifact.name}")
        kind, platform, key = match.groups()
        yield tag_for(platform, key, kind), artifact


def upstream_license(commit):
    with urlopen(f"https://raw.githubusercontent.com/{REPOSITORY}/{commit}/nsh.rev", timeout=30) as data:
        revision = data.read(42).decode().strip()
    if not re.fullmatch(r"[a-f0-9]{40}", revision):
        raise ValueError("Invalid NSH revision for the staged CLI")
    return read_license(f"https://raw.githubusercontent.com/teng-lin/nsh/{revision}/{LICENSE}")


def read_license(url):
    with urlopen(url, timeout=30) as data:
        license_data = data.read(ASSETS[LICENSE] + 1)
    if not 0 < len(license_data) <= ASSETS[LICENSE]:
        raise ValueError("Invalid NSH license size")
    return license_data


def prepare(root):
    commit = trusted_commit()
    artifacts = []
    for tag, directory in staged(root):
        release = lookup(tag)
        if release and not release["isDraft"]:
            asset_digests(release, tag)
            continue
        artifacts.append((tag, directory))
    if not artifacts:
        return False
    licenses = {}
    for tag, directory in artifacts:
        source = "cargo-about" if tag.startswith("ci-cargo-about-") else "nsh"
        if source not in licenses:
            licenses[source] = read_license(CARGO_ABOUT_LICENSE) if source == "cargo-about" else upstream_license(commit)
        binary = directory / payload(tag)
        if not binary.is_file() or not 0 < binary.stat().st_size <= ASSET_LIMIT:
            raise ValueError("Invalid staged CI tool executable")
        (directory / LICENSE).write_bytes(licenses[source])
        (directory / MANIFEST).write_text(json.dumps({"tag": tag, "digest": digest_file(binary)}, sort_keys=True) + "\n")
    return True


def publish_staged(root):
    commit = trusted_commit()
    failures = []
    for tag, directory in staged(root):
        if not (directory / MANIFEST).is_file():
            continue
        try:
            publish(tag, directory, commit)
        except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
            print(f"::error::Could not publish {tag} ({type(error).__name__})")
            failures.append(tag)
    if failures:
        raise RuntimeError(f"Tool releases still pending: {', '.join(failures)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    download = commands.add_parser("restore")
    download.add_argument("platform")
    download.add_argument("key")
    download.add_argument("path", type=Path)
    download.add_argument("--kind", choices=KINDS, default='nsh-cli')
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
        restored = restore(options.platform, options.key, options.path, options.kind)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f"::warning::CI tool release restore failed ({type(error).__name__}); trying the cache")
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        print(f"restored={str(restored).lower()}", file=output)


if __name__ == "__main__":
    main()
