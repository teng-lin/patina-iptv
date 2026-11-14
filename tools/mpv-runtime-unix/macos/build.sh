#!/usr/bin/env bash
# Build the macOS arm64 libmpv runtime only from prepare.py's verified inputs.
# Run on an Apple silicon Mac with the Xcode named in macos/xcode-version
# selected (`sudo xcode-select -s /Applications/Xcode_<version>.app`).
#
# There is no container on macOS, so isolation is explicit: PATH holds only
# the system and Xcode tools plus meson, ninja and pkgconf built here from the
# pinned sources; pkg-config sees only the private prefix; Homebrew and every
# compiler/linker search variable are removed from the environment; and the
# compilation runs under a sandbox profile that refuses network connections.
# The result is one libmpv.2.dylib with FFmpeg, libass, libplacebo, FreeType,
# HarfBuzz, FriBidi, dav1d and zlib linked in statically. It exports only the
# mpv_* API and references only /usr/lib and system frameworks.
set -euo pipefail
if [[ $# != 2 ]]; then
    echo "usage: $0 WORK OUTPUT" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")/.." && pwd)
if [[ -z "${PATINA_RUNTIME_SANDBOXED:-}" ]]; then
    # Unix sockets and loopback (the smoke test's local server) stay usable;
    # every other outbound connection is refused.
    exec env -i HOME="$HOME" TMPDIR="${TMPDIR:-/tmp}" JOBS="${JOBS:-}" \
        DEVELOPER_DIR="${DEVELOPER_DIR:-}" PATINA_RUNTIME_SANDBOXED=1 \
        PATH=/usr/bin:/bin:/usr/sbin:/sbin \
        /usr/bin/sandbox-exec -p '(version 1) (allow default) (deny network-outbound (remote ip)) (allow network-outbound (remote ip "localhost:*"))' \
        /bin/bash "$0" "$@"
fi
[[ -n "$DEVELOPER_DIR" ]] || unset DEVELOPER_DIR
[[ -n "$JOBS" ]] || unset JOBS
work=$(cd "$1" && pwd -P)
mkdir -p "$2"
output=$(cd "$2" && pwd -P)
sources="$work/sources"
prefix="$work/prefix"
tools="$work/tools"
build="$work/build"
jobs=${JOBS:-$(sysctl -n hw.ncpu)}
mkdir -p "$prefix" "$tools/bin" "$build" "$output/logs" "$output/include/mpv"
exec > >(tee "$output/logs/build.log") 2>&1
set -x

xcode=$(tr -d '[:space:]' < "$here/macos/xcode-version")
xcodebuild -version | tee "$output/logs/xcode-version.txt"
if [[ "$(xcodebuild -version | sed -n 1p)" != "Xcode $xcode" ]]; then
    echo "expected Xcode $xcode (macos/xcode-version); select it with xcode-select" >&2
    exit 1
fi
if [[ "$(uname -m)" != arm64 ]]; then
    echo "the macOS runtime is built natively on Apple silicon" >&2
    exit 1
fi
sdk=$(xcrun --sdk macosx --show-sdk-path)
xcrun --sdk macosx --show-sdk-version | tee "$output/logs/sdk-version.txt"
sw_vers | tee "$output/logs/host-version.txt"
export MACOSX_DEPLOYMENT_TARGET=14.0
CC=$(xcrun --sdk macosx -f clang)
CXX=$(xcrun --sdk macosx -f clang++)
export CC CXX AR=/usr/bin/ar RANLIB=/usr/bin/ranlib NM=/usr/bin/nm STRIP=/usr/bin/strip
flags="-arch arm64 -isysroot $sdk -mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET"
export CFLAGS="-O2 $flags" CXXFLAGS="-O2 $flags" OBJCFLAGS="-O2 $flags" LDFLAGS="$flags"
"$CC" --version
/usr/bin/python3 --version
export PATH="$tools/bin:/usr/bin:/bin:/usr/sbin:/sbin"
case ":$PATH:" in *homebrew*|*/usr/local/*) echo "Homebrew on PATH" >&2; exit 1 ;; esac

# Build tools from the pinned sources: ninja, meson (run from its source tree)
# and pkgconf.
(
    cd "$sources/ninja"
    /usr/bin/python3 configure.py --bootstrap
    install -m 755 ninja "$tools/bin/ninja"
)
printf '#!/bin/sh\nexec /usr/bin/python3 "%s/meson/meson.py" "$@"\n' "$sources" > "$tools/bin/meson"
chmod +x "$tools/bin/meson"
meson --version
meson setup "$build/pkgconf" "$sources/pkgconf" --prefix "$tools" --buildtype release \
    --wrap-mode=nofallback -Ddefault_library=static -Dtests=disabled
meson compile -C "$build/pkgconf" -j "$jobs"
meson install -C "$build/pkgconf" --no-rebuild
# Only the private prefix: nothing from Homebrew or the host can be found.
export PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig:$prefix/share/pkgconfig"
export PKG_CONFIG_PATH=
printf '#!/bin/sh\nexec "%s/bin/pkgconf" --static "$@"\n' "$tools" > "$tools/bin/pkg-config"
chmod +x "$tools/bin/pkg-config"
export PKG_CONFIG="$tools/bin/pkg-config"
pkg-config --version

verify() {
    /usr/bin/python3 "$here/runtime.py" verify --platform macos --component "$@"
}

meson_build() {
    local name=$1
    shift
    meson setup "$build/$name" "$sources/$name" \
        --prefix "$prefix" --libdir lib --buildtype release \
        --wrap-mode=nofallback -Ddefault_library=static "$@"
    meson compile -C "$build/$name" -j "$jobs"
    meson install -C "$build/$name" --no-rebuild
    cp "$build/$name/meson-logs/meson-log.txt" "$output/logs/$name-meson.log"
    meson introspect "$build/$name" --buildoptions > "$output/logs/$name-options.json"
}

mkdir -p "$build/zlib"
(
    cd "$build/zlib"
    "$sources/zlib/configure" --static --prefix="$prefix"
    make -j "$jobs"
    make install
    cp configure.log "$output/logs/zlib-configure.log"
)
meson_build freetype -Dauto_features=disabled -Dharfbuzz=disabled -Dzlib=system
meson_build harfbuzz -Dauto_features=disabled -Dtests=disabled \
    -Dutilities=disabled -Dfreetype=enabled
meson_build fribidi -Ddocs=false -Dbin=false -Dtests=false
# Core Text finds the system's fonts for text subtitles.
meson_build libass -Dauto_features=disabled -Dcoretext=enabled -Dfontconfig=disabled \
    -Drequire-system-font-provider=true -Dasm=enabled
meson_build dav1d -Dauto_features=disabled -Denable_tools=false \
    -Denable_examples=false -Denable_tests=false -Denable_docs=false
for name in fast_float jinja markupsafe Vulkan-Headers; do
    mkdir -p "$sources/libplacebo/3rdparty/$name"
    cp -a "$sources/$name/." "$sources/libplacebo/3rdparty/$name/"
done
meson_build libplacebo -Dauto_features=disabled -Ddemos=false -Dtests=false

mkdir -p "$build/ffmpeg"
(
    cd "$build/ffmpeg"
    "$sources/ffmpeg/configure" \
        --prefix="$prefix" --libdir="$prefix/lib" \
        --cc="$CC" --cxx="$CXX" --ar="$AR" --ranlib="$RANLIB" --nm="$NM" --strip="$STRIP" \
        --host-cc="$CC" --host-cflags="$flags" --host-ldflags="$flags" \
        --arch=arm64 --target-os=darwin \
        --pkg-config="$PKG_CONFIG" --pkg-config-flags=--static \
        --extra-cflags="$flags -I$prefix/include" --extra-ldflags="$flags -L$prefix/lib" \
        --disable-autodetect --disable-gpl --disable-version3 --disable-nonfree \
        --disable-shared --enable-static --enable-pic \
        --enable-libdav1d --enable-zlib --enable-pthreads \
        --enable-securetransport --enable-videotoolbox \
        --disable-doc --disable-debug --disable-ffplay --disable-ffprobe
    cp ffbuild/config.log "$output/logs/ffmpeg-configure.log"
    cp config.h "$output/logs/ffmpeg-config.h"
    cp config_components.h "$output/logs/ffmpeg-config_components.h"
    cp ffbuild/config.mak "$output/logs/ffmpeg-config.mak"
    verify ffmpeg "$output/logs/ffmpeg-config.h" "$output/logs/ffmpeg-config_components.h"
    make -j "$jobs"
    make install
)

# Export only the client API; the static libraries stay private to libmpv.
# libplacebo's C++ parts need libc++, which its .pc does not name. mpv turns
# off Meson's undefined-symbol check (dynamic_lookup on macOS); make a
# missing library a link error instead of a load-time failure.
# No build timestamp, so that independent builds can be compared byte for byte.
# mpv's CoreAudio output calls cfstr_from_cstr, which osdep/utils-mac.c
# defines; mpv builds that file only with the cocoa feature, and cocoa needs
# Swift. Build it with the CoreAudio sources instead (this working copy
# only; the published source archive is the pinned upstream one, and this
# script is the change).
/usr/bin/python3 - "$sources/mpv/meson.build" <<'PATCH'
import sys
path = sys.argv[1]
text = open(path).read()
old = "'audio/out/ao_coreaudio_utils.c')"
new = "'audio/out/ao_coreaudio_utils.c')\n    if not features['cocoa']\n        sources += files('osdep/utils-mac.c')\n    endif"
if text.count(old) != 1:
    sys.exit("mpv's meson.build changed: cannot add osdep/utils-mac.c")
open(path, "w").write(text.replace(old, new))
PATCH
meson_build mpv -Ddefault_library=shared -Dauto_features=disabled \
    "-Dc_link_args=['-Wl,-exported_symbol,_mpv_*', '-Wl,-dead_strip_dylibs', '-lc++', '-Wl,-undefined,error']" \
    -Dgpl=false -Dcplayer=false -Dlibmpv=true -Dbuild-date=false -Dgl=enabled -Dplain-gl=enabled \
    -Dcoreaudio=enabled -Dcocoa=disabled -Dswift-build=disabled -Diconv=enabled \
    -Dlibavdevice=enabled -Dzlib=enabled -Dlua=disabled -Djavascript=disabled -Dlibcurl=disabled
cp "$build/mpv/config.h" "$output/logs/mpv-config.h"
verify mpv "$output/logs/mpv-config.h"

library="$output/libmpv.2.dylib"
install -m 755 "$(cd "$prefix/lib" && pwd -P)/$(readlink "$prefix/lib/libmpv.2.dylib" || echo libmpv.2.dylib)" "$library"
install_name_tool -id @rpath/libmpv.2.dylib "$library"
strip -x "$library"
for header in client.h render.h render_gl.h stream_cb.h; do
    install -m 644 "$prefix/include/mpv/$header" "$output/include/mpv/$header"
done
install -m 755 "$prefix/bin/ffmpeg" "$output/ffmpeg"
# mpv links with `-undefined dynamic_lookup`, so a symbol nothing defines
# links fine and fails only when the library is loaded. Refuse it here.
if nm -m -u "$library" | grep "dynamically looked up"; then
    echo "libmpv leaves the symbols above to be found at load time" >&2
    exit 1
fi
strip "$output/ffmpeg"
# Editing load commands voids the linker's signature; arm64 needs a valid one.
codesign --force --sign - "$library"
codesign --force --sign - "$output/ffmpeg"

for binary in libmpv.2.dylib ffmpeg; do
    otool -L "$output/$binary" > "$output/logs/$binary-otool-libraries.txt"
    otool -l "$output/$binary" > "$output/logs/$binary-otool-loads.txt"
done
nm -gU "$library" > "$output/logs/libmpv-exports.txt"
(cd "$output" && shasum -a 256 libmpv.2.dylib ffmpeg > logs/binaries.sha256)
# Load the exact library and play generated MPEG-TS and HLS over loopback.
/usr/bin/python3 "$here/smoke.py" --library "$library" --ffmpeg "$output/ffmpeg" \
    | tee "$output/logs/smoke.txt"
