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


def privacy_fields(manifest):
    if not isinstance(manifest, dict):
        raise ValueError("Privacy manifest must be a dictionary")
    tracking = manifest.get("NSPrivacyTracking")
    domains = manifest.get("NSPrivacyTrackingDomains", [])
    collected = manifest.get("NSPrivacyCollectedDataTypes", [])
    reasons = manifest.get("NSPrivacyAccessedAPITypes", [])
    if tracking is not None and not isinstance(tracking, bool):
        raise ValueError("Invalid tracking flag")
    if not isinstance(domains, list) or any(not isinstance(domain, str) for domain in domains):
        raise ValueError("Invalid tracking domains")
    for entries, name, arrays, flags in (
        (collected, "NSPrivacyCollectedDataType", ["NSPrivacyCollectedDataTypePurposes"],
         ["NSPrivacyCollectedDataTypeLinked", "NSPrivacyCollectedDataTypeTracking"]),
        (reasons, "NSPrivacyAccessedAPIType", ["NSPrivacyAccessedAPITypeReasons"], []),
    ):
        if not isinstance(entries, list):
            raise ValueError("Privacy declarations must be arrays")
        for item in entries:
            if not isinstance(item, dict) or not isinstance(item.get(name), str):
                raise ValueError("Invalid privacy declaration")
            for key in arrays:
                values = item.get(key)
                if not isinstance(values, list) or any(not isinstance(value, str) for value in values):
                    raise ValueError("Invalid privacy purpose or reason array")
            if any(not isinstance(item.get(key), bool) for key in flags):
                raise ValueError("Invalid collected-data flags")
    return {"tracking": tracking, "tracking_domains": domains,
            "collected_data_types": collected, "accessed_api_reasons": reasons}


def inspect(bundle, run=command):
    bundle = bundle.resolve(strict=True)
    info_path = bundle / "Info.plist"
    if info_path.is_symlink() or not info_path.is_file():
        raise ValueError("Bundle information is not a regular file")
    info = plistlib.loads(info_path.read_bytes())
    if not isinstance(info, dict) or not isinstance(info.get("CFBundleExecutable"), str):
        raise ValueError("Bundle information has an invalid executable name")
    executable = bundle / info["CFBundleExecutable"]
    if (executable.parent != bundle or executable.is_symlink() or not executable.is_file()
            or executable.resolve().parent != bundle):
        raise ValueError("Bundle executable is missing or outside the bundle root")
    result = {
        "info": {key: info.get(key) for key in (
            "CFBundleIdentifier", "CFBundleShortVersionString", "CFBundleVersion",
            "CFBundleSupportedPlatforms", "MinimumOSVersion", "UIDeviceFamily",
            "DTXcode", "DTSDKName", "ITSAppUsesNonExemptEncryption", "NSAppTransportSecurity",
            "UISupportedInterfaceOrientations", "UISupportedInterfaceOrientations~ipad",
            "NSLocalNetworkUsageDescription",
        )},
        "executable_sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
        "embedded_profile_present": (bundle / "embedded.mobileprovision").is_file(),
        "frameworks": sorted(path.name for path in bundle.rglob("*.framework") if path.is_dir()),
        "privacy_manifests": [],
    }
    for path in sorted(bundle.rglob("PrivacyInfo.xcprivacy")):
        if path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(bundle):
            raise ValueError("Privacy manifest is not a regular bundle file")
        manifest = plistlib.loads(path.read_bytes())
        result["privacy_manifests"].append({
            "path": str(path.relative_to(bundle)),
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            **privacy_fields(manifest),
        })
    code, stdout, stderr = run(["codesign", "--display", "--verbose=4", "--", str(bundle)])
    signature = (stdout + stderr).decode(errors="replace").splitlines()
    kind = "unknown"
    if code:
        if any(line.endswith(": code object is not signed at all") for line in signature):
            kind = "absent"
    elif "Signature=adhoc" in signature:
        kind = "ad-hoc"
    elif any(line.startswith("Authority=") for line in signature):
        kind = "certificate"
    result["signature_kind"] = kind
    code, _, _ = run(["codesign", "--verify", "--deep", "--strict", "--", str(bundle)])
    result["signature_verifies"] = code == 0
    # Store/signing identities, entitlements and provisioning-profile contents
    # deliberately never enter the report. Inspect those privately for a signed
    # distribution candidate; a certificate alone does not prove distribution.
    # Reject non-JSON plist values (for example binary data or dates) instead
    # of risking an unredacted serialization traceback in callers.
    json.dumps(result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    args = parser.parse_args()
    try:
        report = json.dumps(inspect(args.bundle), indent=2, sort_keys=True)
    except Exception:
        parser.exit(1, "Cannot inspect the bundle; verify its files and local codesign tool privately.\n")
    print(report)


if __name__ == "__main__":
    main()
