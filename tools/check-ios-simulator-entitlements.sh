#!/usr/bin/env bash
# Check that a simulator build carries the entitlements its Keychain needs (#136).
#
#   tools/check-ios-simulator-entitlements.sh target/ios-sim/Patina.app
#   tools/check-ios-simulator-entitlements.sh <a .rlib, .a or executable>
#
# The simulator reads entitlements from the executable's __TEXT,__entitlements
# section, not from the ad hoc signature, so this reads that section and checks
# it grants the app identifier and the Keychain access group of nsh.json's id.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
target=${1:?usage: $0 <Patina.app | executable | rlib>}
if [ -d "$target" ]; then
  executable=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$target/Info.plist")
  target="$target/$executable"
fi
python3 - "$target" "$root/crates/iptv-app/nsh.json" <<'EOF'
import json, plistlib, re, subprocess, sys

binary, config = sys.argv[1:]
expected = json.load(open(config))["id"]
dump = subprocess.run(["otool", "-X", "-s", "__TEXT", "__entitlements", binary],
                      check=True, capture_output=True, text=True).stdout
data = bytearray()
for line in dump.splitlines():
    # "<address>\t<bytes or words>": otool prints 32-bit words on arm64.
    fields = line.split("\t", 1)
    if len(fields) != 2:
        continue
    for word in fields[1].split():
        if not re.fullmatch(r"[0-9a-f]+", word):
            continue
        chunk = bytes.fromhex(word)
        data += chunk[::-1] if len(word) == 8 else chunk
if not data:
    sys.exit(f"{binary}: no __TEXT,__entitlements section; the simulator Keychain will refuse the app")
entitlements = plistlib.loads(bytes(data).rstrip(b"\0"))
problems = []
if entitlements.get("application-identifier") != expected:
    problems.append(f"application-identifier is {entitlements.get('application-identifier')!r}, not {expected!r}")
if expected not in entitlements.get("keychain-access-groups", []):
    problems.append(f"keychain-access-groups {entitlements.get('keychain-access-groups')!r} lacks {expected!r}")
if problems:
    sys.exit(f"{binary}: " + "; ".join(problems))
print(f"{binary}: simulator entitlements grant {expected} its Keychain access group")
EOF
