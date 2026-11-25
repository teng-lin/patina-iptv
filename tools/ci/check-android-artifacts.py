#!/usr/bin/env python3
"""Require packaged notices and native code for every requested Android ABI."""
import os
from pathlib import Path
import re
import sys
import zipfile


def check(artifact, abis):
    prefix = "base/" if artifact.suffix == ".aab" else ""
    with zipfile.ZipFile(artifact) as package:
        if artifact.suffix == ".aab":
            # This metadata survives the isolated signing job and is the map
            # needed to symbolicate this exact bundle's Java/Kotlin crashes.
            name = "BUNDLE-METADATA/com.android.tools.build.obfuscation/proguard.map"
            try:
                mapping = package.read(name).decode("utf-8")
            except KeyError:
                raise ValueError(f"{artifact}: missing R8 mapping") from None
            if not mapping.startswith("# compiler: R8\n") or not re.search(
                    r"(?m)^\S+ -> \S+:$", mapping):
                raise ValueError(f"{artifact}: invalid R8 mapping")
        for abi in abis:
            library = prefix + "lib/" + abi + "/libnsh.so"
            try:
                entry = package.getinfo(library)
            except KeyError:
                raise ValueError(f"{artifact}: missing required native library {library}") from None
            with package.open(entry) as stream:
                if entry.file_size <= 4 or stream.read(4) != b"\x7fELF":
                    raise ValueError(f"{artifact}: invalid native library {library}")
        for name in ("THIRD-PARTY.html", "components.json", "NSH-RUNNERS.html", "runner-components.json"):
            entry = package.getinfo(prefix + "assets/notices/" + name)
            if not entry.file_size:
                raise ValueError(f"{artifact}: empty notice {name}")
        entry = package.getinfo(prefix + "assets/notices/gradle/index.json")
        if not entry.file_size:
            raise ValueError(f"{artifact}: empty Gradle inventory")


if __name__ == "__main__":
    abis = os.environ.get("ABIS", "arm64-v8a,armeabi-v7a").replace(",", " ").split()
    if not abis or not set(abis) <= {"arm64-v8a", "armeabi-v7a", "x86_64", "x86"}:
        raise SystemExit("ABIS must list supported Android architectures")
    if len(sys.argv) < 2:
        raise SystemExit("supply APK and/or AAB artifacts")
    for name in sys.argv[1:]:
        check(Path(name), abis)
