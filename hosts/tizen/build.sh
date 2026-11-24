#!/usr/bin/env bash
# Builds Patina for Samsung Tizen TVs: the core (crates/iptv-web) for the
# oldest supported engine's WebAssembly features, staged with the page into
# a signed .wgt.
#
#   hosts/tizen/build.sh                    build and sign target/tizen/Patina.wgt
#   hosts/tizen/build.sh --install HOST:PORT  also install it on a TV in Developer Mode
#
# Signing uses a local Tizen Studio certificate profile named by
# TIZEN_SIGNING_PROFILE (Certificate Manager; keep it out of the repository).
# Requires Tizen Studio's `tizen` CLI, a nightly toolchain with rust-src,
# and Python 3 (the interface languages' package files).
set -euo pipefail

command -v python3 >/dev/null || { echo "build.sh: python3 is needed to write the language files" >&2; exit 1; }

root="$(cd "$(dirname "$0")/../.." && pwd)"
out="$root/target/tizen"
# Tizen 5.5 (2020, Chromium 69) validates only MVP WebAssembly plus
# sign-extension and mutable globals. Stable Rust's std uses reference types
# and bulk memory, so std is rebuilt for that feature set (needs nightly).
# The repository's pinned nightly (shared with the fuzz targets).
nightly="$("$root/tools/ci/fuzz-imports.sh" --toolchain)"
features="-C target-cpu=mvp -C target-feature=+sign-ext,+mutable-globals"

install_host=""
if [[ "${1:-}" == "--install" ]]; then
    install_host="${2:?--install needs the TV address}"
fi

tizen_cli="$(command -v tizen || true)"
if [[ -z "$tizen_cli" && -x "$HOME/tizen-studio/tools/ide/bin/tizen" ]]; then
    tizen_cli="$HOME/tizen-studio/tools/ide/bin/tizen"
fi

echo "Building the core for the TV engine…"
RUSTFLAGS="$features" cargo "+$nightly" build \
    -Zbuild-std=std,panic_abort \
    --target wasm32-unknown-unknown \
    --release -p iptv-web \
    --target-dir "$root/target/tizen-wasm" \
    --config 'profile.release.panic="abort"' \
    --config 'profile.release.lto=true' \
    --config 'profile.release.codegen-units=1' \
    --config 'profile.release.strip=true'

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
stage="$out/stage"
rm -rf "$stage"
mkdir -p "$stage"
cp "$root"/hosts/tizen/app/{index.html,main.js,worker.js,style.css} "$stage/"
cp "$root/art/icon/play-512.png" "$stage/icon.png"
cp "$root/target/tizen-wasm/wasm32-unknown-unknown/release/iptv_web.wasm" "$stage/"
# The interface languages' messages as package files, one per language,
# read by the core when it first words something in that language (the
# exit prompt): the same files as the NSH host's.
python3 "$root/hosts/tizen-nsh/catalog.py" "$root/crates/iptv-presentation/src/i18n/messages.tsv" \
    "$root/crates/iptv-presentation/src/i18n.rs" "$stage/i18n"
sed "s/__VERSION__/$version/" "$root/hosts/tizen/app/config.xml" >"$stage/config.xml"

if [[ -z "${TIZEN_SIGNING_PROFILE:-}" || -z "$tizen_cli" ]]; then
    if [[ -n "$install_host" ]]; then
        echo "error: --install needs a signed package: set TIZEN_SIGNING_PROFILE and install Tizen Studio's CLI" >&2
        exit 1
    fi
    echo "Staged (unsigned) in $stage. Set TIZEN_SIGNING_PROFILE and install Tizen Studio to sign."
    exit 0
fi

echo "Signing with profile $TIZEN_SIGNING_PROFILE…"
rm -f "$stage"/*.wgt "$out/Patina.wgt"
# The CLI can exit 0 without a package: its output says why.
"$tizen_cli" package -t wgt -s "$TIZEN_SIGNING_PROFILE" -- "$stage"
shopt -s nullglob
packages=("$stage"/*.wgt)
if [[ ${#packages[@]} -ne 1 ]]; then
    echo "error: tizen package produced no .wgt (see its output above)" >&2
    exit 1
fi
# The TV's installer rejects file names with spaces.
mv "${packages[0]}" "$out/Patina.wgt"
echo "Built $out/Patina.wgt"

if [[ -n "$install_host" ]]; then
    sdb="$(command -v sdb || echo "$HOME/tizen-studio/tools/sdb")"
    "$sdb" connect "$install_host" >/dev/null
    "$tizen_cli" install -s "$install_host" -n Patina.wgt -- "$out"
    "$tizen_cli" run -s "$install_host" -p PatinaIPTV.Patina
fi
