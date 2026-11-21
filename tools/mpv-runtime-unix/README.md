# Linux and macOS native runtimes

This directory builds the libmpv that Patina's AppImage (Linux x86_64) and
macOS disk image (Apple silicon) bundle, from the same pinned sources as the
Windows runtime in [`../mpv-runtime`](../mpv-runtime/README.md). It replaces
Ubuntu's and Homebrew's libmpv/FFmpeg, whose versions, licenses and minimum
OS changed with whatever the build machine had installed.

Nothing here is an input of the Windows runtime: the Windows recipe digest,
source bundle and DLL are unchanged. This directory reuses the Windows
pipeline's `prepare.py` (verified downloads, safe extraction, license checks,
offline restore) and `smoke.py` (synthetic MPEG-TS/HLS playback).

| File | Purpose |
| --- | --- |
| `sources.json` | Which components of `../mpv-runtime/sources.lock.json` each platform uses, plus the platform's own SHA-256-pinned sources. Shared entries are copied verbatim, so a version changes in one place. |
| `runtime.py` | `lock` (resolve the platform lock), `verify` (feature and license checks of the configure headers), `package` (validate the build evidence, write the archives and manifest), `fetch` (download, verify and unpack a pinned runtime for app packaging). |
| `linux/Dockerfile`, `linux/build.sh` | The Linux build: the Windows builder's pinned Ubuntu 24.04 image and package snapshot, plus the headers of the host libraries libmpv links dynamically. Compiles with networking disabled. |
| `macos/build.sh`, `macos/xcode-version` | The macOS build on an Apple silicon runner with the pinned Xcode; no Docker, so the script isolates itself (see below). |
| `smoke.py` | Plays generated MPEG-TS and local HLS through the exact library. |
| `runtime-linux.json`, `runtime-macos.json` | The pinned dependency releases the app packaging fetches. Matched to successful native candidates in `../ci/native-candidates.json`; `../ci/native_candidates.py` verifies a candidate before the dependency release exists. |

## What the runtimes contain

Each runtime is one shared library (`libmpv.so.2` or `libmpv.2.dylib`) with
FFmpeg 8.1, libass, libplacebo, FreeType, HarfBuzz, FriBidi, dav1d and zlib
linked in statically. Only the `mpv_*` client API is exported, so the GTK
stack's own HarfBuzz/FreeType, or a system FFmpeg, never mixes with these
copies. LGPL only: FFmpeg is configured with `--disable-gpl
--disable-version3 --disable-nonfree` and mpv with `-Dgpl=false`;
`runtime.py verify` fails the build unless the generated headers say exactly
that (`CONFIG_GPL 0`, `CONFIG_VERSION3 0`, `CONFIG_NONFREE 0`, `HAVE_GPL 0`).

| | Linux x86_64 | macOS arm64 |
| --- | --- | --- |
| Video | OpenGL render API without a windowing backend (`plain-gl`), as `tv.player` draws into a GTK GL area | same, into a `CAOpenGLLayer` |
| Hardware decoding (`hwdec=auto-copy-safe`) | VAAPI (libva, host drivers) and NVDEC (NVIDIA driver loaded at run time) | VideoToolbox |
| Audio | PipeWire, PulseAudio, ALSA | Core Audio |
| HTTPS | OpenSSL 3 (host) | Secure Transport |
| Subtitle fonts | fontconfig (host) | Core Text |
| Host libraries | glibc, libstdc++, libpipewire, libpulse, libasound, libva, libva-drm, libssl, libcrypto, libfontconfig; `runtime.py` refuses any other `NEEDED` entry | `/usr/lib` and system frameworks only; `runtime.py` refuses any other load command |
| Minimum OS | glibc 2.39 (Ubuntu 24.04), the same as the app's Linux packages; checked against `objdump -T` | macOS 14.0, the app's declared minimum; checked against `LC_BUILD_VERSION` |

The app's AppImage deploys the Linux host libraries from Ubuntu 24.04 as
before; their notices and sources still come from
`tools/ci/linux-native-notices.py` and `tools/ci/native-sources.py`.

## Build locally (Linux)

From the repository root, with Docker:

```sh
docker build -t patina-mpv-builder-linux tools/mpv-runtime-unix/linux
mkdir -p target/mpv-runtime-linux-work target/mpv-runtime-linux-build target/mpv-runtime-linux-dist
docker run --rm -v "$PWD:/repo:ro" -v "$PWD/target/mpv-runtime-linux-work:/work" \
  patina-mpv-builder-linux sh -c '
    python3 /repo/tools/mpv-runtime-unix/runtime.py lock --platform linux --output /work/lock.json &&
    python3 /repo/tools/mpv-runtime/prepare.py prepare --lock /work/lock.json --work /work'
docker run --rm --network=none -v "$PWD:/repo:ro" \
  -v "$PWD/target/mpv-runtime-linux-work:/work" -v "$PWD/target/mpv-runtime-linux-build:/output" \
  patina-mpv-builder-linux bash /repo/tools/mpv-runtime-unix/linux/build.sh /work /output
docker run --rm --network=none -v "$PWD:/repo:ro" \
  -v "$PWD/target/mpv-runtime-linux-work:/work" -v "$PWD/target/mpv-runtime-linux-build:/build-output:ro" \
  -v "$PWD/target/mpv-runtime-linux-dist:/dist" patina-mpv-builder-linux \
  python3 /repo/tools/mpv-runtime-unix/runtime.py package --platform linux --lock /work/lock.json \
  --work /work --build /build-output --output /dist --repository OWNER/REPOSITORY
```

Add `--user "$(id -u):$(id -g)"` to keep the files owned by you, and `-e JOBS=N`
to limit parallel compilation. Then smoke the exact library on the host:

```sh
mkdir -p target/mpv-runtime-linux-tools
tar -xzf target/mpv-runtime-linux-dist/test-tools-linux-x86_64.tar.gz -C target/mpv-runtime-linux-tools
python3 tools/mpv-runtime-unix/smoke.py --library target/mpv-runtime-linux-build/libmpv.so.2 \
  --ffmpeg target/mpv-runtime-linux-tools/bin/ffmpeg
```

and run the app's real-player tests against it (they draw through the OpenGL
render API into a surfaceless EGL context):

```sh
python3 tools/mpv-runtime-unix/runtime.py fetch --platform linux \
  --manifest target/mpv-runtime-linux-dist/runtime-linux.json \
  --archive target/mpv-runtime-linux-dist/mpv-runtime-linux-x86_64.tar.gz \
  --destination target/mpv-runtime-linux
PATH="$PWD/target/mpv-runtime-linux-tools/bin:$PATH" \
LIBRARY_PATH="$PWD/target/mpv-runtime-linux/lib" LD_LIBRARY_PATH="$PWD/target/mpv-runtime-linux/lib" \
IPTV_REQUIRE_PLAYER_TESTS=1 cargo test -p iptv-player-mpv --lib
```

## Build on macOS

On an Apple silicon Mac with the Xcode version in `macos/xcode-version`
selected:

```sh
python3 tools/mpv-runtime-unix/runtime.py lock --platform macos --output target/mpv-runtime-macos-work/lock.json
python3 tools/mpv-runtime/prepare.py prepare --lock target/mpv-runtime-macos-work/lock.json \
  --work target/mpv-runtime-macos-work
bash tools/mpv-runtime-unix/macos/build.sh target/mpv-runtime-macos-work target/mpv-runtime-macos-build
python3 tools/mpv-runtime-unix/runtime.py package --platform macos \
  --lock target/mpv-runtime-macos-work/lock.json --work target/mpv-runtime-macos-work \
  --build target/mpv-runtime-macos-build --output target/mpv-runtime-macos-dist --repository OWNER/REPOSITORY
```

There is no container, so `macos/build.sh` isolates the build itself: it
re-executes with an empty environment under `sandbox-exec` with outbound
network access denied, puts only `/usr/bin`, `/bin`, `/usr/sbin`, `/sbin` and
the tools it builds from pinned sources (ninja, meson, pkgconf) on `PATH`,
restricts pkg-config to its private prefix, and refuses to run under another
Xcode. Homebrew may be installed on the machine; nothing can find it. The
package step then rejects any load command outside `/usr/lib` and
`/System/Library/Frameworks`, any `LC_RPATH`, any export beyond `_mpv_*` and
a minimum macOS above 14.0.

The macOS pipeline has not run yet: it was written without access to a Mac
and is verified only by the unit tests and its first CI run.

## Publish and update

Dispatch **Build retained libmpv dependencies** (`mpv-runtime.yml`) with the
platform. It builds, packages, smoke-tests the exact archive on that platform,
and only then publishes the dependency release `mpv-runtime-<platform>-<digest>`
with the runtime, `mpv-runtime-<platform>-sources.tar.gz` (the exact
corresponding sources, licenses and build scripts, restorable offline with
`prepare.py prepare --source-bundle`), `runtime-<platform>.json` and
`SHA256SUMS`. The digest covers the platform lock, this directory's scripts
for that platform and the shared `prepare.py`/`smoke.py`. Existing releases are
never overwritten.

Review the release, then commit its `runtime-<platform>.json` here. App
packaging (`tools/ci/package-linux.sh`, `tools/package-macos.sh`) fetches the
runtime with `runtime.py fetch`, which checks the archive and the library
against that manifest and fails on any mismatch.

Never prune, delete or replace a dependency release or its source assets:
every distributed AppImage and disk image names its runtime's source archive
in `notices/mpv-runtime/SOURCE.txt`. Fix a bad release by changing the inputs
and publishing a new digest.

The relay guard update uses immutable Actions candidates for pre-merge checks.
`tools/ci/promote-native-candidates.py` verifies the successful platform workflow,
recipe digest, artifact ownership, committed manifest and all release checksums.
Its dry run publishes nothing. `.github/workflows/promote-native.yml` publishes
those exact assets only on a trusted master push, after merge, and refuses to
overwrite an existing dependency release. The same guarded Linux library ships
in a private directory in DEB, RPM and pacman packages as well as the AppImage.
