#!/usr/bin/env bash
# Compile both supported iOS slices with the same verified frameworks as packaging.
# This is credential-free source validation, not signing or device acceptance.
set -euo pipefail
cd "$(dirname "$0")/../.."
for name in IOS_CERTIFICATE IOS_CERTIFICATE_PASSWORD IOS_PROFILE IOS_SIGN_IDENTITY; do
  if [ -n "${!name:-}" ]; then
    echo "iOS validation must not receive signing inputs" >&2
    exit 1
  fi
done
root=$PWD
source "$root/tools/ci/locked-build-env.sh"
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
bash tools/fetch-mpvkit.sh
export IPTV_RS_MPVKIT="$PWD/target/mpvkit"
cargo check --locked -p iptv-app --lib --target aarch64-apple-ios
# Build the simulator slice: its Keychain needs the entitlements iptv-app
# links itself (#136), so check its objects carry them.
cargo build --locked -p iptv-app --lib --target aarch64-apple-ios-sim
bash tools/check-ios-simulator-entitlements.sh target/aarch64-apple-ios-sim/debug/libiptv_app.rlib
