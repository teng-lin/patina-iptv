#!/usr/bin/env bash
set -euo pipefail
# The caller imports a Developer ID certificate into the isolated keychain.
: "${MACOS_SIGN_IDENTITY:?}" "${APPLE_ID:?}" "${APPLE_TEAM_ID:?}" "${APPLE_APP_PASSWORD:?}"
app=$1
# Sign inside-out; every bundled executable receives the hardened runtime.
find "$app/Contents" -type f \( -name '*.dylib' -o -perm -111 \) -print0 |
  while IFS= read -r -d '' binary; do
    if file "$binary" | grep -q 'Mach-O'; then
      codesign --force --options runtime --timestamp --preserve-metadata=entitlements --sign "$MACOS_SIGN_IDENTITY" "$binary"
    fi
  done
# Framework/XPC/app resource envelopes must be sealed after their executable
# children. Sparkle includes nested helper bundles; signing only loose binaries
# leaves their enclosing signatures invalid. Do not use --deep for signing.
python3 - "$app" "$MACOS_SIGN_IDENTITY" <<'PY'
from pathlib import Path
import subprocess
import sys
app, identity = Path(sys.argv[1]), sys.argv[2]
bundles = [p for p in (app / "Contents").rglob("*")
           if p.is_dir() and not p.is_symlink() and p.suffix in (".app", ".xpc", ".framework")]
for bundle in sorted(bundles, key=lambda p: len(p.parts), reverse=True):
    subprocess.run(["codesign", "--force", "--options", "runtime", "--timestamp",
                    "--preserve-metadata=entitlements", "--sign", identity, str(bundle)], check=True)
PY
codesign --force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$app"
# Every Mach-O, the pinned libmpv runtime in Contents/Frameworks included,
# must carry this identity's signature with the hardened runtime: notarization
# rejects anything less, and library validation refuses to load a dylib signed
# by anyone else.
find "$app/Contents" -type f \( -name '*.dylib' -o -perm -111 \) -print0 |
  while IFS= read -r -d '' binary; do
    if file "$binary" | grep -q 'Mach-O'; then
      details=$(codesign --display --verbose=2 "$binary" 2>&1)
      if ! grep -q 'flags=.*(runtime)' <<<"$details" || ! grep -q '^Timestamp=' <<<"$details"; then
        echo "$binary lacks the hardened runtime or a secure timestamp" >&2
        exit 1
      fi
    fi
  done
codesign --verify --deep --strict --verbose=2 "$app"
ditto -c -k --keepParent "$app" "$RUNNER_TEMP/notarize.zip"
xcrun notarytool submit "$RUNNER_TEMP/notarize.zip" --apple-id "$APPLE_ID" \
  --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
xcrun stapler staple "$app"
spctl --assess --type execute --verbose "$app"
