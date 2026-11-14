#!/usr/bin/env bash
# libmpv for iOS and iPadOS: MPVKit's LGPL build (libmpv, FFmpeg without
# GPL parts, libass, libplacebo, MoltenVK and what they link), the
# versions and checksums of MPVKit 1.0.0's Package.swift, as static
# xcframeworks. Only the iOS device and simulator slices are kept.
# crates/iptv-app/build.rs links them into the iOS app (`tv.player` on
# libmpv, crates/iptv-player-mpv/src/mpv/ios.rs).
# Usage: tools/fetch-mpvkit.sh [directory, default target/mpvkit]
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$root/target/mpvkit}
mkdir -p "$out/zips"
out=$(cd "$out" && pwd)

# name url sha256
list() {
  cat <<'EOF'
Libmpv https://github.com/mpvkit/MPVKit/releases/download/1.0.0/Libmpv.xcframework.zip c381ceb4c1504efac12da95293e56585bbeb691634aa64e3a729f517169933ba
Libuchardet https://github.com/mpvkit/libuchardet-build/releases/download/0.0.8/Libuchardet.xcframework.zip ea4f548a230a755e059144657cc9e2ff563c1cdeae03974c38f8b6e1a40303fb
Libbluray https://github.com/mpvkit/libbluray-build/releases/download/1.4.0/Libbluray.xcframework.zip bc037d34e2b0b5ab7f202fb371f5fb298136cc66fdf406c2172185d06f53f18d
Libavcodec https://github.com/mpvkit/MPVKit/releases/download/1.0.0/Libavcodec.xcframework.zip 136e432919a8a7b5b80155c68e9dc91b0ef3ae6623970b87bb8bd96a452543cf
Libavdevice https://github.com/mpvkit/MPVKit/releases/download/1.0.0/Libavdevice.xcframework.zip 65713615f53352b37baffb38c5b88d88bab647d050d999bcc4684344ab636bdf
Libavfilter https://github.com/mpvkit/MPVKit/releases/download/1.0.0/Libavfilter.xcframework.zip 3e71af8633d99d365f14b88064bceafd57acb23fc64db90e55b715a9c0dca328
Libavformat https://github.com/mpvkit/MPVKit/releases/download/1.0.0/Libavformat.xcframework.zip 2afb601375929640e743e7bdaa6c4a88e2b582a07e1c5f2dc95cc7f5b26a0810
Libavutil https://github.com/mpvkit/MPVKit/releases/download/1.0.0/Libavutil.xcframework.zip 5dc251c8807c501982edfb0bc9bddfee4148733142d6ebb947738c60fb3bf8d8
Libswresample https://github.com/mpvkit/MPVKit/releases/download/1.0.0/Libswresample.xcframework.zip d5c36acf2ff944e15706f4b7bfbf18bb1993ffc5b446c9f67f1aa79de5441f15
Libswscale https://github.com/mpvkit/MPVKit/releases/download/1.0.0/Libswscale.xcframework.zip 4fc00de6a7a8cddcdfe0eeb73b4f919cfaf9a0fd5a442f42a43e0f56e157d7c5
Libssl https://github.com/mpvkit/openssl-build/releases/download/3.3.5/Libssl.xcframework.zip ff5ffd43d015d7285fd37e4a3145b25cbd8d2842740bd629a711c299a20e226a
Libcrypto https://github.com/mpvkit/openssl-build/releases/download/3.3.5/Libcrypto.xcframework.zip 593283be2a90f7fd66f6e6ed331b2f099cf403e0926fe3b4ac09a7062b793965
Libass https://github.com/mpvkit/libass-build/releases/download/0.17.5/Libass.xcframework.zip 3f4c576d2818ceb4544aa2a20e1f55846511c5e706fd19adc3ea9fd842270498
Libfreetype https://github.com/mpvkit/libass-build/releases/download/0.17.5/Libfreetype.xcframework.zip 496ca62488530e14b1e4624d20ee2b237c0bd675cd70c19da578a5768302d02d
Libfribidi https://github.com/mpvkit/libass-build/releases/download/0.17.5/Libfribidi.xcframework.zip bc15e097b892f2f90424e4a27ba287070cc2f98a74a4da10e6d2481d15cf5ff9
Libharfbuzz https://github.com/mpvkit/libass-build/releases/download/0.17.5/Libharfbuzz.xcframework.zip aa8e0b9ca0387dac74e3e93c86e34d11982bb013b28022d0e6966a8427a35b2e
MoltenVK https://github.com/mpvkit/moltenvk-build/releases/download/1.4.2/MoltenVK.xcframework.zip aee189c54ad7c62bf734a3dc51eb4cfad5685d1d63b0ec519ecd1b437c332418
Libshaderc_combined https://github.com/mpvkit/libshaderc-build/releases/download/2025.5.0/Libshaderc_combined.xcframework.zip 758047b615708575b580eb960a2d083f760a29dc462d6eaa360416c946ce433b
lcms2 https://github.com/mpvkit/lcms2-build/releases/download/2.17.0/lcms2.xcframework.zip dc0dce0606f6ab6841a8ec5a6bd4448e2f3ef00661a050460f806c9393dc6982
Libplacebo https://github.com/mpvkit/libplacebo-build/releases/download/7.360.1/Libplacebo.xcframework.zip 2fa3d54cb81f302d6f11c7b2f509af30944381c3b11ee9d35096eb4637a6e2dd
Libdovi https://github.com/mpvkit/libdovi-build/releases/download/3.3.2/Libdovi.xcframework.zip e693e239808350868e79c5448ef9f02e2716bc822dd8632a41a368a1eae5ca7d
Libunibreak https://github.com/mpvkit/libass-build/releases/download/0.17.5/Libunibreak.xcframework.zip 940d9833cf4477d0a260d9f2b4066125bc0ff7bbc111ac3c90e774765b77a559
gmp https://github.com/mpvkit/gnutls-build/releases/download/3.8.11/gmp.xcframework.zip ad33c7a08f4cdcb9924c8f0e6d9a054dad33d7794b97667bf8b6fb2b236ae585
nettle https://github.com/mpvkit/gnutls-build/releases/download/3.8.11/nettle.xcframework.zip 0fdf3ebf8bd7b8bc8eee837cf27261cb4c52ae520b6576a2f468656aa1691e02
hogweed https://github.com/mpvkit/gnutls-build/releases/download/3.8.11/hogweed.xcframework.zip 25727c9fa67287fa0a4f4722f88bb8be669b23cd7e837e2d00870eb8a25d3f27
gnutls https://github.com/mpvkit/gnutls-build/releases/download/3.8.11/gnutls.xcframework.zip 3dbec5809339189bf9679e218c6cff387ebf8fb72745927835afc2678f5c9f4d
Libdav1d https://github.com/mpvkit/libdav1d-build/releases/download/1.5.3/Libdav1d.xcframework.zip d1a32ae6a1f0193e9f05c44c9176844af7f6d2a58cb33843f6f1b8dfd9224083
Libuavs3d https://github.com/mpvkit/libuavs3d-build/releases/download/1.2.1-fix/Libuavs3d.xcframework.zip bd5256081486d16c51c868d755bf70266c424b54c895269580de44ec6707f789
EOF
}

list | while read -r name url sum; do
  dest="$out/$name.xcframework"
  if [ -d "$dest" ]; then
    continue
  fi
  zip="$out/zips/$name.zip"
  if [ ! -f "$zip" ] || [ "$(shasum -a 256 "$zip" | cut -d' ' -f1)" != "$sum" ]; then
    echo "fetching $name"
    curl -fsSL --retry 3 -o "$zip.part" "$url"
    mv "$zip.part" "$zip"
  fi
  got=$(shasum -a 256 "$zip" | cut -d' ' -f1)
  if [ "$got" != "$sum" ]; then
    echo "checksum mismatch for $name: $got" >&2
    exit 1
  fi
  tmp=$(mktemp -d "$out/unzip.XXXXXX")
  unzip -q "$zip" -d "$tmp"
  # The iOS slices only (device; simulator, arm64 and x86_64).
  for slice in "$tmp/$name.xcframework"/*/; do
    case "$(basename "$slice")" in
      ios-arm64 | ios-arm64_x86_64-simulator | ios-arm64-simulator) ;;
      *) rm -rf "$slice" ;;
    esac
  done
  mv "$tmp/$name.xcframework" "$dest"
  rm -rf "$tmp"
done
# Replace only libavformat; the builder uses the verified stock frameworks as
# its baseline. The receipt makes reuse sensitive to a changed native pin.
if [ "${PATINA_MPVKIT_BASE_ONLY:-0}" != 1 ]; then
  guarded=$(python3 "$root/tools/ci/native_candidates.py" ios)
  expected=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["sha256"])' "$root/tools/mpv-runtime-ios/runtime-ios.json")
  receipt="$out/.patina-libavformat-sha256"
  if [ ! -f "$receipt" ] || [ "$(cat "$receipt")" != "$expected" ]; then
    tmp=$(mktemp -d "$out/guarded.XXXXXX")
    unzip -q "$guarded" -d "$tmp"
    test -d "$tmp/Libavformat.xcframework"
    rm -rf "$out/Libavformat.xcframework"
    mv "$tmp/Libavformat.xcframework" "$out/Libavformat.xcframework"
    rmdir "$tmp"
    printf '%s\n' "$expected" > "$receipt"
  fi
fi
du -sh "$out"/*.xcframework | sort -h | tail -5
echo "$out"
