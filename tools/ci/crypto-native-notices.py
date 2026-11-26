#!/usr/bin/env python3
"""Preserve vendored native licenses, beyond the Rust source-wrapper licenses."""
from pathlib import Path
import hashlib
import json
import re
import subprocess
import sys


def sqlcipher_notices(source):
    notices = list(dict.fromkeys(
        match.group(0) for match in re.finditer(r"/\*.*?\*/", source, re.S)
        if re.search(r"copyright|redistribution|blessing|public license", match.group(0), re.I)
    ))
    if not any("ZETETIC" in notice and "Redistribution" in notice for notice in notices):
        raise ValueError("SQLCipher source is missing its upstream redistribution notice")
    return "\n\n".join(notices) + "\n"


def main():
    output = Path(sys.argv[1])
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--format-version", "1"
    ]))
    output.mkdir(parents=True, exist_ok=True)
    records = []
    for name, relative, destination in [
        ("sqlcipher-src", "sqlcipher/sqlcipher.c", "SQLCipher-SQLite-NOTICES.txt"),
        ("aws-lc-sys", "aws-lc/LICENSE", "AWS-LC-LICENSE.txt"),
        ("openssl-src", "openssl/LICENSE.txt", "OpenSSL-LICENSE.txt"),
    ]:
        packages = [package for package in metadata["packages"] if package["name"] == name]
        if len(packages) != 1:
            raise ValueError(f"expected exactly one resolved native source package: {name}")
        package = packages[0]
        source = Path(package["manifest_path"]).parent / relative
        content = source.read_bytes()
        text = sqlcipher_notices(content.decode()) if name == "sqlcipher-src" else content.decode()
        if not text.strip():
            raise ValueError(f"empty native license: {name}")
        (output / destination).write_text(text)
        records.append({
            "package": name, "version": package["version"], "registry_source": package["source"],
            "source_file": relative, "source_sha256": hashlib.sha256(content).hexdigest(),
            "notice": destination,
            "notice_sha256": hashlib.sha256(text.encode()).hexdigest(),
        })
    (output / "components.json").write_text(json.dumps(records, indent=2) + "\n")


if __name__ == "__main__":
    main()
