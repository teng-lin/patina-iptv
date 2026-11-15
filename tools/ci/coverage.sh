#!/usr/bin/env bash
# Line coverage of the library crates' tests, with cargo-llvm-cov 0.9.1 and
# the toolchain's llvm-tools component. Enforces coverage floors per library
# crate (tools/ci/check-coverage-floors.py). The app crate is left out for now;
# its scenario tests run the whole app and need a display stack.
# Usage: tools/ci/coverage.sh [-p CRATE ...]   (default: every library crate)
# Writes target/coverage/lcov.info, target/coverage/html/ and summary.txt.
set -euo pipefail
cd "$(dirname "$0")/../.."
# The library crates tools/check.sh tests (tools/ci/test-check-parity.py).
crates=(-p iptv-core -p iptv-nav -p iptv-settings -p iptv-epg -p iptv-providers -p iptv-guide -p iptv-import -p iptv-net -p iptv-store-sqlite -p iptv-sqlcipher-init -p iptv-search -p iptv-player-mpv -p iptv-library -p iptv-runtime)
[ "$#" -eq 0 ] || crates=("$@")
features=()
if [ "$#" -eq 0 ]; then
  # Match tools/check.sh: include the native mock-provider and admission scenarios.
  features=(--features iptv-runtime/test-support)
fi
out=target/coverage
rm -rf "$out"
mkdir -p "$out"
cargo llvm-cov clean --workspace
cargo llvm-cov --locked --no-report "${features[@]}" "${crates[@]}"
if [ "$#" -eq 0 ]; then
  cargo llvm-cov --locked --no-report -p iptv-runtime --features test-support --lib \
    guide_repair::tests::repair_analysis_on_100k_channels_and_1k_groups_keeps_display_lane_responsive \
    -- --ignored --exact
fi
cargo llvm-cov report --lcov --output-path "$out/lcov.info"
cargo llvm-cov report --html --output-dir "$out"
cargo llvm-cov report --summary-only | tee "$out/summary.txt"
if [ "$#" -eq 0 ]; then
  python3 tools/ci/check-coverage-floors.py "$out/summary.txt"
fi
