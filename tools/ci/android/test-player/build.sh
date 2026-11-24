#!/usr/bin/env bash
# Builds the synthetic external player used to validate Patina's handoff on
# an emulator (#283): an app that takes http video/* from ACTION_VIEW and
# plays it with MediaPlayer, logging its position under the tag TestPlayer.
#
#   tools/ci/android/test-player/build.sh [OUT_DIR]   # default target/test-player
#   adb -s emulator-<port> install -r target/test-player/test-player.apk
#
# Needs a JDK and the Android SDK (ANDROID_HOME, default ~/Android/Sdk) with
# build-tools and an android-35 or later platform. Signed with the debug key.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-target/test-player}
sdk=${ANDROID_HOME:-$HOME/Android/Sdk}
tools=$(ls -d "$sdk"/build-tools/* | sort -V | tail -n 1)
platform=$(ls -d "$sdk"/platforms/android-* | sort -V | tail -n 1)
keystore=${DEBUG_KEYSTORE:-$HOME/.android/debug.keystore}
rm -rf "$out" && mkdir -p "$out/classes"
javac --release 11 -classpath "$platform/android.jar" -d "$out/classes" \
  "$here"/src/org/example/testplayer/*.java
"$tools/d8" --release --min-api 26 --lib "$platform/android.jar" \
  --output "$out" "$out"/classes/org/example/testplayer/*.class
"$tools/aapt2" link -o "$out/unsigned.apk" -I "$platform/android.jar" \
  --manifest "$here/AndroidManifest.xml"
(cd "$out" && zip -q unsigned.apk classes.dex)
"$tools/zipalign" -f 4 "$out/unsigned.apk" "$out/aligned.apk"
"$tools/apksigner" sign --ks "$keystore" --ks-pass pass:android \
  --out "$out/test-player.apk" "$out/aligned.apk"
echo "$out/test-player.apk"
