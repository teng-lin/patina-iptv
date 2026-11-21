#!/usr/bin/env bash
# Rebuilds the bundled fonts in crates/iptv-app/fonts from Google's official
# Roboto release (googlefonts/roboto-3-classic v3.016, SIL OFL 1.1): the
# unhinted static Roboto Regular and Medium and Roboto Condensed Regular,
# subset to Latin (with Latin Extended), Greek, Cyrillic, punctuation and
# the symbols the guide draws, as WOFF2 (NSH unpacks it when it registers
# the font). Every other script falls back to the system's fonts.
# Downloads are checked against pinned SHA-256 sums; the subsetter is a
# pinned fontTools run through uvx. Output names keep their name tables
# (license and copyright fields) intact. Output (fontTools 4.60.1):
#   Roboto-Regular.woff2, Roboto-Medium.woff2, RobotoCondensed-Regular.woff2
#   with the sums printed at the end; see crates/iptv-app/fonts/README.md.
# Usage: tools/fetch-fonts.sh   (needs curl, unzip, sha256sum, uvx)
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/crates/iptv-app/fonts"
release=v3.016
zip_url="https://github.com/googlefonts/roboto-3-classic/releases/download/$release/Roboto_$release.zip"
zip_sha=1653dbe12f248da8fb0b9920db7b9496cd677ed3981154f6f15285c8bd4e334f
ofl_url="https://raw.githubusercontent.com/googlefonts/roboto-3-classic/$release/OFL.txt"
ofl_sha=061402327a96aadb0bfb694a960ed289ecd38d383e396243831ab81feb109c41
fonttools=fonttools==4.60.1
brotli=brotli==1.1.0
# Basic Latin + Latin-1, Latin Extended-A/B, IPA/spacing modifiers and
# combining marks, Greek, Cyrillic (+ supplement), Latin Extended
# Additional (Vietnamese), punctuation, super/subscripts, currency,
# letterlike symbols, arrows, geometric shapes and misc symbols where Roboto
# has them (it has no ▶ or ★: those come from the system's fonts).
unicodes="U+0000-024F,U+02B0-036F,U+0370-03FF,U+0400-052F,U+1E00-1EFF,U+2000-206F,U+2070-209F,U+20A0-20CF,U+2100-214F,U+2190-21FF,U+25A0-25FF,U+2600-26FF,U+FEFF,U+FFFD"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/roboto.zip" "$zip_url"
echo "$zip_sha  $tmp/roboto.zip" | sha256sum -c --quiet
curl -fsSL -o "$tmp/OFL.txt" "$ofl_url"
echo "$ofl_sha  $tmp/OFL.txt" | sha256sum -c --quiet

mkdir -p "$out"
for face in Roboto-Regular Roboto-Medium RobotoCondensed-Regular; do
  unzip -q -o -j "$tmp/roboto.zip" "unhinted/static/$face.ttf" -d "$tmp"
  uvx --from "$fonttools" --with "$brotli" pyftsubset "$tmp/$face.ttf" \
    --unicodes="$unicodes" \
    --layout-features='*' \
    --name-IDs='*' --name-legacy --name-languages='*' \
    --notdef-outline --no-hinting --flavor=woff2 \
    --output-file="$out/$face.woff2"
done
cp "$tmp/OFL.txt" "$out/OFL.txt"
(cd "$out" && rm -f ./*.ttf && sha256sum ./*.woff2 OFL.txt)
