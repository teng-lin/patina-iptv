#!/usr/bin/env bash
# Sign previously validated APK/AAB files without running dependency build code.
# Signing material lives outside the checkout (RUNNER_TEMP in CI).
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${VERSION:?}" "${ANDROID_HOME:?}" "${KEYSTORE_PATH:?}"
: "${STORE_PASSWORD:?}" "${KEY_PASSWORD:?}" "${KEY_ALIAS:?}"
test -r "$KEYSTORE_PATH"

# This job receives only unsigned artifacts; never run project build tools here.
unsigned=target/android-unsigned
python3 tools/ci/check-android-artifacts.py "$unsigned/app-release-unsigned.apk" "$unsigned/app-release.aab"
mkdir -p dist
if [ -f "$unsigned/Patina-$VERSION-android-classpath-sbom.cdx.json" ]; then
  cp "$unsigned/Patina-$VERSION-android-classpath-sbom.cdx.json" dist/
fi
apk="dist/Patina-$VERSION-android-tablet-tv.apk"
aab="dist/Patina-$VERSION-android-tablet-tv.aab"
apksigner="$ANDROID_HOME/build-tools/35.0.1/apksigner"
"$apksigner" sign --v4-signing-enabled false --ks "$KEYSTORE_PATH" --ks-key-alias "$KEY_ALIAS" \
  --ks-pass env:STORE_PASSWORD --key-pass env:KEY_PASSWORD \
  --out "$apk" "$unsigned/app-release-unsigned.apk"
cp "$unsigned/app-release.aab" "$aab"
jarsigner -keystore "$KEYSTORE_PATH" -storepass:env STORE_PASSWORD \
  -keypass:env KEY_PASSWORD "$aab" "$KEY_ALIAS"
"$apksigner" verify "$apk"
jarsigner -verify "$aab"

# Verify both final artifacts carry the selected release certificate.
expected=$(keytool -exportcert -keystore "$KEYSTORE_PATH" -alias "$KEY_ALIAS" \
  -storepass:env STORE_PASSWORD | sha256sum | cut -d' ' -f1)
apk_cert=$("$apksigner" verify --print-certs "$apk" | sed -n 's/^Signer #1 certificate SHA-256 digest: //p')
aab_cert=$(keytool -printcert -rfc -jarfile "$aab" | openssl x509 -outform DER | sha256sum | cut -d' ' -f1)
test "$apk_cert" = "$expected"
test "$aab_cert" = "$expected"

python3 tools/ci/check-android-artifacts.py "$apk" "$aab"
