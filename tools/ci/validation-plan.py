#!/usr/bin/env python3
"""Select PR validation conservatively; full tiers validate the exact checked-out SHA.

The aggregate consumes this same plan and every reusable job's result. Missing,
failed, or canceled selected jobs fail closed; only deliberate skips are accepted.
"""
import argparse
import fnmatch
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
LANES = ("host", "windows", "android", "packages", "windows-package", "coverage", "fuzz", "smoke", "stress", "ios", "credentials")
HOST_VERIFICATIONS = {"credentials": "credentials-verified", "ios": "ios-verified"}
BUILD = ("Cargo.toml", "Cargo.lock", "crates/**/Cargo.toml", "rust-toolchain.toml", "nsh.rev", ".cargo/**", "vendor/nsh-tv/**")
CI = (".github/workflows/**", ".github/actions/**", "tools/ci/validation-plan.py", "tools/ci/check-plan.py",
      "tools/ci/pins.py", "tools/ci/apt-install.sh", "tools/ci/locked-build-env.sh", "tools/ci/locked-cargo.py", "tools/check.sh",
      "tools/ci/build-inventory.py", "tools/ci/build-provenance.py", "tools/ci/release-plan.py", "tools/ci/signing-config.py",
      "tools/ci/signing-policy.json", "tools/ci/publish-release.py", "tools/ci/native-candidates.json", "tools/ci/native_candidates.py", "tools/ci/run-xvfb.py")
# Called leaf workflows have their own coverage. Other called/shared workflows
# exercise every lane; workflows outside the call graph receive helper checks.
LANE_WORKFLOWS = {
    ".github/workflows/ci.yml": "host",
    ".github/workflows/android-smoke.yml": "smoke",
    ".github/workflows/coverage.yml": "coverage",
    ".github/workflows/fuzz.yml": "fuzz",
    ".github/workflows/import-stress.yml": "stress",
    ".github/workflows/ios.yml": "ios",
    ".github/workflows/credentials.yml": "credentials",
    ".github/workflows/windows.yml": "windows",
    ".github/workflows/windows-package-check.yml": "windows-package",
    ".github/workflows/package-check.yml": "packages",
}
NOTICES = ("about.toml", "LICENSE", "tools/ci/notices.hbs", "tools/ci/component-inventory.py",
           "tools/ci/crypto-native-notices.py", "tools/ci/check-notices.py", "tools/ci/notices.sh",
           "tools/ci/check-package-lock.py", "tools/ci/audit-runners.sh")
ANDROID_PACKAGE = (*NOTICES, *BUILD, "tools/ci/package.Cargo.lock", "hosts/android/**", "crates/iptv-app/build.rs", "crates/iptv-app/nsh.json",
                   "crates/iptv-app/icon.png", "crates/iptv-app/fonts/**", "about.toml", "LICENSE",
                   "tools/sync-host.sh", "tools/ci/android-ndk-env.sh", "tools/ci/check-android*",
                   "tools/ci/sign-android.sh", "tools/ci/prepare-android.sh", "tools/ci/package-android.sh",
                   "tools/ci/merge-android-native.py", "tools/ci/sbom.py",
                   "tools/mpv-runtime/**", "tools/mpv-runtime-unix/**", "crates/iptv-player-mpv/fixtures/**")
LINUX_PACKAGE = ("tools/ci/*linux*.sh", "tools/ci/*linux*.py", "tools/ci/Dockerfile.*-smoke", "hosts/linux/**",
                 "tools/ci/enable-deb-src.sh")
MACOS_PACKAGE = ("tools/package-macos.sh", "tools/bundle-macos.py", "tools/ci/*macos*.sh", "tools/ci/*macos*.py", "hosts/macos/**")
# General Rust build inputs already select native compilation on PRs.
# Run desktop installers for inputs specific to their packaging or shipped resources.
PACKAGES = (*NOTICES, *LINUX_PACKAGE, *MACOS_PACKAGE, "nsh.rev", "crates/iptv-app/build.rs", "crates/iptv-app/nsh.json",
            "crates/iptv-app/icon.png", "crates/iptv-app/fonts/**",
            "tools/ci/native-sources.py", "tools/ci/smoke-desktop.py", "tools/ci/nsh-tool-key.py",
            "tools/ci/native_candidates.py", "tools/ci/setup-linux-packaging.sh",
            "tools/ci/unpack-signing-input.py", "tools/ci/version.py", "tools/mpv-runtime/**", "tools/mpv-runtime-unix/**")
WINDOWS_RESOURCES = ("crates/iptv-app/windows.rc", "crates/iptv-app/icon.ico")
WINDOWS_PACKAGE = (*NOTICES, *WINDOWS_RESOURCES, "crates/iptv-app/build.rs", "hosts/windows/**", "tools/package-windows.ps1", "tools/build-windows.ps1",
                    "tools/fetch-mpv-windows.ps1", "tools/ci/*windows*.ps1", "tools/ci/*windows*.py",
                    "tools/ci/windows-notices.py", "tools/ci/windows-installer.iss", "tools/ci/setup-inno.ps1",
                    "tools/ci/version.py", "tools/mpv-runtime/**", "crates/iptv-app/icon.png", "crates/iptv-app/fonts/**")
SMOKE = (*BUILD, "tools/ci/package.Cargo.lock", "hosts/android/**", "crates/iptv-app/**", "crates/iptv-runtime/**",
         "crates/iptv-player-mpv/**", "crates/iptv-net/**", "crates/iptv-nav/**", "tools/sync-host.sh",
         "tools/ci/android/**", "tools/ci/android-smoke.sh", "tools/ci/disposable-avd.py", "tools/ci/*hls-fixtures.py",
         "tools/ci/android-ndk-env.sh", "tools/make-*fixtures*", "tools/mock-server.py", "tools/mpv-runtime/**")
# Include the parsers, schedules, and models exercised by the allocation fixtures.
STRESS_CRATES = ("iptv-store-sqlite", "iptv-sqlcipher-init", "iptv-import", "iptv-epg",
                 "iptv-providers", "iptv-guide", "iptv-core", "iptv-library", "iptv-settings")
STRESS = (*BUILD, *(f"crates/{crate}/**" for crate in STRESS_CRATES), "tools/ci/import-stress.sh")
COVERAGE = ("tools/ci/coverage.sh", "tools/ci/*coverage*.py")
HOST_INPUTS = ("docs/capabilities.md", "tools/ci/lint.sh", "tools/ci/check-line-length.sh")
OTHER_HOSTS = ("hosts/android/**", "hosts/ios/**", "hosts/linux/**", "hosts/windows/**",
               "hosts/tizen/**", "hosts/tizen-nsh/**")
HELPER_INPUTS = ("docs/contributing.md",)
IOS = (*BUILD, "crates/**/*.rs", "crates/iptv-app/nsh.json", "tools/fetch-mpvkit.sh", "tools/ci/check-ios.sh",
       "tools/check-ios-simulator-entitlements.sh",
       "tools/mpv-runtime-ios/**", "hosts/ios/**")
CREDENTIALS = (*BUILD, "crates/iptv-app/src/host/native/credential_key*", "crates/iptv-app/src/host/native/credential_key/**",
               "crates/iptv-store-sqlite/**", "crates/iptv-sqlcipher-init/**",
               "tools/ci/isolated-credentials.py")
DOCS = ("docs/**", ".gitignore", ".editorconfig")
SHIPPED_MARKDOWN = ("tools/mpv-runtime*/**", "crates/iptv-app/fonts/**", "vendor/**", *HOST_INPUTS)
METADATA = (".github/ISSUE_TEMPLATE/**", ".github/pull_request_template.md", ".github/CODEOWNERS", ".github/dependabot.yml")


def matches(paths, patterns):
    return any(fnmatch.fnmatchcase(path, pattern) for path in paths for pattern in patterns)


def documentation(path):
    # Test the suffix explicitly: fnmatch's * also crosses directory boundaries.
    if matches([path], SHIPPED_MARKDOWN):
        return False
    return matches([path], DOCS) or path.lower().endswith(".md")


def validation_workflows(root=ROOT):
    """Follow local reusable calls without a YAML dependency in the plan job.

    Accept the repository's block-style job `uses` syntax, including quotes.
    If a local reference uses an unfamiliar form or a called file is missing,
    return unknown so workflow changes keep broad validation rather than skip it.
    """
    pending = [".github/workflows/validation.yml"]
    called = set()
    while pending:
        path = pending.pop()
        if path in called:
            continue
        called.add(path)
        try:
            text = (root / path).read_text()
        except (OSError, UnicodeError):
            return None
        in_jobs = False
        for line in text.splitlines():
            stripped = line.lstrip()
            if stripped.startswith("#"):
                continue
            if re.match(r"jobs\s*:", line):
                if re.fullmatch(r"jobs:\s*(?:#.*)?", line) is None:
                    return None
                in_jobs = True
            elif in_jobs and re.match(r"  \S", line):
                if re.fullmatch(r"  [\w.-]+:\s*(?:#.*)?", line) is None:
                    return None
            # Quoted/explicit keys can hide `uses`, including through YAML
            # escape sequences. The lightweight parser does not decode them.
            if re.match(r"\s*(?:- )?['\"].*['\"]\s*:|\s*\?\s|\s*<<\s*:", line):
                return None
            if re.search(r"\buses\s*:", line):
                # Escaped scalars, YAML aliases/tags, dynamic expressions, and
                # flow-style jobs cannot prove a local workflow is unrelated.
                if re.fullmatch(r"\s*(?:- )?uses: (?:['\"][\w./@-]+['\"]|[\w./@-]+)\s*(?:#.*)?", line) is None:
                    return None
            if ".github/workflows/" not in line:
                continue
            match = re.fullmatch(r"    uses: ['\"]?(\./\.github/workflows/[\w.-]+\.ya?ml)['\"]?\s*(?:#.*)?", line)
            if match is None:
                return None
            pending.append(match[1][2:])
        if not in_jobs:
            return None
    return called


def fuzz_targets(paths, full=False, root=ROOT):
    names = sorted(target["name"] for target in tomllib.loads((root / "fuzz/Cargo.toml").read_text())["bin"])
    if full or matches(paths, (*BUILD, *CI, "fuzz/Cargo.toml", "fuzz/Cargo.lock", "tools/ci/fuzz*",
                              "tools/ci/run-fuzz.py", "tools/ci/locked-cargo.py",
                              "crates/iptv-core/**", "crates/iptv-settings/**")):
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


# Heavy lanes a draft pull request defers until it is marked ready for review.
DRAFT_DEFERRED = ("packages", "windows-package", "coverage", "smoke")


def select(paths, tier="pr", root=ROOT, candidate_build=False, draft=False):
    if tier not in ("pr", "full", "release"):
        raise ValueError(f"unknown validation tier: {tier}")
    if type(draft) is not bool or (draft and tier != "pr"):
        raise ValueError("only pull request validation can be a draft")
    if type(candidate_build) is not bool or (candidate_build and tier != "release"):
        raise ValueError("unsigned candidate builds require the release validation tier")
    full = tier != "pr"
    called = validation_workflows(root)
    unrelated = {path for path in paths if path.startswith(".github/workflows/")
                 and called is not None and path not in called}
    lane_workflows = set(LANE_WORKFLOWS) & called if called is not None else set()
    requested = {LANE_WORKFLOWS[path] for path in paths if path in lane_workflows}
    # Remove prose before lane patterns: hosts/** and crates/** also match README.md.
    inputs = [path for path in paths if path not in lane_workflows and path not in unrelated
              and not documentation(path)]
    ci = matches(inputs, CI)
    production = [path for path in inputs if not matches([path], ("crates/iptv-app/tests/**",))]
    rust = matches(inputs, (*BUILD, "crates/**/*.rs"))
    native_rust = matches(production, (*BUILD, "crates/**/*.rs"))
    # Unknown code/build inputs still receive host validation. A documentation
    # change alone never installs GTK, Rust targets, media runtimes, or Android.
    helper_paths = (*METADATA, "tools/ci/**", "tools/mpv-runtime*/**", "tools/*windows*.ps1", "tools/sync-host.sh",
                    "tools/package-*", "tools/bundle-*.py", "tools/mock-server.py", "tools/make-*fixtures*")
    host = (full or ci or "host" in requested or matches(inputs, HOST_INPUTS)
            or any(not documentation(path) and not matches([path], helper_paths) for path in inputs))
    common_package = full or ci or "packages" in requested or any(
        matches([path], PACKAGES) and not matches([path], (*LINUX_PACKAGE, *MACOS_PACKAGE)) for path in inputs)
    platforms = [entry for entry, selected in (
        ({"platform": "linux", "os": "ubuntu-24.04"}, common_package or matches(inputs, LINUX_PACKAGE)),
        ({"platform": "macos", "os": "macos-15"}, common_package or matches(inputs, MACOS_PACKAGE)),
    ) if selected]
    android_package_inputs = full or ci or matches(inputs, ANDROID_PACKAGE)
    result = {
        "tier": tier,
        "candidate_build": candidate_build,
        "ios": full or ci or "ios" in requested or matches(production, IOS),
        "credentials": full or ci or "credentials" in requested or matches(inputs, CREDENTIALS),
        "host": host,
        "helpers_full": full or matches(paths, (*HOST_INPUTS, *HELPER_INPUTS, *METADATA)) or any(not documentation(path) for path in paths),
        "actionlint": full or matches(paths, (".github/workflows/**", ".github/actions/**")),
        "windows": (full or ci or rust or "windows" in requested
                    or matches(inputs, (*WINDOWS_RESOURCES, "hosts/windows/**", "tools/*windows*.ps1"))),
        "android": full or ci or native_rust or android_package_inputs,
        "android_package": False,
        "packages": bool(platforms),
        "package_matrix": {"include": platforms},
        "windows-package": full or ci or "windows-package" in requested or matches(inputs, WINDOWS_PACKAGE),
        # Any Rust source change can move a crate below its coverage floor;
        # catch it before merge instead of on master's gate (#290).
        "coverage": full or ci or "coverage" in requested or matches(inputs, (*COVERAGE, "crates/**/*.rs")),
        "fuzz": fuzz_targets(inputs, full or ci or "fuzz" in requested, root),
        "smoke": full or ci or "smoke" in requested or matches(production, SMOKE),
        "stress": full or ci or "stress" in requested or matches(inputs, STRESS),
    }
    if tier == "release":
        # The release pipeline builds and validates the shipping packages once,
        # in clean jobs. Avoid constructing throwaway installers first.
        result["package_matrix"] = {"include": []}
        for lane in ("android", "android_package", "packages", "windows-package"):
            result[lane] = False
    # A draft gets fast feedback; its deferred lanes run once it is marked
    # ready. The gate fails a draft run, so its result never stands in for them.
    result["draft"] = draft
    result["deferred"] = [lane for lane in DRAFT_DEFERRED if result[lane]] if draft else []
    if draft:
        result["package_matrix"] = {"include": []}
        for lane in DRAFT_DEFERRED:
            result[lane] = False
    macos_host = host and (full or "host" in requested or any(
        not documentation(path) and not matches([path], OTHER_HOSTS) for path in inputs))
    macos_package = any(entry["platform"] == "macos" for entry in result["package_matrix"]["include"])
    result["macos_host"] = ("packages" if macos_package else "host") if macos_host else "skip"
    for lane in HOST_VERIFICATIONS:
        # A PR editing the standalone workflow must exercise that entry point.
        has_host = macos_host if lane == "ios" else host
        result[f"{lane}_on_host"] = result[lane] and has_host and (tier != "pr" or lane not in requested)
    return result


def gate(plan, needs):
    errors = []
    if needs.get("plan", {}).get("result") != "success":
        errors.append("change detection did not succeed")
    if needs.get("helpers", {}).get("result") != "success":
        errors.append("validation helper checks did not succeed")
    if not isinstance(plan, dict) or set(LANES) - plan.keys():
        return errors + ["validation plan is missing or incomplete"]
    tier = plan.get("tier")
    if tier not in ("pr", "full", "release"):
        return errors + ["validation tier is missing or invalid"]
    draft, deferred = plan.get("draft", False), plan.get("deferred", [])
    if (type(draft) is not bool or not isinstance(deferred, list) or not set(deferred) <= set(DRAFT_DEFERRED)
            or (draft and tier != "pr") or (deferred and not draft)):
        errors.append("draft selection is missing or invalid")
    elif draft:
        errors.append("draft pull request: the selected fast checks ran"
                      + (f"; {', '.join(deferred)} run when it is marked ready for review" if deferred else "")
                      + ". Mark it ready for review for a mergeable result.")
    candidate = plan.get("candidate_build")
    if type(candidate) is not bool:
        errors.append("unsigned candidate selection is missing or invalid")
    elif needs.get("candidate", {}).get("result") not in (("success",) if candidate else ("success", "skipped")):
        errors.append("unsigned candidate packages did not meet the validation plan")
    macos_host = plan.get("macos_host")
    if (macos_host not in ("skip", "host", "packages")
            or (macos_host != "skip" and plan["host"] is not True)
            or (macos_host == "packages" and (plan["packages"] is not True or tier == "release"))
            or (tier in ("full", "release") and macos_host == "skip")):
        errors.append("macOS host routing is missing or invalid")
    elif macos_host != "skip" and needs.get(macos_host, {}).get("outputs", {}).get("macos-host-verified") != "true":
        errors.append("macOS host checks did not report success")
    for lane in LANES:
        if (lane == "fuzz" and not isinstance(plan[lane], list)) or (lane != "fuzz" and type(plan[lane]) is not bool):
            errors.append(f"invalid selection for {lane}")
            continue
        result = needs.get(lane, {}).get("result")
        selected = bool(plan[lane])
        if lane in HOST_VERIFICATIONS:
            on_host = plan.get(f"{lane}_on_host")
            has_host = macos_host in ("host", "packages") if lane == "ios" else plan["host"] is True
            eligible = selected and has_host
            if (type(on_host) is not bool or (on_host and not eligible)
                    or (tier != "pr" and on_host != eligible)):
                errors.append(f"host routing is missing or invalid for {lane}")
            if on_host is True:
                job = macos_host if lane == "ios" else "host"
                evidence = needs.get(job, {}).get("outputs", {}).get(HOST_VERIFICATIONS[lane])
                if result != "skipped" or evidence != "true":
                    errors.append(f"selected {lane}: missing successful host verification")
                continue
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
    draft = event == "pull_request" and os.environ.get("DRAFT", "false") == "true"
    plan = select(paths, tier, candidate_build=os.environ.get("CANDIDATE_BUILD", "false") == "true", draft=draft)
    print("plan=" + json.dumps(plan, separators=(",", ":")))
    for key, value in plan.items():
        print(f"{key}=" + (str(value).lower() if isinstance(value, bool) else json.dumps(value) if isinstance(value, (list, dict)) else value))
    return 0


if __name__ == "__main__":
    sys.exit(main())
