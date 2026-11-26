#!/usr/bin/env bash
# Build the isolated probe with the Rust toolchain and Apple media inputs used by Patina.
set -euo pipefail
case "${1:-}" in
  macos|ios|ios-simulator) platform=$1 ;;
  *) echo "Usage: $0 {macos|ios|ios-simulator}" >&2; exit 2 ;;
esac
project=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$project/../.." && pwd)
cd "$root"
source "$root/tools/ci/locked-build-env.sh"
nsh=${NSH:-$root/../nsh-patina/target/release/nsh}
out="$root/target/airplay-probe/$platform"
mkdir -p "$project/.nsh/rust" "$out"
cp "$root/tools/ci/package.Cargo.lock" "$project/.nsh/rust/Cargo.lock"
export PATINA_AIRPLAY_PROBE=1

# NSH builds and executes a macOS descriptor even when packaging an iPad app.
# Set its dyld path here, after macOS has launched the script interpreter.
runtime="$root/target/mpv-runtime-macos"
fetch=(--platform macos --destination "$runtime")
if [[ -n "${PATINA_MPV_RUNTIME_MANIFEST:-}" ]]; then
  fetch+=(--manifest "$PATINA_MPV_RUNTIME_MANIFEST")
elif [[ -z "${PATINA_MPV_RUNTIME_ARCHIVE:-}" ]]; then
  PATINA_MPV_RUNTIME_ARCHIVE=$(python3 tools/ci/native_candidates.py macos)
fi
if [[ -n "${PATINA_MPV_RUNTIME_ARCHIVE:-}" ]]; then
  fetch+=(--archive "$PATINA_MPV_RUNTIME_ARCHIVE")
fi
python3 tools/mpv-runtime-unix/runtime.py fetch "${fetch[@]}"
export PKG_CONFIG_PATH="$runtime/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
export DYLD_FALLBACK_LIBRARY_PATH="$runtime/lib${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"

if [[ "$platform" != macos ]]; then
  triple=aarch64-apple-ios
  [[ "$platform" != ios-simulator ]] || triple+=-sim
  rustup target add "$triple"
  bash tools/fetch-mpvkit.sh
  export IPTV_RS_MPVKIT="$root/target/mpvkit"
fi

"$nsh" package "$project" --target "$platform" -o "$out"
bundle="$out/Patina AirPlay Probe.app"
if [[ "$platform" == macos ]]; then
  python3 -I tools/bundle-macos.py --runtime "$runtime" "$bundle"
  codesign --verify --deep --strict "$bundle"
fi
test -d "$bundle"
echo "$bundle"
