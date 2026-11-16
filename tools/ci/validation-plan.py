#!/usr/bin/env python3
"""Select PR validation conservatively; full tiers validate the exact checked-out SHA.

The aggregate consumes this same plan and every reusable job's result. Missing,
failed, or canceled selected jobs fail closed; only deliberate skips are accepted.
"""
import argparse
import fnmatch
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
LANES = ("host", "windows", "android", "packages", "windows-package", "coverage", "fuzz", "smoke", "stress")
BUILD = ("Cargo.toml", "Cargo.lock", "crates/**/Cargo.toml", "rust-toolchain.toml", "nsh.rev", ".cargo/**")
CI = (".github/workflows/**", ".github/actions/**", "tools/ci/validation-plan.py", "tools/ci/check-plan.py",
      "tools/ci/pins.py", "tools/ci/locked-build-env.sh", "tools/ci/locked-cargo.py", "tools/check.sh")
NOTICES = ("about.toml", "LICENSE", "tools/ci/notices.hbs", "tools/ci/component-inventory.py",
           "tools/ci/crypto-native-notices.py", "tools/ci/check-notices.py", "tools/ci/notices.sh",
           "tools/ci/package.Cargo.lock", "tools/ci/check-package-lock.py", "tools/ci/audit-runners.sh")
ANDROID_PACKAGE = (*NOTICES, *BUILD, "hosts/android/**", "crates/iptv-app/build.rs", "crates/iptv-app/nsh.json",
                   "crates/iptv-app/icon.png", "crates/iptv-app/fonts/**", "about.toml", "LICENSE",
                   "tools/sync-host.sh", "tools/ci/android-ndk-env.sh", "tools/ci/check-android*",
                   "tools/ci/sign-android.sh", "tools/ci/prepare-android.sh", "tools/ci/package-android.sh", "tools/ci/sbom.py",
                   "tools/mpv-runtime/**", "tools/mpv-runtime-unix/**", "crates/iptv-player-mpv/fixtures/**")
LINUX_PACKAGE = ("tools/ci/*linux*.sh", "tools/ci/*linux*.py", "hosts/linux/**")
MACOS_PACKAGE = ("tools/package-macos.sh", "tools/bundle-macos.py", "tools/ci/*macos*.sh", "tools/ci/*macos*.py", "hosts/macos/**")
PACKAGES = (*NOTICES, *LINUX_PACKAGE, *MACOS_PACKAGE, *BUILD, "about.toml", "LICENSE", "crates/iptv-app/build.rs", "crates/iptv-app/nsh.json",
            "crates/iptv-app/icon.png", "crates/iptv-app/fonts/**", "tools/package-*.sh",
            "tools/bundle-macos.py", "tools/ci/package-*.sh", "tools/ci/*packages.sh",
            "tools/ci/native-sources.py", "tools/ci/smoke-desktop.py", "tools/ci/nsh-tool-key.py",
            "tools/ci/native_candidates.py", "tools/ci/setup-linux-packaging.sh", "tools/ci/enable-deb-src.sh",
            "tools/ci/unpack-signing-input.py", "tools/mpv-runtime/**", "tools/mpv-runtime-unix/**")
WINDOWS_PACKAGE = (*NOTICES, *BUILD, "crates/iptv-app/build.rs", "hosts/windows/**", "tools/package-windows.ps1", "tools/build-windows.ps1",
                    "tools/fetch-mpv-windows.ps1", "tools/ci/*windows*.ps1", "tools/ci/*windows*.py",
                    "tools/ci/windows-notices.py", "tools/mpv-runtime/**",
                    "about.toml", "LICENSE", "crates/iptv-app/icon.png", "crates/iptv-app/fonts/**")
SMOKE = (*BUILD, "hosts/android/**", "crates/iptv-app/**", "crates/iptv-runtime/**",
         "crates/iptv-player-mpv/**", "crates/iptv-net/**", "crates/iptv-nav/**", "tools/sync-host.sh",
         "tools/ci/android/**", "tools/ci/android-smoke.sh", "tools/ci/*hls-fixtures.py",
         "tools/ci/android-ndk-env.sh", "tools/make-*fixtures*", "tools/mock-server.py", "tools/mpv-runtime/**")
STRESS = (*BUILD, "crates/iptv-store-sqlite/**", "crates/iptv-import/**", "crates/iptv-epg/**",
          "tools/ci/import-stress.sh")
COVERAGE = ("tools/ci/coverage.sh", "tools/ci/*coverage*.py")
DOCS = ("docs/**", ".gitignore", ".editorconfig")


def matches(paths, patterns):
    return any(fnmatch.fnmatchcase(path, pattern) for path in paths for pattern in patterns)


def documentation(path):
    # fnmatch's * crosses /, so '*.md' would hide embedded crate assets.
    return matches([path], DOCS) or ("/" not in path and path.lower().endswith(".md"))


def fuzz_targets(paths, full=False, root=ROOT):
    names = sorted(target["name"] for target in tomllib.loads((root / "fuzz/Cargo.toml").read_text())["bin"])
    if full or matches(paths, (*BUILD, *CI, "fuzz/Cargo.toml", "fuzz/Cargo.lock", "tools/ci/fuzz*",
                              "tools/ci/locked-cargo.py", "crates/iptv-core/**", "crates/iptv-settings/**")):
        return names
    selected = set()
    for path in paths:
        if path.startswith("fuzz/"):
            # Unknown/new shared fuzz inputs must not silently miss a target.
            target = next((name for name in names if f"/{name}/" in path or path.endswith(f"/{name}.rs")), None)
            selected.update([target] if target else names)
        elif path.startswith("crates/iptv-providers/"):
            if "/src/m3u" in path:
                selected.update(("m3u", "catchup", "gzip"))
            elif "/src/xtream" in path:
                selected.add("xtream")
            elif "/src/stalker" in path:
                selected.add("stalker")
            else:
                selected.update(names)
        elif path.startswith("crates/iptv-epg/"):
            selected.update(("xmltv", "gzip"))
        elif path.startswith("crates/iptv-import/"):
            selected.update(("backup_manifest", "gzip"))
        elif path.startswith("crates/iptv-net/"):
            selected.update(("dash", "hls"))
    if selected - set(names):
        raise ValueError("fuzz target dependency map is stale")
    return sorted(selected)


def select(paths, tier="pr", root=ROOT):
    if tier not in ("pr", "full", "release"):
        raise ValueError(f"unknown validation tier: {tier}")
    full = tier != "pr"
    ci = matches(paths, CI)
    rust = matches(paths, (*BUILD, "crates/**/*.rs"))
    # Unknown code/build inputs still receive host validation. A documentation
    # change alone never installs GTK, Rust targets, media runtimes, or Android.
    helper_paths = ("tools/ci/**", "tools/mpv-runtime*/**", "tools/*windows*.ps1", "tools/sync-host.sh",
                    "tools/package-*", "tools/bundle-*.py", "tools/mock-server.py", "tools/make-*fixtures*")
    host = full or ci or any(not documentation(path) and not matches([path], helper_paths) for path in paths)
    common_package = full or ci or any(matches([path], PACKAGES) and not matches([path], (*LINUX_PACKAGE, *MACOS_PACKAGE))
                                      for path in paths)
    platforms = [entry for entry, selected in (
        ({"platform": "linux", "os": "ubuntu-24.04"}, common_package or matches(paths, LINUX_PACKAGE)),
        ({"platform": "macos", "os": "macos-15"}, common_package or matches(paths, MACOS_PACKAGE)),
    ) if selected]
    android_package = full or ci or matches(paths, ANDROID_PACKAGE)
    result = {
        "tier": tier,
        "host": host,
        "helpers_full": full or any(not documentation(path) for path in paths),
        "windows": full or ci or rust or matches(paths, ("hosts/windows/**", "tools/*windows*.ps1")),
        "android": full or ci or rust or android_package,
        "android_package": android_package,
        "packages": bool(platforms),
        "package_matrix": {"include": platforms},
        "windows-package": full or ci or matches(paths, WINDOWS_PACKAGE),
        "coverage": full or ci or matches(paths, COVERAGE),
        "fuzz": fuzz_targets(paths, full or ci, root),
        "smoke": full or ci or matches(paths, SMOKE),
        "stress": full or ci or matches(paths, STRESS),
    }
    if tier == "release":
        # The release pipeline builds and validates the shipping packages once,
        # in clean jobs. Avoid constructing throwaway installers first.
        result["package_matrix"] = {"include": []}
        for lane in ("android", "android_package", "packages", "windows-package"):
            result[lane] = False
    return result


def gate(plan, needs):
    errors = []
    if needs.get("plan", {}).get("result") != "success":
        errors.append("change detection did not succeed")
    if needs.get("helpers", {}).get("result") != "success":
        errors.append("validation helper checks did not succeed")
    if not isinstance(plan, dict) or set(LANES) - plan.keys():
        return errors + ["validation plan is missing or incomplete"]
    if plan.get("tier") not in ("pr", "full", "release"):
        return errors + ["validation tier is missing or invalid"]
    for lane in LANES:
        if (lane == "fuzz" and not isinstance(plan[lane], list)) or (lane != "fuzz" and type(plan[lane]) is not bool):
            errors.append(f"invalid selection for {lane}")
            continue
        result = needs.get(lane, {}).get("result")
        selected = bool(plan[lane])
        if selected and result != "success":
            errors.append(f"selected {lane}: {result or 'missing'}")
        elif not selected and result not in ("success", "skipped"):
            errors.append(f"unselected {lane}: unexpected {result or 'missing'}")
    return errors


def changed_paths(environment=os.environ):
    event = environment["EVENT"]
    if event == "workflow_dispatch":
        return subprocess.check_output(["git", "diff", "--name-only", "--no-renames", "-z", "HEAD^..HEAD"], text=True).split("\0")[:-1]
    if event not in ("pull_request", "push", "merge_group"):
        return []
    base = environment["BASE_SHA"]
    head = environment["HEAD_SHA"]
    if not base or not head or set(base) == {"0"}:
        raise ValueError("change detection requires valid base and head commits")
    comparison = base + ("..." if event == "pull_request" else "..") + head
    return subprocess.check_output(["git", "diff", "--name-only", "--no-renames", "-z", comparison], text=True).split("\0")[:-1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("plan", "gate"))
    args = parser.parse_args()
    if args.action == "gate":
        try:
            errors = gate(json.loads(os.environ.get("PLAN", "{}")), json.loads(os.environ["NEEDS"]))
        except (ValueError, KeyError, TypeError) as error:
            errors = [f"invalid validation evidence: {error}"]
        for error in errors:
            print(error, file=sys.stderr)
        return bool(errors)
    event = os.environ["EVENT"]
    tier = os.environ.get("TIER", "") or ("full" if event in ("schedule", "workflow_dispatch") else "pr")
    paths = changed_paths() if tier == "pr" else []
    plan = select(paths, tier)
    print("plan=" + json.dumps(plan, separators=(",", ":")))
    for key, value in plan.items():
        print(f"{key}=" + (str(value).lower() if isinstance(value, bool) else json.dumps(value) if isinstance(value, (list, dict)) else value))
    return 0


if __name__ == "__main__":
    sys.exit(main())
