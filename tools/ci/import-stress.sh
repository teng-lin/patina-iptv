#!/usr/bin/env bash
# Allocation budgets run explicitly, including the ignored million-program EPG.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test --locked --release -p iptv-store-sqlite --test memory -- --include-ignored --format=pretty
