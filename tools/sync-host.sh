#!/usr/bin/env bash
# Builds the Rust crate's Android libnsh.so with nsh and copies it, plus the
# app's NSH assets, into the Gradle host (hosts/android/app/build/nsh).
# Usage: tools/sync-host.sh [--release] [--no-build]
# --release: arm64-v8a only (ABIS=... to choose), smallest library.
# ABIS also limits development builds (default x86_64,arm64-v8a).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
source "$root/tools/ci/locked-build-env.sh"
app="$root/crates/iptv-app"
nsh=${NSH:-$root/../nsh/target/release/nsh}
out="$root/hosts/android/app/build/nsh"
export JAVA_HOME=${JAVA_HOME:-/usr/lib/jvm/default}
# The Android SDK and NDK locations, and the NDK compiler for C dependencies.
source "$root/tools/ci/android-ndk-env.sh"
# Development builds link fast; `--release` builds the smallest library
# (arm64 only: 44 MB -> 17 MB stripped with these and nsh.json "features").
abis=${ABIS:-x86_64,arm64-v8a}
if [ "${1:-}" = "--release" ]; then
  shift
  abis=${ABIS:-arm64-v8a}
  export CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
  export CARGO_PROFILE_RELEASE_OPT_LEVEL=s
fi
export CARGO_PROFILE_RELEASE_LTO=${CARGO_PROFILE_RELEASE_LTO:-thin}
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=${CARGO_PROFILE_RELEASE_CODEGEN_UNITS:-16}

if [ "${1:-}" != "--no-build" ]; then
  # x86_64 for emulators, arm64-v8a for real TV boxes and tablets.
  (cd "$app" && "$nsh" package --target android --abi "$abis")
fi

# Newest APK nsh produced; a pattern with no match must not end the script (pipefail).
shopt -s nullglob
apks=("$app"/.nsh/out/*.apk "$app"/.nsh/rust/android/*.apk)
shopt -u nullglob
[ ${#apks[@]} -gt 0 ] || { echo "no APK under $app/.nsh: build the crate first" >&2; exit 1; }
# shellcheck disable=SC2012
apk=$(ls -t "${apks[@]}" | head -1)
lib="$app/.nsh/rust/android-rust-release/lib"
[ -d "$lib" ] || { echo "no $lib: build the crate first" >&2; exit 1; }

rm -rf "$out" && mkdir -p "$out/jniLibs" "$out/assets"
# Only the ABIs this run built: an older build's would ship stale code.
for abi in ${abis//,/ }; do
  [ -f "$lib/$abi/libnsh.so" ] || { echo "no $lib/$abi/libnsh.so" >&2; exit 1; }
  cp -r "$lib/$abi" "$out/jniLibs/"
done
# The app's NSH assets (app.json, images, translations) as nsh packaged them.
(cd "$out/assets" && unzip -q -o "$apk" 'assets/nsh/*' && mv assets/nsh nsh && rmdir assets)
echo "synced from $(basename "$apk"):"
find "$out" -type f | sed "s#$out/##"
