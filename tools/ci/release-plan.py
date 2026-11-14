#!/usr/bin/env python3
"""Decide what a run of the release workflow packages, and refuse the runs
that must not start, before anything is built.

Three kinds of run exist (.github/workflows/release.yml):

- a pushed `v*` tag: the release. Android and desktop, signed in the protected
  `release` environment and published.
- a manual run on master without `rehearsal`: the same packaging and signing,
  with the selected platforms, and no publication.
- a manual run with `rehearsal`, on any branch: the whole pipeline against a
  throwaway draft release. Its signing jobs run in the `release-rehearsal`
  environment and receive no release credential, so it covers exactly the
  platforms that can be packaged without one: Android (signed with a key made
  for the run, tools/ci/sign-android.sh) and the desktop previews. An IPA
  cannot be signed without the distribution certificate and provisioning
  profile, so a rehearsal of `all` is refused here rather than failing in the
  signing job an hour later.

Reads EVENT_NAME, REF, PLATFORMS and REHEARSAL from the environment and writes
`matrix` and `signing_matrix` to the file named by GITHUB_OUTPUT.
"""
import json
import os

SELECTIONS = ("android-desktop", "android", "desktop", "all")
REHEARSAL_SELECTION = "android-desktop"


def plan(event, ref, selection, rehearsal):
    """The platforms to package: [{"platform": ..., "os": ...}]."""
    if event not in ("push", "workflow_dispatch"):
        raise SystemExit(f"the release workflow does not run on {event!r} events")
    if rehearsal not in ("true", "false"):
        raise SystemExit(f"REHEARSAL must be true or false, not {rehearsal!r}")
    if selection not in SELECTIONS:
        raise SystemExit(f"unknown platform selection {selection!r}")
    if event == "push":
        if not ref.startswith("refs/tags/v"):
            raise SystemExit(f"a pushed release must be a v* tag, not {ref}")
        if rehearsal == "true" or selection != "android-desktop":
            raise SystemExit("a pushed tag is always a real Android and desktop release")
    elif rehearsal == "true":
        if selection == "all":
            raise SystemExit(
                "A rehearsal cannot include iOS: signing the IPA needs the distribution certificate and "
                "provisioning profile, and a rehearsal never receives release credentials. "
                f"Rehearse with platforms={REHEARSAL_SELECTION}; validate iOS with a manual platforms=all run on master.")
        if selection != REHEARSAL_SELECTION:
            raise SystemExit(
                f"A rehearsal requires the complete Android and desktop matrix (platforms={REHEARSAL_SELECTION}), "
                f"not platforms={selection}")
    elif ref != "refs/heads/master":
        raise SystemExit(
            f"A manual run that is not a rehearsal signs with the release credentials and only runs on master, not {ref}")
    platforms = []
    if selection in ("android", "android-desktop", "all"):
        platforms.append({"platform": "android", "os": "ubuntu-24.04"})
    if selection in ("desktop", "android-desktop", "all"):
        platforms += [
            {"platform": "linux", "os": "ubuntu-24.04"},
            {"platform": "macos", "os": "macos-15"},
        ]
    if selection == "all":
        platforms.append({"platform": "ios", "os": "macos-15"})
    return platforms


def main():
    environment = os.environ
    rehearsal = environment.get("REHEARSAL", "")
    platforms = plan(environment.get("EVENT_NAME", ""), environment.get("REF", ""),
                     environment.get("PLATFORMS", ""), rehearsal)
    # Linux packages are not platform-signed; the others pass through the signing job.
    signing = [entry for entry in platforms if entry["platform"] != "linux"]
    with open(environment["GITHUB_OUTPUT"], "a") as output:
        output.write("matrix=" + json.dumps({"include": platforms}) + "\n")
        output.write("signing_matrix=" + json.dumps({"include": signing}) + "\n")
    names = ", ".join(entry["platform"] for entry in platforms)
    if rehearsal == "true":
        print(f"rehearsal of {names}: no release credentials, throwaway Android key, unsigned desktop previews")
    else:
        print(f"packaging {names} with the release credentials")


if __name__ == "__main__":
    main()
