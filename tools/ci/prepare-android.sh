#!/usr/bin/env bash
# Build and validate unsigned artifacts before the protected signing job.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${VERSION:?}" "${ANDROID_HOME:?}"
# Never inherit signing material when running dependency build/plugin code.
unset KEYSTORE_PATH STORE_PASSWORD KEY_PASSWORD KEY_ALIAS KEYSTORE
export IPTV_UNSIGNED_RELEASE=1
bash tools/ci/notices.sh
(cd hosts/android && ./gradlew --no-daemon :app:generateLicenseReport)
rm -rf hosts/android/app/build/nsh/assets/notices
mkdir -p hosts/android/app/build/nsh/assets/notices
cp -R target/notices/. hosts/android/app/build/nsh/assets/notices/
cp -R hosts/android/app/build/reports/dependency-license hosts/android/app/build/nsh/assets/notices/gradle
(cd hosts/android && ./gradlew --no-daemon lintRelease assembleRelease bundleRelease)
rm -rf target/android-unsigned
mkdir -p target/android-unsigned
python3 tools/ci/sbom.py android-classpath \
  hosts/android/app/build/reports/dependency-license/index.json \
  "target/android-unsigned/Patina-$VERSION-android-classpath-sbom.cdx.json" "$VERSION"
cp hosts/android/app/build/outputs/apk/release/app-release-unsigned.apk target/android-unsigned/
cp hosts/android/app/build/outputs/bundle/release/app-release.aab target/android-unsigned/
python3 tools/ci/check-android-artifacts.py \
  target/android-unsigned/app-release-unsigned.apk target/android-unsigned/app-release.aab
