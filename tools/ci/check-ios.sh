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
for target in aarch64-apple-ios aarch64-apple-ios-sim; do
  cargo check --locked -p iptv-app --lib --target "$target"
done
