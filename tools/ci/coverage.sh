#!/usr/bin/env bash
# Line coverage of the workspace tests, with cargo-llvm-cov 0.9.1 and
# the toolchain's llvm-tools component. Enforces coverage floors per measured
# crate (tools/ci/check-coverage-floors.py). Headless application scenarios also
# exercise the runtime's actual shell/service handoff and isolated child processes.
# Usage: tools/ci/coverage.sh [-p CRATE ...]   (default: every workspace crate)
# Writes target/coverage/lcov.info, target/coverage/html/ and summary.txt.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ "$#" -eq 0 ] && [ "$(uname -s)" != Linux ]; then
  echo 'The default coverage floors are measured on Linux CI; use -p CRATE for a host-specific report.' >&2
  exit 2
fi
# The workspace crates tools/check.sh tests (tools/ci/test-check-parity.py).
crates=(-p iptv-core -p iptv-nav -p iptv-settings -p iptv-epg -p iptv-providers -p iptv-guide -p iptv-import -p iptv-net -p iptv-store-sqlite -p iptv-sqlcipher-init -p iptv-search -p iptv-player-mpv -p iptv-library -p iptv-runtime -p iptv-app)
[ "$#" -eq 0 ] || crates=("$@")
features=()
if [ "$#" -eq 0 ]; then
  # Match tools/check.sh: include the native mock-provider and admission scenarios.
  features=(--features iptv-runtime/test-support,iptv-app/test-support)
fi
out=target/coverage
rm -rf "$out"
mkdir -p "$out"
cargo llvm-cov clean --workspace
cargo llvm-cov --locked --no-report "${features[@]}" "${crates[@]}"
if [ "$#" -eq 0 ]; then
  if [ "$(uname -s)" = Linux ]; then
    python3 tools/ci/run-xvfb.py cargo llvm-cov test --locked --no-report \
      -p iptv-app --features test-support --test native_player -- \
      --ignored --exact native_gtk_player_shares_pip_frames_and_recreates_only_after_context_loss
  fi
  cargo llvm-cov test --locked --no-report -p iptv-runtime --features test-support --lib \
    guide_repair::tests::repair_analysis_on_100k_channels_and_1k_groups_keeps_display_lane_responsive \
    -- --ignored --exact
fi
cargo llvm-cov report --lcov --output-path "$out/lcov.info"
cargo llvm-cov report --html --output-dir "$out"
cargo llvm-cov report --summary-only | tee "$out/summary.txt"
if [ "$#" -eq 0 ]; then
  python3 tools/ci/check-coverage-floors.py "$out/summary.txt"
fi
