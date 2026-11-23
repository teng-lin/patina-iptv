#!/usr/bin/env bash
# Build and validate unsigned artifacts before the protected signing job.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${VERSION:?}" "${ANDROID_HOME:?}"
# Never inherit signing material when running dependency build/plugin code.
unset KEYSTORE_PATH STORE_PASSWORD KEY_PASSWORD KEY_ALIAS KEYSTORE
export IPTV_UNSIGNED_RELEASE=1
notices=${1:-target/notices}
if [[ $# == 0 ]]; then
  bash tools/ci/notices.sh
else
  # The native build audits the generated runner workspace.
  test -s "$notices/NSH-RUNNERS.html"
  test -s "$notices/runner-components.json"
fi
(cd hosts/android && ./gradlew --no-daemon :app:generateLicenseReport)
rm -rf hosts/android/app/build/nsh/assets/notices
mkdir -p hosts/android/app/build/nsh/assets/notices
cp -R "$notices/." hosts/android/app/build/nsh/assets/notices/
cp -R hosts/android/app/build/reports/dependency-license hosts/android/app/build/nsh/assets/notices/gradle
(cd hosts/android && ./gradlew --no-daemon testReleaseUnitTest lintRelease assembleRelease bundleRelease)
# The signed AAB retains the exact R8 map for this build.
mapping=hosts/android/app/build/outputs/mapping/release/mapping.txt
bundle=hosts/android/app/build/outputs/bundle/release/app-release.aab
test -s "$mapping"
unzip -p "$bundle" BUNDLE-METADATA/com.android.tools.build.obfuscation/proguard.map | cmp "$mapping" -
rm -rf target/android-unsigned
mkdir -p target/android-unsigned
python3 tools/ci/sbom.py android-classpath \
  hosts/android/app/build/reports/dependency-license/index.json \
  "target/android-unsigned/Patina-$VERSION-android-classpath-sbom.cdx.json" "$VERSION"
cp hosts/android/app/build/outputs/apk/release/app-release-unsigned.apk target/android-unsigned/
cp hosts/android/app/build/outputs/bundle/release/app-release.aab target/android-unsigned/
python3 tools/ci/check-android-artifacts.py \
  target/android-unsigned/app-release-unsigned.apk target/android-unsigned/app-release.aab
