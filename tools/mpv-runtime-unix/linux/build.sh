#!/usr/bin/env bash
# Build the Linux x86_64 libmpv runtime only from prepare.py's verified inputs.
# Run in the pinned builder (linux/Dockerfile) with networking disabled.
#
# One shared library, libmpv.so.2, with FFmpeg, libass, libplacebo, FreeType,
# HarfBuzz, FriBidi, dav1d and zlib linked in statically and hidden
# (--exclude-libs): only the mpv_* API is exported, so the GTK stack's own
# HarfBuzz/FreeType and any system FFmpeg never mix with these copies. Host
# libraries stay dynamic: glibc, audio servers (PipeWire, PulseAudio, ALSA),
# VAAPI, OpenSSL and fontconfig. CUDA (nvdec) is loaded at run time only.
set -euo pipefail
if [[ $# != 2 ]]; then
    echo "usage: $0 WORK OUTPUT" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")/.." && pwd)
work=$(realpath "$1")
mkdir -p "$2"
output=$(realpath "$2")
sources="$work/sources"
prefix="$work/prefix"
build="$work/build"
jobs=${JOBS:-$(nproc)}
mkdir -p "$prefix" "$build" "$output/logs" "$output/include/mpv" "$work/bin"
exec > >(tee "$output/logs/build.log") 2>&1
set -x
export CC=gcc CXX=g++ AR=ar NM=nm RANLIB=ranlib STRIP=strip
# The pinned static libraries come first; the snapshot's system .pc files
# describe only the dynamic host libraries listed above.
export PKG_CONFIG_PATH="$prefix/lib/pkgconfig:$prefix/share/pkgconfig"
pkg_config=$(command -v pkg-config)
printf '#!/bin/sh\nexec "%s" --static "$@"\n' "$pkg_config" > "$work/bin/pkg-config-static"
chmod +x "$work/bin/pkg-config-static"
export PKG_CONFIG="$work/bin/pkg-config-static"
for command in "$CC" "$CXX" meson ninja make nasm python3 readelf objdump; do
    command -v "$command"
done
"$CC" --version
meson --version
python3 --version
ldd --version | sed -n 1p
dpkg-query -W > "$output/logs/host-packages.txt"

verify() {
    python3 "$here/runtime.py" verify --platform linux --component "$@"
}

meson_build() {
    local name=$1
    shift
    # Always configure afresh: a reconfigure can keep stale link arguments.
    rm -rf "${build:?}/$name"
    meson setup "$build/$name" "$sources/$name" \
        --prefix "$prefix" --libdir lib --buildtype release \
        --wrap-mode=nofallback -Ddefault_library=static -Db_staticpic=true "$@"
    meson compile -C "$build/$name" -j "$jobs"
    meson install -C "$build/$name" --no-rebuild
    cp "$build/$name/meson-logs/meson-log.txt" "$output/logs/$name-meson.log"
    meson introspect "$build/$name" --buildoptions > "$output/logs/$name-options.json"
}

mkdir -p "$build/zlib"
(
    cd "$build/zlib"
    CFLAGS="-O2 -fPIC" "$sources/zlib/configure" --static --prefix="$prefix"
    make -j "$jobs"
    make install
    cp configure.log "$output/logs/zlib-configure.log"
)
# "system" means the pinned zlib found through pkg-config, not a subproject.
meson_build freetype -Dauto_features=disabled -Dharfbuzz=disabled -Dzlib=system
meson_build harfbuzz -Dauto_features=disabled -Dtests=disabled \
    -Dutilities=disabled -Dfreetype=enabled
meson_build fribidi -Ddocs=false -Dbin=false -Dtests=false
# Fontconfig finds the host's fonts for text subtitles.
meson_build libass -Dauto_features=disabled -Dfontconfig=enabled \
    -Drequire-system-font-provider=true -Dasm=enabled
meson_build dav1d -Dauto_features=disabled -Denable_tools=false \
    -Denable_examples=false -Denable_tests=false -Denable_docs=false

# The commit archives omit submodules; supply the pinned ones (as on Windows).
for name in fast_float jinja markupsafe Vulkan-Headers; do
    mkdir -p "$sources/libplacebo/3rdparty/$name"
    cp -a "$sources/$name/." "$sources/libplacebo/3rdparty/$name/"
done
meson_build libplacebo -Dauto_features=disabled -Ddemos=false -Dtests=false

# Header-only: FFmpeg's nvdec loads the NVIDIA driver at run time.
make -C "$sources/nv-codec-headers" PREFIX="$prefix" install

mkdir -p "$build/ffmpeg"
(
    cd "$build/ffmpeg"
    "$sources/ffmpeg/configure" \
        --prefix="$prefix" --libdir="$prefix/lib" \
        --cc="$CC" --cxx="$CXX" --ar="$AR" --ranlib="$RANLIB" --nm="$NM" --strip="$STRIP" \
        --pkg-config="$PKG_CONFIG" --pkg-config-flags=--static \
        --extra-cflags="-I$prefix/include" --extra-ldflags="-L$prefix/lib" \
        --disable-autodetect --disable-gpl --disable-version3 --disable-nonfree \
        --disable-shared --enable-static --enable-pic \
        --enable-libdav1d --enable-zlib --enable-openssl --enable-pthreads \
        --enable-vaapi --enable-ffnvcodec --enable-nvdec \
        --disable-doc --disable-debug --disable-ffplay --disable-ffprobe
    cp ffbuild/config.log "$output/logs/ffmpeg-configure.log"
    cp config.h "$output/logs/ffmpeg-config.h"
    cp config_components.h "$output/logs/ffmpeg-config_components.h"
    cp ffbuild/config.mak "$output/logs/ffmpeg-config.mak"
    verify ffmpeg "$output/logs/ffmpeg-config.h" "$output/logs/ffmpeg-config_components.h"
    make -j "$jobs"
    make install
)

# --exclude-libs hides every symbol of the static archives above; --as-needed
# drops host libraries that pkg-config's --static lists but nothing calls.
# libplacebo's C++ parts need libstdc++, which its .pc does not name and mpv's
# C link would not add. mpv turns off Meson's undefined-symbol check; turn it
# back on, so a missing library fails here rather than at load time.
# No build timestamp, so that independent builds can be compared byte for byte.
meson_build mpv -Ddefault_library=shared -Dauto_features=disabled \
    "-Dc_link_args=['-Wl,--exclude-libs,ALL', '-Wl,--no-undefined', '-Wl,--as-needed', '-Wl,--push-state,--no-as-needed,-lstdc++,--pop-state']" \
    -Dgpl=false -Dcplayer=false -Dlibmpv=true -Dbuild-date=false -Dgl=enabled -Dplain-gl=enabled \
    -Dpipewire=enabled -Dpulse=enabled -Dalsa=enabled -Diconv=enabled \
    -Dlibavdevice=enabled -Dzlib=enabled -Dlua=disabled -Djavascript=disabled -Dlibcurl=disabled
cp "$build/mpv/config.h" "$output/logs/mpv-config.h"
verify mpv "$output/logs/mpv-config.h"

# The installed file is libmpv.so.2.x.y; ship it under its SONAME, stripped of
# symbol tables (the GNU build ID note stays and identifies the build).
install -m 755 "$(readlink -f "$prefix/lib/libmpv.so.2")" "$output/libmpv.so.2"
strip --strip-unneeded "$output/libmpv.so.2"
for header in client.h render.h render_gl.h stream_cb.h; do
    install -m 644 "$prefix/include/mpv/$header" "$output/include/mpv/$header"
done
install -m 755 "$prefix/bin/ffmpeg" "$output/ffmpeg"
strip "$output/ffmpeg"

readelf --dynamic "$output/libmpv.so.2" > "$output/logs/libmpv-dynamic.txt"
readelf --notes "$output/libmpv.so.2" > "$output/logs/libmpv-notes.txt"
nm --dynamic --defined-only "$output/libmpv.so.2" > "$output/logs/libmpv-exports.txt"
objdump --dynamic-syms "$output/libmpv.so.2" > "$output/logs/libmpv-symbols.txt"
readelf --dynamic "$output/ffmpeg" > "$output/logs/ffmpeg-dynamic.txt"
(cd "$output" && sha256sum libmpv.so.2 ffmpeg > logs/binaries.sha256)
# Load the exact library and play generated MPEG-TS and HLS over loopback.
python3 "$here/smoke.py" --library "$output/libmpv.so.2" --ffmpeg "$output/ffmpeg" \
    | tee "$output/logs/smoke.txt"
