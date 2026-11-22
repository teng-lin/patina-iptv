#!/usr/bin/env bash
# Builds Patina's NSH interface for Samsung Tizen TVs: iptv-app on
# nsh-platform-tizen (pinned in Cargo.toml), for Tizen 5.5's WebAssembly
# (MVP + sign-extension + mutable globals, std rebuilt on the pinned
# nightly), with the `tizen` profile (release: LTO, one codegen unit,
# panic=abort). Stages the page into a signed .wgt beside the JavaScript
# bridge (hosts/tizen), which stays the default package for 2020 sets.
#
#   hosts/tizen-nsh/build.sh                      build, stage and sign target/tizen-nsh/PatinaNsh.wgt
#   hosts/tizen-nsh/build.sh --stage              build and stage only (no Tizen Studio, no signing)
#   hosts/tizen-nsh/build.sh --install HOST:PORT  also install it on a TV in Developer Mode and run it
#
#   NSH_OPT=s|z|3       opt-level of the tizen profile (default: the profile's, z)
#   NSH_WASM_OPT=0      skip binaryen's wasm-opt -Oz (MVP + the two features),
#                       which runs by default when wasm-opt is on PATH
#   NSH_SIZE=0          build without the size flags below (for comparisons)
#   NSH_UNSTRIPPED=1    keep the names section (for twiggy; never installed)
#   NSH_STD_FEATURES    -Zbuild-std-features for the rebuilt std
#   SQLITE_WASM_RS_FLAGS  SQLite's compile options over sqlite-wasm-rs's
#                       (default: the TV's, below; empty for its own)
#   NSH_RUSTFLAGS       more rustc flags
#
# The size flags (the plan's "Module size and launch" measures each):
# std built with optimize_for_size, panics that abort at once
# (-Cpanic=immediate-abort: a panic is a trap without its message, which
# the platform reports as the app's fatal error), and no panic locations.
# -Zfmt-debug=none is not used: it would also empty `{:?}` output the app
# shows (key labels) and its logs. With them, when `clang` has rustc's
# LLVM major version, SQLite's C code joins the module's link-time
# optimization (-flto and -Clinker-plugin-lto: cross-language inlining
# and dead code across the boundary); otherwise it is linked as objects.
#   TIZEN_APP_ID        the application id (default PatinaNsh1.Patina; Tizen packages are 10 letters or digits)
#
# Signing uses a local Tizen Studio certificate profile named by
# TIZEN_SIGNING_PROFILE (keep it out of the repository). The SQLite C code
# is compiled for the same feature set (CFLAGS below).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
out="$root/target/tizen-nsh"
nightly="$("$root/tools/ci/fuzz-imports.sh" --toolchain)"
app_id="${TIZEN_APP_ID:-PatinaNsh1.Patina}"
mode="${1:-}"
install_host=""
if [[ "$mode" == "--install" ]]; then
    install_host="${2:?--install needs the TV address}"
fi

config=()
if [[ -n "${NSH_OPT:-}" ]]; then
    config+=(--config "profile.tizen.opt-level=\"${NSH_OPT}\"")
    [[ "$NSH_OPT" =~ ^[0-3]$ ]] && config=(--config "profile.tizen.opt-level=${NSH_OPT}")
fi
if [[ -n "${NSH_UNSTRIPPED:-}" ]]; then
    config+=(--config 'profile.tizen.strip=false' --config 'profile.tizen.debug=false')
fi

flags="-C target-cpu=mvp -C target-feature=+sign-ext,+mutable-globals"
cflags="-mcpu=mvp -msign-ext -mmutable-globals"
if [[ "${NSH_SIZE:-1}" != 0 ]]; then
    flags+=" -Zunstable-options -Cpanic=immediate-abort -Zlocation-detail=none"
    NSH_STD_FEATURES="${NSH_STD_FEATURES:-optimize_for_size}"
    rust_llvm="$(rustc "+$nightly" -vV | sed -n 's/^LLVM version: \([0-9]*\).*/\1/p')"
    clang_llvm="$(clang --version 2>/dev/null | sed -n 's/.*clang version \([0-9]*\).*/\1/p' | head -1)"
    if [[ -n "$rust_llvm" && "$rust_llvm" == "$clang_llvm" ]]; then
        flags+=" -Clinker-plugin-lto"
        cflags+=" -flto"
    else
        echo "warning: clang's LLVM (${clang_llvm:-none}) is not rustc's ($rust_llvm): SQLite stays out of LTO" >&2
    fi
fi
flags+="${NSH_RUSTFLAGS:+ $NSH_RUSTFLAGS}"
std_features=()
if [[ -n "${NSH_STD_FEATURES:-}" ]]; then
    std_features=("-Zbuild-std-features=$NSH_STD_FEATURES")
fi
# SQLite without what Patina never asks of it (sqlite-wasm-rs enables
# these extensions and APIs for every app): R*Tree, sessions and the
# preupdate hook, the introspection virtual tables, column metadata,
# sqlite_offset, unlock notify, the math functions, authorizers, trace
# and profile, UTF-16 entry points, sqlite3_complete, sqlite3_get_table,
# Tcl variables, the introspection pragmas, deserialize, declared types,
# auto-vacuum and memory statistics. FTS5 (channel search), JSON (the
# store's queries) and EXPLAIN (the store's query plan tests) stay.
# A function a build still calls fails the link. The store's wasm tests
# pass with them: SQLITE_WASM_RS_FLAGS="…" node tools/wasm-test.mjs --cargo
# --locked -p iptv-store-sqlite --lib --test it.
if [[ -z "${SQLITE_WASM_RS_FLAGS+set}" ]]; then
    sqlite_flags=""
    for option in RTREE SESSION PREUPDATE_HOOK BYTECODE_VTAB DBPAGE_VTAB DBSTAT_VTAB STMTVTAB \
        UNKNOWN_SQL_FUNCTION COLUMN_METADATA OFFSET_SQL_FUNC UNLOCK_NOTIFY MATH_FUNCTIONS; do
        sqlite_flags+=" -USQLITE_ENABLE_$option"
    done
    for option in AUTHORIZATION TRACE UTF16 COMPLETE GET_TABLE TCL_VARIABLE INTROSPECTION_PRAGMAS \
        DESERIALIZE DECLTYPE AUTOVACUUM; do
        sqlite_flags+=" -DSQLITE_OMIT_$option"
    done
    export SQLITE_WASM_RS_FLAGS="${sqlite_flags# } -DSQLITE_DEFAULT_MEMSTATUS=0"
fi
echo "Building the module (profile tizen${NSH_OPT:+, opt-level $NSH_OPT}; $flags${NSH_STD_FEATURES:+; std features $NSH_STD_FEATURES}; C $cflags)…"
(cd "$here" && RUSTFLAGS="$flags" \
    CFLAGS_wasm32_unknown_unknown="$cflags" \
    cargo "+$nightly" build -Zbuild-std=std,panic_abort "${std_features[@]}" --target wasm32-unknown-unknown \
    --profile tizen --locked --target-dir "$out/cargo" "${config[@]}")
module="$out/cargo/wasm32-unknown-unknown/tizen/patina_tizen.wasm"

# The platform's page and Worker scripts, from the pinned checkout.
platform="$(cd "$here" && cargo "+$nightly" metadata --format-version 1 --locked 2>/dev/null |
    python3 -c 'import json,sys; m=json.load(sys.stdin); print(next(p["manifest_path"] for p in m["packages"] if p["name"]=="nsh-platform-tizen"))')"
runtime="$(dirname "$platform")/../../runtime"

stage="$out/stage"
rm -rf "$stage"
mkdir -p "$stage"
"$runtime/bundle.sh" "$stage"
rm -f "$stage/nsh-bench.js" "$stage/nsh-engine.js"
cp "$here"/app/{index.html,page.css,patina-host.js,patina-worker.js} "$stage/"
cp "$root/art/icon/play-512.png" "$stage/icon.png"
# iptv-app's data as package files (its `browser-package` feature): the
# splash, the fonts the page loads (patina-host.js) and one file of
# messages per interface language, read when the language is shown.
mkdir -p "$stage/assets" "$stage/fonts"
cp "$root/art/icon/banner.png" "$stage/assets/banner.png"
cp "$root"/crates/iptv-app/fonts/*.woff2 "$stage/fonts/"
python3 "$here/catalog.py" "$root/crates/iptv-core/src/i18n/messages.tsv" \
    "$root/crates/iptv-core/src/i18n.rs" "$stage/i18n"
cp "$module" "$stage/app.wasm"
if [[ "${NSH_WASM_OPT:-1}" != 0 ]]; then
    # Binaryen (version_132 measured) keeps the module to MVP plus the two
    # features the TV validates.
    if command -v wasm-opt >/dev/null; then
        wasm-opt -Oz --mvp-features --enable-sign-ext --enable-mutable-globals \
            "$stage/app.wasm" -o "$stage/app.wasm"
    else
        echo "warning: wasm-opt is not installed: the module is about 2 MB larger (binaryen)" >&2
    fi
fi
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
sed -e "s/__VERSION__/$version/" -e "s/__APP_ID__/$app_id/" -e "s/__PACKAGE__/${app_id%%.*}/" \
    "$here/app/config.xml" >"$stage/config.xml"
# The module's SHA-256 keys the compiled-module cache: an update never
# runs the old compiled code.
sha="$(sha256sum "$stage/app.wasm" | cut -c1-64)"
printf "/* Written by build.sh. */\nwindow.NSH_MODULE_VERSION = '%s';\n" "$sha" >"$stage/app-config.js"
python3 "$here/sections.py" "$stage/app.wasm"

if [[ "$mode" == "--stage" ]]; then
    echo "Staged in $stage"
    exit 0
fi
tizen_cli="$(command -v tizen || true)"
if [[ -z "$tizen_cli" && -x "$HOME/tizen-studio/tools/ide/bin/tizen" ]]; then
    tizen_cli="$HOME/tizen-studio/tools/ide/bin/tizen"
fi
if [[ -z "${TIZEN_SIGNING_PROFILE:-}" || -z "$tizen_cli" ]]; then
    echo "error: signing needs TIZEN_SIGNING_PROFILE and Tizen Studio's CLI (or use --stage)" >&2
    exit 1
fi
rm -f "$stage"/*.wgt "$out/PatinaNsh.wgt"
"$tizen_cli" package -t wgt -s "$TIZEN_SIGNING_PROFILE" -- "$stage"
shopt -s nullglob
packages=("$stage"/*.wgt)
if [[ ${#packages[@]} -ne 1 ]]; then
    echo "error: tizen package produced no .wgt (see its output above)" >&2
    exit 1
fi
mv "${packages[0]}" "$out/PatinaNsh.wgt"
echo "Built $out/PatinaNsh.wgt"
if [[ -n "$install_host" ]]; then
    sdb="$(command -v sdb || echo "$HOME/tizen-studio/tools/sdb")"
    "$sdb" connect "$install_host" >/dev/null
    "$tizen_cli" install -s "$install_host" -n PatinaNsh.wgt -- "$out"
    "$tizen_cli" run -s "$install_host" -p "$app_id"
fi
