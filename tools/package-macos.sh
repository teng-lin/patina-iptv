#!/usr/bin/env bash
# The macOS app, self-contained: NSH's release runner as an ad hoc signed
# .app (nsh package --target macos), linked against Patina's pinned libmpv
# runtime (tools/mpv-runtime-unix: LGPL-only FFmpeg linked into one
# libmpv.2.dylib, macOS 14 or newer), which tools/bundle-macos.py copies into
# Contents/Frameworks and signs again. Runs on any Apple silicon Mac with
# that macOS; not notarized (tools/ci/package-macos.sh does that when
# signing credentials exist).
# PATINA_MPV_RUNTIME_MANIFEST and PATINA_MPV_RUNTIME_ARCHIVE substitute a
# locally built runtime candidate, still verified by hash.
# Usage: tools/package-macos.sh [output directory, default target/package]
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
source "$root/tools/ci/locked-build-env.sh"
nsh=${NSH:-$root/../nsh-patina/target/release/nsh}
out=$(mkdir -p "${1:-$root/target/package}" && cd "${1:-$root/target/package}" && pwd)
runtime="$root/target/mpv-runtime-macos"
fetch=(--platform macos --destination "$runtime")
[ -z "${PATINA_MPV_RUNTIME_MANIFEST:-}" ] || fetch+=(--manifest "$PATINA_MPV_RUNTIME_MANIFEST")
if [ -z "${PATINA_MPV_RUNTIME_ARCHIVE:-}" ] && [ -z "${PATINA_MPV_RUNTIME_MANIFEST:-}" ]; then
  PATINA_MPV_RUNTIME_ARCHIVE=$(python3 "$root/tools/ci/native_candidates.py" macos)
fi
[ -z "${PATINA_MPV_RUNTIME_ARCHIVE:-}" ] || fetch+=(--archive "$PATINA_MPV_RUNTIME_ARCHIVE")
python3 "$root/tools/mpv-runtime-unix/runtime.py" fetch "${fetch[@]}"
# The app's build script asks pkg-config where libmpv is: the runtime first,
# ahead of any Homebrew mpv on the machine.
export PKG_CONFIG_PATH="$runtime/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
test "$(cd "$(pkg-config --variable=libdir mpv)" && pwd -P)" = "$(cd "$runtime/lib" && pwd -P)"
rm -rf "$out/Patina.app"
# The runtime's libmpv is @rpath/libmpv.2.dylib. NSH builds and runs a helper
# that links it while packaging, and that helper has no rpath; let dyld find
# the runtime there. (Set here, not inherited: macOS drops DYLD_* variables
# when it starts a protected program such as bash. The bundled app gets its
# own rpath, @executable_path/../Frameworks, from tools/bundle-macos.py.)
export DYLD_FALLBACK_LIBRARY_PATH="$runtime/lib${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"
(cd "$root/crates/iptv-app" && PATINA_UPDATE_PACKAGE=dmg "$nsh" package --target macos -o "$out" .)
unset DYLD_FALLBACK_LIBRARY_PATH
bash "$root/tools/ci/notices.sh"
cp "$root/LICENSE" "$out/Patina.app/Contents/Resources/LICENSE"
cp "$root/crates/iptv-app/fonts/OFL.txt" "$out/Patina.app/Contents/Resources/OFL.txt"
cp -R "$root/target/notices" "$out/Patina.app/Contents/Resources/notices"
python3 -I "$root/tools/bundle-macos.py" --runtime "$runtime" "$out/Patina.app"
python3 "$root/tools/ci/check-notices.py" "$out/Patina.app" mpv-runtime/components.json
# Homebrew libraries, if the executable still links any, keep their notices
# and source records (tools/ci/native-sources.py).
if [ -d "$out/Patina.app/Contents/Resources/notices/native" ]; then
  python3 "$root/tools/ci/check-notices.py" "$out/Patina.app" native/components.json
  python3 "$root/tools/ci/check-notices.py" "$out/Patina.app" native/sources.json
fi
codesign --verify --deep --strict "$out/Patina.app"
echo "$out/Patina.app"
