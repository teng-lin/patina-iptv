#!/usr/bin/env bash
# No line of tracked Rust under crates/ may be longer than 200 bytes.
#
# `cargo fmt --check` does not catch these. rustfmt gives up on a chain or
# statement that holds something it cannot fit (a long string literal, a
# dense closure) and leaves the whole of it exactly as written, without a
# word; a 400-character line then passes the format check. Split such code
# into named helpers and `const` strings, wrap a long literal with `\` (or
# `concat!` for a raw string), and rustfmt formats the rest again.
#
# Lengths are counted in bytes (LC_ALL=C), so every awk gives the same answer.
set -euo pipefail
cd "$(dirname "$0")/../.."
limit=200

# shellcheck disable=SC2016
long=$(git ls-files -z 'crates/**/*.rs' |
  LC_ALL=C xargs -0 awk -v limit="$limit" 'length($0) > limit { print FILENAME ":" FNR ": " length($0) }')

if [ -n "$long" ]; then
  printf '%s\n' "$long"
  echo "Lines over $limit bytes: rustfmt leaves such code unformatted. Split it up (see the head of this script)." >&2
  exit 1
fi
