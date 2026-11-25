#!/usr/bin/env python3
"""Read an iOS bundle without printing signing identities or profile contents.

This inventory is evidence for review, not an export-compliance determination,
required-reason API audit, or proof of App Store acceptance.
"""
import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import subprocess


def command(arguments):
    result = subprocess.run(arguments, capture_output=True, check=False, timeout=60)
    return result.returncode, result.stdout, result.stderr


def inspect(bundle, run=command):
    info = plistlib.loads((bundle / "Info.plist").read_bytes())
    executable = bundle / info["CFBundleExecutable"]
    if executable.parent != bundle or not executable.is_file():
        raise ValueError("Bundle executable is missing or outside the bundle root")
    result = {
        "info": {key: info.get(key) for key in (
            "CFBundleIdentifier", "CFBundleShortVersionString", "CFBundleVersion",
            "CFBundleSupportedPlatforms", "MinimumOSVersion", "UIDeviceFamily",
            "DTXcode", "DTSDKName", "ITSAppUsesNonExemptEncryption", "NSAppTransportSecurity",
        )},
        "executable_sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
        "embedded_profile_present": (bundle / "embedded.mobileprovision").is_file(),
        "frameworks": sorted(path.name for path in bundle.rglob("*.framework") if path.is_dir()),
        "privacy_manifests": [],
    }
    for path in sorted(bundle.rglob("PrivacyInfo.xcprivacy")):
        manifest = plistlib.loads(path.read_bytes())
        result["privacy_manifests"].append({
            "path": str(path.relative_to(bundle)),
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "tracking": manifest.get("NSPrivacyTracking"),
            "collected_data_types": sorted(item["NSPrivacyCollectedDataType"] for item in
                                           manifest.get("NSPrivacyCollectedDataTypes", [])),
            "accessed_api_reasons": manifest.get("NSPrivacyAccessedAPITypes", []),
        })
    code, stdout, stderr = run(["codesign", "--display", "--verbose=4", str(bundle)])
    signature = (stdout + stderr).decode(errors="replace")
    result["signature_kind"] = (
        "absent" if code else "ad-hoc" if "Signature=adhoc" in signature else "certificate"
    )
    code, _, _ = run(["codesign", "--verify", "--deep", "--strict", str(bundle)])
    result["signature_verifies"] = code == 0
    # Store/signing identities, entitlements and provisioning-profile contents
    # deliberately never enter the report. Inspect those privately for a signed
    # distribution candidate; a certificate alone does not prove distribution.
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    args = parser.parse_args()
    try:
        report = inspect(args.bundle)
    except (KeyError, ValueError, OSError, plistlib.InvalidFileException, subprocess.TimeoutExpired):
        parser.exit(1, "Cannot inspect the bundle; verify its files and local codesign tool privately.\n")
    print(json.dumps(report, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
