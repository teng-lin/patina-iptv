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
# Packages and test-support features come from the shared host-check inventory.
checks=("$@")
if [ "$#" -eq 0 ]; then
  arguments=$(python3 tools/ci/check-plan.py coverage-args)
  read -r -a checks <<< "$arguments"
fi
out=target/coverage
rm -rf "$out"
mkdir -p "$out"
cargo llvm-cov clean --workspace
if [ "$#" -eq 0 ]; then
  cargo llvm-cov --locked --no-report "${checks[@]}" -- --format=pretty
else
  # Preserve custom Cargo/libtest arguments, including their existing separator
  # and output format. Libtest's default already prints each test name.
  cargo llvm-cov --locked --no-report "${checks[@]}"
fi
if [ "$#" -eq 0 ]; then
  # Keep the strict display budget covered without parallel harness contention.
  # Reuse the ordinary run's package/feature graph and compiled test binaries.
  cargo llvm-cov test --locked --no-report "${checks[@]}" \
    foundation_inspection::a_hundred_thousand_channel_analysis_keeps_display_reads_responsive \
    -- --ignored --exact --format=pretty
  if [ "$(uname -s)" = Linux ]; then
    python3 tools/ci/run-xvfb.py cargo llvm-cov test --locked --no-report \
      -p iptv-app --features test-support --test native_player -- \
      --ignored --exact native_gtk_player_shares_pip_frames_and_recreates_only_after_context_loss --format=pretty
  fi
  cargo llvm-cov test --locked --no-report -p iptv-runtime --features test-support --lib \
    guide_repair::tests::repair_analysis_on_100k_channels_and_1k_groups_keeps_display_lane_responsive \
    -- --ignored --exact --format=pretty
fi
cargo llvm-cov report --lcov --output-path "$out/lcov.info"
cargo llvm-cov report --html --output-dir "$out"
cargo llvm-cov report --summary-only | tee "$out/summary.txt"
if [ "$#" -eq 0 ]; then
  python3 tools/ci/check-coverage-floors.py "$out/summary.txt"
fi
