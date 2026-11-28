# Windows native runtime

This directory builds Patina's x64 libmpv dependency from pinned inputs. App PRs
download the runtime selected by `runtime.json`. Native compilation runs only
when explicitly requested; app releases produce an `.exe` installer, not a
portable app ZIP.

`sources.lock.json` records exact source and toolchain URLs, SHA-256 hashes and
explicit license paths. `prepare.py` verifies downloads, safely extracts them and
checks every listed license. `Dockerfile` pins the Linux base image and host
package snapshot. Its checksum-verified CA bootstrap package comes from that
same snapshot, so bootstrap does not depend on a moving apt repository.
`build.sh` compiles with network access disabled and disables
automatic dependency fallbacks. Its configure options define the enabled feature
and license choices; consult the shipped component notices rather than assuming
the license of a different mpv distribution applies.

## Build locally

Run from the repository root on Linux with Docker installed:

```sh
docker build -t patina-mpv-builder tools/mpv-runtime
mkdir -p target/mpv-runtime-work target/mpv-runtime-build target/mpv-runtime-dist
docker run --rm -v "$PWD:/repo:ro" -v "$PWD/target/mpv-runtime-work:/work" \
  patina-mpv-builder python3 /repo/tools/mpv-runtime/prepare.py prepare \
  --lock /repo/tools/mpv-runtime/sources.lock.json --work /work
docker run --rm --network=none -v "$PWD:/repo:ro" \
  -v "$PWD/target/mpv-runtime-work:/work" -v "$PWD/target/mpv-runtime-build:/output" \
  patina-mpv-builder bash /repo/tools/mpv-runtime/build.sh /work /output
docker run --rm --network=none -v "$PWD:/repo:ro" \
  -v "$PWD/target/mpv-runtime-work:/work" -v "$PWD/target/mpv-runtime-build:/build-output:ro" \
  -v "$PWD/target/mpv-runtime-dist:/dist" patina-mpv-builder \
  python3 /repo/tools/mpv-runtime/package.py \
  --lock /repo/tools/mpv-runtime/sources.lock.json --work /work \
  --build /build-output --output /dist --repository OWNER/REPOSITORY
```

Replace `OWNER/REPOSITORY` with the repository that will retain the dependency
release. Use fresh work/build directories for changed inputs; cached download
archives can be reused because their hashes are checked. The package command
revalidates the prepared inputs and invokes the source bundler. To create just
the corresponding-source archive, run:

```sh
docker run --rm --network=none -v "$PWD:/repo:ro" \
  -v "$PWD/target/mpv-runtime-work:/work" -v "$PWD/target/mpv-runtime-dist:/dist" \
  patina-mpv-builder python3 /repo/tools/mpv-runtime/prepare.py bundle \
  --lock /repo/tools/mpv-runtime/sources.lock.json --work /work \
  --output /dist/mpv-runtime-sources.tar.gz
```

The resulting runtime ZIP contains `libmpv-2.dll`, licenses, component inventory,
source lock, build configuration and `SOURCE.txt`. The separate `test-tools.zip`
contains `ffmpeg.exe`; never copy it into app packages. The source archive retains
the exact downloaded archives, licenses, lock and build scripts, including the
toolchain inputs. Pinned inputs and offline builds make the process auditable;
byte-for-byte reproducibility requires comparison of independent clean builds.

To recover inputs when an upstream download is unavailable, obtain the retained
`mpv-runtime-sources.tar.gz` from the dependency release and verify its checksum
against `runtime.json`. Restore into a fresh work directory with the matching
source lock:

```sh
python3 tools/mpv-runtime/prepare.py prepare \
  --lock tools/mpv-runtime/sources.lock.json --work target/mpv-runtime-restored \
  --source-bundle path/to/mpv-runtime-sources.tar.gz
```

This mode checks the retained lock and every input archive checksum, restores the
cache, and prepares sources without network access. Missing or modified inputs
fail instead of falling back to upstream URLs. Continue with the build command
above using this restored work directory and the matching retained build scripts.

## Validate and publish

On Windows, extract the runtime and test-tools ZIPs into separate directories and
run the exact candidate DLL:

```powershell
python tools/mpv-runtime/smoke.py --dll path/to/runtime/libmpv-2.dll --ffmpeg path/to/test-tools/ffmpeg.exe
```

The smoke test generates its own video/audio, serves MPEG-TS and HLS on loopback,
and checks advancing playback, decoded audio/video tracks and distinct nonblack
software-rendered frames. It reads no user profile or real provider media.

Dispatch **Build retained libmpv dependencies** (`mpv-runtime.yml`) with platform
`windows` to run the full pipeline. The workflow publishes only from master, with
`publish` enabled, after its Windows smoke job (playback and the
[relay guard](patches/README.md)) succeeds. Its tag is `mpv-runtime-<digest>`; the
digest covers the source lock, Dockerfile, patches and build/helper scripts.
Existing releases are never overwritten.
The retained release contains:

- `mpv-runtime-windows-x86_64.zip`
- `mpv-runtime-sources.tar.gz`
- `runtime.json`
- `SHA256SUMS`

To update the app's dependency, review the source/license changes, run the
dependency workflow, verify its published checksums and commit the published
`runtime.json` to this directory. Then run the normal Windows build and installer
checks. The fetch script verifies the archive and DLL; installer generation also
requires the DLL and native inventory to match the committed manifest.

Never prune, delete or replace a retained dependency release or its source assets.
Old installers continue to reference their original source URL after the app
updates to a newer dependency. Redistributed installers must include the notices
and preserve access to those same corresponding sources. Fix a bad release by
changing the inputs and publishing a new digest, not by overwriting its assets.
