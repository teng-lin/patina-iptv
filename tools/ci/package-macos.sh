#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${VERSION:?}"
if [ "${IPTV_SIGNING_STAGE:-}" = 1 ]; then
  python3 tools/ci/signing-config.py macos
else
  python3 tools/ci/signing-config.py macos --prepare
fi
test -d target/package/Patina.app
if [ -n "${MACOS_CERTIFICATE:-}" ]; then
  : "${MACOS_CERTIFICATE_PASSWORD:?}"
  umask 077
  keychain="$RUNNER_TEMP/macos-release.keychain-db"
  password=$(openssl rand -hex 24)
  echo "::add-mask::$password"
  printf '%s' "$MACOS_CERTIFICATE" | base64 --decode > "$RUNNER_TEMP/macos.p12"
  security create-keychain -p "$password" "$keychain"
  security set-keychain-settings -lut 21600 "$keychain"
  security unlock-keychain -p "$password" "$keychain"
  security import "$RUNNER_TEMP/macos.p12" -P "$MACOS_CERTIFICATE_PASSWORD" -A -t cert -f pkcs12 -k "$keychain"
  security list-keychains -d user -s "$keychain"
  security set-key-partition-list -S apple-tool:,apple:,codesign: -k "$password" "$keychain"
  bash tools/ci/notarize-macos.sh target/package/Patina.app
else
  # Released as a preview: the release notes and README say it is unsigned.
  echo "Developer ID certificate absent: producing an ad hoc signed, unnotarized preview package"
fi
rm -rf target/dmg
mkdir -p dist target/dmg
cp -R target/package/Patina.app target/dmg/
ln -s /Applications target/dmg/Applications
dmg="$PWD/dist/Patina-$VERSION-macos-arm64.dmg"
hdiutil create -volname Patina -srcfolder target/dmg -ov -format UDZO "$dmg"
if [ -n "${MACOS_CERTIFICATE:-}" ]; then
  codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "$dmg"
  xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" --wait
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
fi
python3 tools/ci/verify-macos-dmg.py "$dmg"
mount="$PWD/target/dmg-mounted"
mkdir -p "$mount"
hdiutil attach "$dmg" -readonly -nobrowse -mountpoint "$mount"
trap 'hdiutil detach "$mount"' EXIT
app="$mount/Patina.app"
python3 tools/ci/check-notices.py "$app" mpv-runtime/components.json
codesign --verify --deep --strict "$app"
python3 - "$app" <<'PY'
from pathlib import Path
import os
import plistlib
import subprocess
import sys
app = Path(sys.argv[1])
info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
executable = app / "Contents/MacOS" / info["CFBundleExecutable"]
subprocess.run(["lipo", str(executable), "-verify_arch", "arm64"], check=True)
print("Minimum macOS:", info["LSMinimumSystemVersion"])
# Execute the app only during unsigned preparation, before importing a keychain.
if not os.environ.get("IPTV_SIGNING_STAGE"):
    subprocess.run(["python3", "tools/ci/smoke-desktop.py", str(executable)], check=True)
PY
