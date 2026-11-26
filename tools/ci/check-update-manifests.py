#!/usr/bin/env python3
"""Inspect the distributed APK and every module manifest in the store bundle."""
from pathlib import Path
import os
import subprocess
import sys
import xml.etree.ElementTree as ET
import zipfile

PERMISSION = "android.permission.REQUEST_INSTALL_PACKAGES"
ACTIVITY = "com.hantekllc.iptvrs.host.DirectUpdateActivity"
ANDROID = "{http://schemas.android.com/apk/res/android}"


def check(apk, aab):
    analyzer = Path(os.environ["ANDROID_HOME"]) / "cmdline-tools/latest/bin/apkanalyzer"
    manifest = ET.fromstring(subprocess.check_output([analyzer, "manifest", "print", apk]))
    if not any(p.get(ANDROID + "name") == PERMISSION for p in manifest.findall("uses-permission")):
        raise ValueError("direct APK lacks install permission")
    activities = [a for a in manifest.findall("application/activity") if a.get(ANDROID + "name") == ACTIVITY]
    if len(activities) != 1 or activities[0].get(ANDROID + "exported") != "false":
        raise ValueError("direct APK installer must be private")
    with zipfile.ZipFile(aab) as bundle:
        # AAPT2's protobuf XmlNode stores element and attribute strings as UTF-8.
        # Requiring the base manifest and a known cleartext permission also
        # rejects an unrecognized manifest encoding.
        base = bundle.read("base/manifest/AndroidManifest.xml")
        if b"android.permission.INTERNET" not in base:
            raise ValueError("unknown store manifest encoding")
        for name in bundle.namelist():
            if name.endswith("/manifest/AndroidManifest.xml"):
                data = bundle.read(name)
                if PERMISSION.encode() in data or b"DirectUpdateActivity" in data:
                    raise ValueError(f"store module contains direct install capability: {name}")


if __name__ == "__main__":
    check(*sys.argv[1:])
