#!/usr/bin/env bash
set -euo pipefail
: "${VERSION:?}"
root=$(cd "$(dirname "$0")/../.." && pwd)
source "$root/tools/ci/locked-build-env.sh"
cd "$root"
out="$root/dist"
stage="$root/target/linux-root"
appdir="$root/target/Patina.AppDir"
rm -rf "$stage" "$appdir" target/linux
mkdir -p "$out" target/linux
# The AppImage's libmpv (FFmpeg and the rest linked into it) is the pinned,
# LGPL-only runtime from tools/mpv-runtime-unix, verified by hash; never the
# build machine's. PATINA_MPV_RUNTIME_MANIFEST and PATINA_MPV_RUNTIME_ARCHIVE
# substitute a locally built candidate, which is verified the same way.
runtime="$root/target/mpv-runtime-linux"
fetch=(--platform linux --destination "$runtime")
[ -z "${PATINA_MPV_RUNTIME_MANIFEST:-}" ] || fetch+=(--manifest "$PATINA_MPV_RUNTIME_MANIFEST")
if [ -z "${PATINA_MPV_RUNTIME_ARCHIVE:-}" ] && [ -z "${PATINA_MPV_RUNTIME_MANIFEST:-}" ]; then
  PATINA_MPV_RUNTIME_ARCHIVE=$(python3 "$root/tools/ci/native_candidates.py" linux)
fi
[ -z "${PATINA_MPV_RUNTIME_ARCHIVE:-}" ] || fetch+=(--archive "$PATINA_MPV_RUNTIME_ARCHIVE")
python3 tools/mpv-runtime-unix/runtime.py fetch "${fetch[@]}"
# Link every package against the guarded runtime. Native packages retain it
# in a private directory so distribution libmpv cannot lose the IO guard.
export LIBRARY_PATH="$runtime/lib${LIBRARY_PATH:+:$LIBRARY_PATH}"
(cd crates/iptv-app && "$NSH" package --target linux -o "$root/target/linux" .)
bash "$root/tools/ci/notices.sh"
native="$root/target/linux/Patina"
cp -R target/notices "$native/notices"
cp LICENSE "$native/LICENSE"
cp crates/iptv-app/fonts/OFL.txt "$native/OFL.txt"
python3 tools/ci/check-notices.py "$native"
license=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["license"])')
mapfile -t executables < <(find "$native" -maxdepth 1 -type f -executable -printf '%f\n')
test "${#executables[@]}" -eq 1
exe=${executables[0]}
id=com.hantekllc.iptvrs
mkdir -p "$stage/opt/patina" "$stage/usr/bin" "$stage/usr/share/applications" "$stage/usr/share/icons/hicolor/1024x1024/apps"
cp -a "$native/." "$stage/opt/patina/"
mkdir -p "$stage/opt/patina/lib"
cp "$runtime/lib/libmpv.so.2" "$stage/opt/patina/lib/"
patchelf --set-rpath "\$ORIGIN/lib" "$stage/opt/patina/$exe"
python3 tools/mpv-runtime-unix/runtime.py notices --runtime "$runtime" \
  --output "$stage/opt/patina/notices/mpv-runtime"
printf '#!/bin/sh\nset -e\ncd /opt/patina\nexec "./%s" "$@"\n' "$exe" > "$stage/usr/bin/patina"
chmod +x "$stage/usr/bin/patina"
cp "$native/$id.png" "$stage/usr/share/icons/hicolor/1024x1024/apps/"
sed 's/^Exec=.*/Exec=patina/' "$native/$id.desktop" > "$stage/usr/share/applications/$id.desktop"
desktop-file-validate "$stage/usr/share/applications/$id.desktop"
# GLES is loaded dynamically by the renderer, so ELF dependency scanning does
# not add its dispatch library to native package requirements.
for format in deb rpm pacman; do
  case "$format" in
    deb) deps=(--depends 'libc6 (>= 2.39)' --depends 'libgtk-4-1 (>= 4.14)' --depends libdbus-1-3 --depends libfontconfig1 --depends libva-drm2 --depends libssl3t64 --depends libva2 --depends libasound2t64 --depends libpipewire-0.3-0t64 --depends libpulse0 --depends libstdc++6 --depends libgles2 --depends libgstreamer1.0-0 --depends libgstreamer-plugins-base1.0-0); extension=deb;;
    rpm) deps=(--depends 'glibc >= 2.39' --depends 'gtk4 >= 4.14' --depends dbus-libs --depends 'libfontconfig.so.1()(64bit)' --depends 'libva-drm.so.2()(64bit)' --depends 'libssl.so.3()(64bit)' --depends 'libva.so.2()(64bit)' --depends 'libasound.so.2()(64bit)' --depends 'libpipewire-0.3.so.0()(64bit)' --depends 'libpulse.so.0()(64bit)' --depends 'libstdc++.so.6()(64bit)' --depends libglvnd-gles --depends gstreamer1 --depends gstreamer1-plugins-base); extension=rpm;;
    pacman) deps=(--depends 'glibc>=2.39' --depends 'gtk4>=4.14' --depends dbus --depends fontconfig --depends libva --depends openssl --depends alsa-lib --depends libpipewire --depends libpulse --depends libstdc++ --depends libglvnd --depends gstreamer --depends gst-plugins-base-libs --pacman-compression zstd); extension=pkg.tar.zst;;
  esac
  printf '%s\n' "$format" > "$stage/opt/patina/patina-package"
  fpm --force -s dir -t "$format" -n patina -v "$VERSION" --license "$license" \
    --url https://github.com/teng-lin/patina-iptv \
    --description 'Patina IPTV player' --maintainer 'Patina maintainers' \
    "${deps[@]}" -C "$stage" -p "$out/Patina-$VERSION-linux-x86_64.$extension" .
done
mkdir -p "$appdir/usr/bin"
cp -a "$native/." "$appdir/usr/bin/"
printf 'appimage\n' > "$appdir/usr/bin/patina-package"
cp "$native/$id.desktop" "$appdir/"
cp "$native/$id.png" "$appdir/"
cat > "$appdir/AppRun" <<'RUN'
#!/usr/bin/env bash
set -e
export APPDIR="$(dirname "$(readlink -f "$0")")"
export LD_LIBRARY_PATH="$APPDIR/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
for hook in "$APPDIR"/apprun-hooks/*.sh; do
  [ ! -f "$hook" ] || source "$hook"
done
# Identify the mounted file by inode when a viewer later requests replacement.
if [[ -n ${APPIMAGE:-} && -f $APPIMAGE ]] && exec {image_fd}<"$APPIMAGE"; then
  export PATINA_APPIMAGE_FD=$image_fd
fi
cd "$APPDIR/usr/bin"
RUN
printf 'exec "./%s" "$@"\n' "$exe" >> "$appdir/AppRun"
chmod +x "$appdir/AppRun"
# GTK needs schemas, loaders and other resources beyond its linked libraries.
export DEPLOY_GTK_VERSION=4 OUTPUT="$out/Patina-$VERSION-linux-x86_64.AppImage"
# linuxdeploy excludes these common desktop libraries by default. They are
# direct/transitive dependencies here and may be absent on a minimal desktop.
# Native notice attribution assumes distro system-library provenance. Browser
# private libraries must not enter the deployment through caller environment.
unset LD_LIBRARY_PATH LD_PRELOAD
libraries=()
for library in libharfbuzz.so.0 libfribidi.so.0 libfontconfig.so.1 libfreetype.so.6 \
  libasound.so.2 libpipewire-0.3.so.0; do
  path=$(ldconfig -p | awk -v name="$library" '$1 == name { print $NF }' | head -n 1)
  test -n "$path"
  canonical=$(readlink -f "$path")
  case "$canonical" in
    /usr/lib/*|/lib/*) ;;
    *) echo "refusing non-system deployment library: $library" >&2; exit 1 ;;
  esac
  # Preserve the SONAME filename: linuxdeploy uses the supplied basename for
  # its destination. Passing the resolved versioned file drops the loader alias.
  libraries+=(--library "$path")
done
# linuxdeploy resolves dependencies with ldd: point the executable at the
# runtime so libmpv.so.2 comes from there (linuxdeploy then rewrites the
# RUNPATH to $ORIGIN/../lib), even when a system libmpv is installed.
patchelf --set-rpath "$runtime/lib" "$appdir/usr/bin/$exe"
linuxdeploy --appdir "$appdir" --executable "$appdir/usr/bin/$exe" "${libraries[@]}" --plugin gtk
for ((i=1; i<${#libraries[@]}; i+=2)); do
  test -f "$appdir/usr/lib/$(basename "${libraries[i]}")"
done
if readelf --dynamic "$appdir/usr/bin/$exe" | grep -F "$runtime"; then
  echo "the AppImage executable still searches the build machine's runtime" >&2
  exit 1
fi
python3 tools/mpv-runtime-unix/runtime.py check-bundle --platform linux \
  --runtime "$runtime" --libraries "$appdir/usr/lib"
python3 tools/mpv-runtime-unix/runtime.py notices --runtime "$runtime" \
  --output "$appdir/usr/bin/notices/mpv-runtime"
# License texts, the inventory, and the source package of every other
# bundled library (GTK and the host libraries libmpv uses: Ubuntu builds).
python3 tools/ci/linux-native-notices.py "$appdir" "$VERSION" "$runtime"
python3 tools/ci/check-notices.py "$appdir" mpv-runtime/components.json
python3 tools/ci/check-notices.py "$appdir" native/components.json
python3 tools/ci/check-notices.py "$appdir" native/sources.json
linuxdeploy --appdir "$appdir" --output appimage
