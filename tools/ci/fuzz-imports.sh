#!/usr/bin/env bash
# Fuzz the parser and rewriter boundaries with synthetic inputs: every target
# `cargo fuzz list` reports (the [[bin]] entries of fuzz/Cargo.toml), or only
# the ones named.
# Usage: tools/ci/fuzz-imports.sh [SECONDS per target, default 300] [TARGET ...]
#        tools/ci/fuzz-imports.sh --toolchain    print the pinned nightly
# cargo-fuzz 0.13.2 calls `cargo build` through PATH and has no --locked flag.
# Reuse the release wrapper so both harness metadata and builds enforce its lock.
set -euo pipefail
cd "$(dirname "$0")/../.."
export RUSTUP_TOOLCHAIN="${IPTV_FUZZ_TOOLCHAIN:-nightly-2026-10-24}"
if [ "${1:-}" = --toolchain ]; then
  echo "$RUSTUP_TOOLCHAIN"
  exit
fi
seconds=${1:-300}
shift || true
export IPTV_REAL_CARGO
IPTV_REAL_CARGO=$(rustup which --toolchain "$RUSTUP_TOOLCHAIN" cargo)
fuzz_bin=$(mktemp -d)
trap 'rm -rf "$fuzz_bin"' EXIT
ln -s "$PWD/tools/ci/locked-cargo.py" "$fuzz_bin/cargo"
export PATH="$fuzz_bin:$PATH"
known=$(cargo-fuzz list)
[ -n "$known" ] || { echo "cargo fuzz list reports no targets" >&2; exit 1; }
failed=()
targets=("$@")
[ "${#targets[@]}" -gt 0 ] || read -r -a targets <<< "$known"
for target in "${targets[@]}"; do
  grep -qxF -- "$target" <<< "$known" || { echo "unknown fuzz target: $target" >&2; exit 1; }
  # Committed seeds join whatever corpus earlier runs accumulated.
  mkdir -p "fuzz/corpus/$target"
  if [ -d "fuzz/seeds/$target" ]; then
    cp -R "fuzz/seeds/$target/." "fuzz/corpus/$target/"
  fi
  # Every target gets its time, also after an earlier one found a crash.
  cargo-fuzz run "$target" -- -max_total_time="$seconds" -max_len=65537 -rss_limit_mb=512 || failed+=("$target")
done
if [ "${#failed[@]}" -gt 0 ]; then
  echo "fuzzing failed for: ${failed[*]} (crashing inputs are under fuzz/artifacts/)" >&2
  exit 1
fi
