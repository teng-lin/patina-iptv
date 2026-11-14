#!/usr/bin/env bash
# Build only from prepare.py's verified inputs. Run in the pinned Linux builder.
set -euo pipefail
if [[ $# != 2 ]]; then
    echo "usage: $0 WORK OUTPUT" >&2
    exit 2
fi
work=$(realpath "$1")
mkdir -p "$2"
output=$(realpath "$2")
sources="$work/sources"
prefix="$work/prefix"
build="$work/build"
toolchain="$sources/llvm-mingw"
jobs=${JOBS:-$(nproc)}
mkdir -p "$prefix" "$build" "$output/logs" "$work/bin"
exec > >(tee "$output/logs/build.log") 2>&1
set -x
# Pin native generators before putting the cross toolchain on PATH. Meson can
# need build-machine executables even though every installed library is Windows.
native_cc=$(command -v gcc)
native_cxx=$(command -v g++)
export PATH="$toolchain/bin:$PATH"
target=x86_64-w64-mingw32
export CC="$toolchain/bin/$target-clang"
export CXX="$toolchain/bin/$target-clang++"
export AR="$toolchain/bin/llvm-ar"
export RANLIB="$toolchain/bin/llvm-ranlib"
export NM="$toolchain/bin/llvm-nm"
export STRIP="$toolchain/bin/llvm-strip"
export PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig:$prefix/share/pkgconfig"
export PKG_CONFIG_PATH=
export PKG_CONFIG_SYSROOT_DIR=
# Do not let host-library detection or automatic Meson wraps alter the graph.
pkg_config=$(command -v pkg-config)
printf '#!/bin/sh\nexec "%s" --static "$@"\n' "$pkg_config" > "$work/bin/pkg-config-static"
chmod +x "$work/bin/pkg-config-static"
export PKG_CONFIG="$work/bin/pkg-config-static"
for command in "$CC" "$CXX" "$AR" meson ninja make nasm python3; do
    command -v "$command"
done
"$CC" --version
meson --version
python3 --version
dpkg-query -W > "$output/logs/host-packages.txt"

# Python avoids shell interpolation into Meson's quoted strings, including paths.
python3 - "$work" "$toolchain" "$native_cc" "$native_cxx" <<'PY'
import pathlib, sys
work, tc = map(pathlib.Path, sys.argv[1:3])
def quote(value):
    return "'" + str(value).replace('\\', '\\\\').replace("'", "\\'") + "'"
binaries = {'c': tc/'bin/x86_64-w64-mingw32-clang',
            'cpp': tc/'bin/x86_64-w64-mingw32-clang++',
            'ar': tc/'bin/llvm-ar', 'strip': tc/'bin/llvm-strip',
            'windres': tc/'bin/x86_64-w64-mingw32-windres',
            'pkg-config': work/'bin/pkg-config-static', 'nasm': '/usr/bin/nasm'}
text = '[binaries]\n' + ''.join(k+' = '+quote(v)+'\n' for k,v in binaries.items())
text += """
[host_machine]
system = 'windows'
cpu_family = 'x86_64'
cpu = 'x86_64'
endian = 'little'
[properties]
needs_exe_wrapper = true
[built-in options]
c_link_args = ['-static']
cpp_link_args = ['-static']
"""
(work/'cross.ini').write_text(text)
(work/'native.ini').write_text('[binaries]\nc = '+quote(sys.argv[3])+
                              '\ncpp = '+quote(sys.argv[4])+'\n')
PY
cp "$work/cross.ini" "$output/logs/cross.ini"
cp "$work/native.ini" "$output/logs/native.ini"

verify_features() {
    python3 - "$1" "$build/$1" "$output/logs" <<'PY'
import pathlib, re, sys
name, directory, logs = sys.argv[1], pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3])
headers = ['config.h', 'config_components.h'] if name == 'ffmpeg' else ['config.h']
defines = {}
for header in headers:
    text = (directory/header).read_text()
    (logs/(name+'-'+header)).write_text(text)
    defines.update(re.findall(r'^#define\s+(\w+)\s+([01])\s*$', text, re.MULTILINE))
if name == 'ffmpeg':
    enabled = '''SCHANNEL NETWORK W32THREADS ZLIB LIBDAV1D
        AVCODEC AVFORMAT AVFILTER AVUTIL SWRESAMPLE SWSCALE
        HLS_DEMUXER MPEGTS_DEMUXER MOV_DEMUXER MATROSKA_DEMUXER RTSP_DEMUXER
        HTTP_PROTOCOL HTTPS_PROTOCOL TCP_PROTOCOL TLS_PROTOCOL CRYPTO_PROTOCOL PATINA_RELAY_GUARD_V1_PROTOCOL
        H264_DECODER HEVC_DECODER MPEG2VIDEO_DECODER LIBDAV1D_DECODER
        AAC_DECODER MP2_DECODER AC3_DECODER EAC3_DECODER
        SUBRIP_DECODER WEBVTT_DECODER ASS_DECODER
        MPEG2VIDEO_ENCODER MP2_ENCODER RAWVIDEO_DEMUXER PCM_S16LE_DEMUXER
        HLS_MUXER MPEGTS_MUXER'''.split()
    disabled = ['GPL', 'VERSION3', 'NONFREE']
    prefix = 'CONFIG_'
else:
    enabled = ['WASAPI', 'WIN32_THREADS', 'LIBASS', 'LIBPLACEBO', 'FFMPEG', 'LIBAVDEVICE', 'ZLIB']
    disabled = ['GPL', 'LIBCURL']
    prefix = 'HAVE_'
errors = []
for expected, names in [('1', enabled), ('0', disabled)]:
    for feature in names:
        key = 'HAVE_W32THREADS' if name == 'ffmpeg' and feature == 'W32THREADS' else prefix+feature
        if defines.get(key) != expected:
            errors.append(f'{key}: expected {expected}, got {defines.get(key, "missing")}')
if errors:
    raise SystemExit(name+' required feature check failed:\n'+'\n'.join(errors))
print(name+': verified required playback features and LGPL build configuration')
PY
}

meson_build() {
    local name=$1
    shift
    local configure=()
    if [[ -f "$build/$name/meson-private/coredata.dat" ]]; then
        configure=(--reconfigure)
    fi
    meson setup "$build/$name" "$sources/$name" \
        "${configure[@]}" \
        --cross-file "$work/cross.ini" --native-file "$work/native.ini" \
        --prefix "$prefix" --libdir lib \
        --buildtype release --wrap-mode=nofallback -Ddefault_library=static "$@"
    if [[ "$name" == mpv ]]; then
        verify_features mpv
    fi
    meson compile -C "$build/$name" -j "$jobs"
    meson install -C "$build/$name" --no-rebuild
    cp "$build/$name/meson-logs/meson-log.txt" "$output/logs/$name-meson.log"
    meson introspect "$build/$name" --buildoptions > "$output/logs/$name-options.json"
}

# zlib's configure supports out-of-tree static builds and the MinGW target.
mkdir -p "$build/zlib"
(
    cd "$build/zlib"
    CHOST="$target" CFLAGS=-O2 "$sources/zlib/configure" --static --prefix="$prefix"
    make -j "$jobs"
    make install
    cp configure.log "$output/logs/zlib-configure.log"
)
# FreeType calls pkg-config's installed dependency "system"; "external"
# instead means its Meson subproject and would require an unpinned fallback.
meson_build freetype -Dauto_features=disabled -Dharfbuzz=disabled -Dzlib=system
meson_build harfbuzz -Dauto_features=disabled -Dtests=disabled \
    -Dutilities=disabled -Dfreetype=enabled
meson_build fribidi -Ddocs=false -Dbin=false -Dtests=false
meson_build libass -Dauto_features=disabled -Ddirectwrite=enabled -Dasm=enabled
meson_build dav1d -Dauto_features=disabled -Denable_tools=false \
    -Denable_examples=false -Denable_tests=false -Denable_docs=false

# The commit archives omit submodules. Supply the pinned preprocessing modules
# and floating-point conversion fallback, without invoking git or pip. Even
# with Vulkan disabled, libplacebo's public API stubs need the Vulkan types.
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
        --target-os=mingw32 --arch=x86_64 --enable-cross-compile \
        --host-cc="$native_cc" \
        --cc="$CC" --cxx="$CXX" --ar="$AR" --ranlib="$RANLIB" \
        --nm="$NM" --strip="$STRIP" --windres="$toolchain/bin/$target-windres" \
        --pkg-config="$PKG_CONFIG" --pkg-config-flags=--static \
        --extra-cflags="-I$prefix/include" --extra-ldflags="-L$prefix/lib -static" \
        --disable-autodetect --disable-gpl --disable-version3 --disable-nonfree \
        --disable-shared --enable-static --enable-libdav1d --enable-zlib \
        --enable-schannel --enable-w32threads --disable-pthreads \
        --disable-doc --disable-debug --disable-ffplay --disable-ffprobe
    verify_features ffmpeg
    # Keep built-in codecs, demuxers, protocols, filters and lavfi inputs. The
    # resulting ffmpeg.exe can generate fixtures without GPL encoder libraries.
    make -j "$jobs"
    make install
    cp ffbuild/config.log "$output/logs/ffmpeg-configure.log"
    cp config.h "$output/logs/ffmpeg-config.h"
    cp ffbuild/config.mak "$output/logs/ffmpeg-config.mak"
)
meson_build mpv -Ddefault_library=shared -Dauto_features=disabled \
    "-Dc_link_args=['-static', '-lc++']" \
    -Dgpl=false -Dcplayer=false -Dlibmpv=true -Dgl=disabled -Dwasapi=enabled \
    -Dwin32-threads=enabled -Dlibavdevice=enabled -Dzlib=enabled -Dlua=disabled -Dlibcurl=disabled

install -m 644 "$prefix/bin/libmpv-2.dll" "$output/libmpv-2.dll"
install -m 755 "$prefix/bin/ffmpeg.exe" "$output/ffmpeg.exe"
# Keep import dependencies visible for the Windows smoke/package checks.
"$toolchain/bin/llvm-readobj" --coff-imports "$output/libmpv-2.dll" \
    > "$output/logs/libmpv-imports.txt"
"$toolchain/bin/llvm-readobj" --coff-imports "$output/ffmpeg.exe" \
    > "$output/logs/ffmpeg-imports.txt"
sha256sum "$output/libmpv-2.dll" "$output/ffmpeg.exe" > "$output/logs/binaries.sha256"
