#!/usr/bin/env python3
"""Opt-in pinned Sparkle bundling, before app signing. No private keys are used.

All three PATINA_SPARKLE_* values are required together. The archive is the
unmodified upstream release; the public key and HTTPS appcast belong to Patina.
Current preview builds leave them unset and retain manual DMG installation.
"""
import argparse
import base64
import hashlib
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tarfile
import tempfile
from urllib.parse import urlsplit

VERSION = "2.10.1"
SHA256 = "1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27"
CONFIG = ("PATINA_SPARKLE_ARCHIVE", "PATINA_SPARKLE_FEED", "PATINA_SPARKLE_PUBLIC_KEY")


def configuration(environment):
    values = [environment.get(name, "") for name in CONFIG]
    if not any(values):
        return None
    if not all(values):
        raise ValueError("set all three PATINA_SPARKLE_ARCHIVE, PATINA_SPARKLE_FEED and PATINA_SPARKLE_PUBLIC_KEY")
    archive, feed, key = values
    url = urlsplit(feed)
    if (url.scheme != "https" or not url.hostname or url.username is not None
            or url.password is not None or "#" in feed):
        raise ValueError("Sparkle feed must be an HTTPS URL without credentials or fragment")
    try:
        decoded = base64.b64decode(key, validate=True)
    except ValueError as error:
        raise ValueError("Sparkle public key must be a base64 Ed25519 public key") from error
    if len(decoded) != 32:
        raise ValueError("Sparkle public key must contain 32 bytes")
    return Path(archive), feed, key


def configure(app, environment):
    config = configuration(environment)
    if config is None:
        return False
    archive, feed, key = config
    if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
        raise ValueError("Sparkle archive checksum differs from pinned upstream release")
    # The hash pins the complete upstream archive, including its symlinks.
    # Extract only the framework and license, never upstream sample apps/tools.
    with tempfile.TemporaryDirectory(prefix="patina-sparkle-") as directory:
        root = Path(directory)
        with tarfile.open(archive, "r:xz") as source:
            members = [member for member in source.getmembers()
                       if member.name.removeprefix("./").startswith("Sparkle.framework/")
                       or member.name.removeprefix("./") == "LICENSE"]
            source.extractall(root, members=members, filter="data")
        framework = root / "Sparkle.framework"
        info = plistlib.loads((framework / "Resources/Info.plist").read_bytes())
        if info.get("CFBundleIdentifier") != "org.sparkle-project.Sparkle" or info.get("CFBundleShortVersionString") != VERSION:
            raise ValueError("Sparkle framework identity/version differs from pin")
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(framework)], check=True)
        destination = app / "Contents/Frameworks/Sparkle.framework"
        if destination.exists():
            raise ValueError("refusing to replace an existing Sparkle framework")
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(framework, destination, symlinks=True)
        notices = app / "Contents/Resources/notices/sparkle"
        notices.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(root / "LICENSE", notices / "LICENSE")
        (notices / "SOURCE.txt").write_text(
            f"Sparkle {VERSION}\nhttps://github.com/sparkle-project/Sparkle/releases/tag/{VERSION}\n"
            f"Archive SHA256: {SHA256}\n", encoding="utf-8")
    path = app / "Contents/Info.plist"
    info = plistlib.loads(path.read_bytes())
    info.update(PatinaSparkleEnabled=True, SUFeedURL=feed, SUPublicEDKey=key,
                SUEnableAutomaticChecks=False, SUAllowsAutomaticUpdates=False,
                SUAutomaticallyUpdate=False, SUEnableJavaScript=False)
    path.write_bytes(plistlib.dumps(info))
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    args = parser.parse_args()
    try:
        enabled = configure(args.app, os.environ)
    except (ValueError, OSError, subprocess.CalledProcessError, tarfile.TarError) as error:
        parser.exit(1, f"Cannot configure Sparkle: {error}\n")
    print("Sparkle configured; Developer ID signing is required" if enabled else "Sparkle unconfigured; manual DMG installation")


if __name__ == "__main__":
    main()
