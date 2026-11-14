#!/usr/bin/env python3
"""Publish a tagged release exactly once.

A public release is immutable: people have downloaded its files, and the
checksums and provenance attestations name those exact bytes. This script
therefore only ever writes to a release that is absent or still a draft, and
publishes it once every file is uploaded. An interrupted run leaves a draft,
which a rerun completes; a release that is already public is refused, and the
fix is a new version.

The macOS and Windows packages are platform-signed only when the signing
credentials exist. Each signing job records what it did (`signing`), and the
release notes then say plainly which packages are unsigned previews (`notes`).

Usage (with GH_TOKEN and GH_REPO in the environment for checksums/publish):
  publish-release.py checksums DIR TAG          refuse a public release, write DIR/SHA256SUMS
  publish-release.py publish DIR TAG [NOTES]    create or reuse the draft (NOTES prepended to the
                                                generated notes), upload DIR/*, publish
  publish-release.py signing PLATFORM STATE DIR record STATE (signed|unsigned) as DIR/PLATFORM.json
  publish-release.py notes DIR OUTPUT           write release notes from DIR's signing records
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

CHECKSUMS = "SHA256SUMS"


def gh(*arguments):
    return subprocess.run(["gh", *arguments], capture_output=True, text=True)


def describe(tag):
    """(absent, draft or public; the names of its files). Anything unclear is
    an error, never "absent"."""
    result = gh("release", "view", tag, "--json", "isDraft,tagName,assets")
    if result.returncode != 0:
        if "release not found" in result.stderr.lower():
            return "absent", []
        raise SystemExit(f"could not read the state of release {tag}: {result.stderr.strip()}")
    release = json.loads(result.stdout)
    files = release.get("assets", [])
    if (release.get("tagName") != tag or not isinstance(release.get("isDraft"), bool)
            or not isinstance(files, list) or not all(isinstance(f, dict) and isinstance(f.get("name"), str) for f in files)):
        raise SystemExit(f"unexpected description of release {tag}: {result.stdout.strip()}")
    return ("draft" if release["isDraft"] else "public"), [f["name"] for f in files]


def release_state(tag):
    return describe(tag)[0]


def refuse_public(tag):
    state = release_state(tag)
    if state == "public":
        raise SystemExit(
            f"release {tag} is already public: published files are never replaced. "
            "Release a new version instead.")
    return state


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def assets(directory):
    """Every file to release, by name, checksum file aside."""
    entries = sorted(directory.iterdir(), key=lambda path: path.name)
    if any(not path.is_file() or path.is_symlink() for path in entries):
        raise SystemExit(f"{directory} must hold only regular files")
    files = [path for path in entries if path.name != CHECKSUMS]
    if not files:
        raise SystemExit(f"no files to release in {directory}")
    return files


def write_checksums(directory):
    """The format `sha256sum --check` reads, names without a directory."""
    lines = [f"{digest(path)}  {path.name}\n" for path in assets(directory)]
    (directory / CHECKSUMS).write_text("".join(lines))
    return lines


def verify_checksums(directory):
    """The files about to be uploaded are exactly the ones that were attested."""
    listed = (directory / CHECKSUMS).read_text().splitlines(keepends=True)
    actual = [f"{digest(path)}  {path.name}\n" for path in assets(directory)]
    if listed != actual:
        raise SystemExit(f"{directory}/{CHECKSUMS} does not describe the files beside it")


def must(*arguments):
    result = gh(*arguments)
    if result.returncode != 0:
        raise SystemExit(f"gh {' '.join(arguments[:3])} failed: {result.stderr.strip()}")


# What platform trust each signed package gets, and what an unsigned one lacks.
SIGNED_PLATFORMS = {
    "macos": ("macOS disk image", "signed with a Developer ID certificate and notarized by Apple",
              "Gatekeeper blocks it until you allow it"),
    "windows": ("Windows installer", "Authenticode-signed",
                "SmartScreen warns before it runs"),
}


def record_signing(platform, state, directory):
    if platform not in SIGNED_PLATFORMS or state not in ("signed", "unsigned"):
        raise SystemExit(f"unknown signing record: {platform} {state}")
    directory.mkdir(parents=True, exist_ok=True)
    (directory / f"{platform}.json").write_text(json.dumps({"platform": platform, "signed": state == "signed"}) + "\n")


def release_notes(directory):
    """Notes for the unsigned packages, from every signing record. Missing records fail:
    a package must never be described as signed without its job saying so."""
    states = {}
    for platform in SIGNED_PLATFORMS:
        path = directory / f"{platform}.json"
        if not path.is_file():
            raise SystemExit(f"no signing record for {platform} in {directory}")
        record = json.loads(path.read_text())
        if record.get("platform") != platform or not isinstance(record.get("signed"), bool):
            raise SystemExit(f"malformed signing record: {path}")
        states[platform] = record["signed"]
    previews = [platform for platform, signed in states.items() if not signed]
    if not previews:
        return ""
    lines = ["## Preview builds: " + " and ".join(SIGNED_PLATFORMS[p][0] for p in previews), ""]
    for platform in previews:
        name, trust, warning = SIGNED_PLATFORMS[platform]
        lines.append(f"- The {name} is a **preview**: it is not {trust}, so {warning}. "
                     "The README explains how to open it.")
    lines += ["", "Signed packages follow once signing credentials are configured. SHA256SUMS and the "
              "build provenance attestations cover every file either way.", ""]
    return "\n".join(lines)


def publish(directory, tag, notes=None, rehearsal=False):
    state = refuse_public(tag)
    verify_checksums(directory)
    if state == "absent":
        extra = ["--notes-file", str(notes)] if notes is not None and notes.read_text().strip() else []
        must("release", "create", tag, "--verify-tag", "--draft", "--generate-notes", *extra)
    elif notes is not None:
        must("release", "edit", tag, "--notes-file", str(notes))
    # Replacing is safe only here: the release was just seen to be a draft.
    files = [*assets(directory), directory / CHECKSUMS]
    must("release", "upload", tag, *(str(path) for path in files), "--clobber")
    state, present = describe(tag)
    if state != "draft":
        raise SystemExit(f"release {tag} stopped being a draft during the upload; not publishing")
    # An earlier attempt may have left files this one does not have: they are
    # in neither SHA256SUMS nor the attestations, so they must not go public.
    for name in sorted(set(present) - {path.name for path in files}):
        must("release", "delete-asset", tag, name, "--yes")
    if not rehearsal:
        must("release", "edit", tag, "--draft=false")


def main(arguments):
    rehearsal = False
    if "--rehearsal" in arguments:
        rehearsal = True
        arguments = [a for a in arguments if a != "--rehearsal"]
    if os.environ.get("IPTV_RELEASE_REHEARSAL") == "1":
        rehearsal = True

    command = arguments[0] if arguments else None
    if command == "signing" and len(arguments) == 4:
        record_signing(arguments[1], arguments[2], Path(arguments[3]))
        return
    if command == "notes" and len(arguments) == 3:
        text = release_notes(Path(arguments[1]))
        Path(arguments[2]).write_text(text)
        print(text or "every platform-signed package is signed: no preview notes")
        return
    if not (command == "checksums" and len(arguments) == 3 or command == "publish" and len(arguments) in (3, 4)):
        raise SystemExit(__doc__)
    directory, tag = Path(arguments[1]), arguments[2]
    if command == "checksums":
        refuse_public(tag)
        sys.stdout.writelines(write_checksums(directory))
    else:
        publish(directory, tag, Path(arguments[3]) if len(arguments) == 4 else None, rehearsal=rehearsal)
        print(f"{'rehearsed' if rehearsal else 'published'} {tag}")


if __name__ == "__main__":
    main(sys.argv[1:])
