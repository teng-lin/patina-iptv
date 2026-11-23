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
workers=${IPTV_FUZZ_JOBS:-1}
case "$workers" in 1|2|3) ;; *) echo "unsupported fuzz worker count: $workers" >&2; exit 2 ;; esac
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
if [ "${#targets[@]}" -eq 0 ]; then
  while IFS= read -r target; do
    targets+=("$target")
  done <<< "$known"
fi
seen=" "
for target in "${targets[@]}"; do
  grep -qxF -- "$target" <<< "$known" || { echo "unknown fuzz target: $target" >&2; exit 1; }
  if [ "$workers" -gt 1 ] && [[ "$seen" == *" $target "* ]]; then
    echo "parallel fuzz targets must be unique: $target" >&2
    exit 2
  fi
  seen+="$target "
done

run_target() {
  local target=$1
  # Committed seeds join whatever corpus earlier runs accumulated.
  mkdir -p "fuzz/corpus/$target" || return
  if [ -d "fuzz/seeds/$target" ]; then
    cp -R "fuzz/seeds/$target/." "fuzz/corpus/$target/" || return
  fi
  # Every target gets its time, also after an earlier one found a crash.
  # One input may allocate at most 512 MiB (a decompression bomb still fails);
  # the process may grow to 1 GiB, as ASan quarantines up to 256 MiB of freed
  # memory while it replays the corpus earlier runs accumulated.
  cargo-fuzz run "$target" -- -max_total_time="$seconds" -max_len=65537 \
    -rss_limit_mb=1024 -malloc_limit_mb=512
}

pids=()
active=()
finish_batch() {
  local index
  for index in "${!pids[@]}"; do
    wait "${pids[index]}" || failed+=("${active[index]}")
  done
  pids=()
  active=()
}

if [ "$workers" -gt 1 ] && [ "${#targets[@]}" -gt 1 ]; then
  # Build the instrumented targets once before starting up to three distinct
  # libFuzzer processes. Each process owns its corpus and crash directory.
  cargo-fuzz build
  for target in "${targets[@]}"; do
    run_target "$target" &
    pids+=("$!")
    active+=("$target")
    if [ "${#pids[@]}" -eq "$workers" ]; then
      finish_batch
    fi
  done
  finish_batch
else
  for target in "${targets[@]}"; do
    run_target "$target" || failed+=("$target")
  done
fi
if [ "${#failed[@]}" -gt 0 ]; then
  echo "fuzzing failed for: ${failed[*]} (crashing inputs are under fuzz/artifacts/)" >&2
  exit 1
fi
