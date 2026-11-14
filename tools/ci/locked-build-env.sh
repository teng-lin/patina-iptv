# shellcheck shell=bash
# Source before invoking NSH: it honors CARGO for generated builds. Cargo itself
# resets CARGO inside `cargo run`, so NSH's xtask also adds --locked explicitly.
# Pin nested commands that change directory into the sibling NSH workspace.
RUSTUP_TOOLCHAIN=$(python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1], "rb"))["toolchain"]["channel"])' "$(dirname "${BASH_SOURCE[0]}")/../../rust-toolchain.toml")
export RUSTUP_TOOLCHAIN
if command -v rustup >/dev/null 2>&1; then
  IPTV_REAL_CARGO=$(rustup which --toolchain "$RUSTUP_TOOLCHAIN" cargo)
else
  # A distribution or Nix toolchain has no rustup to select the pinned release.
  # Use the Cargo on PATH; the workspace rust-version rejects an older one.
  IPTV_REAL_CARGO=$(command -v cargo) || {
    echo "locked-build-env.sh: neither rustup nor cargo is on PATH" >&2
    # shellcheck disable=SC2317
    return 1 2>/dev/null || exit 1
  }
  echo "locked-build-env.sh: rustup not found; using $IPTV_REAL_CARGO ($("$IPTV_REAL_CARGO" --version 2>/dev/null)) instead of pinned Rust $RUSTUP_TOOLCHAIN" >&2
fi
export IPTV_REAL_CARGO
CARGO_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/locked-cargo.py"
export CARGO="$CARGO_PATH"
