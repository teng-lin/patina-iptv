#!/usr/bin/env bash
# Generate notices and an inventory from the locked dependency graph.
set -euo pipefail
cd "$(dirname "$0")/../.."
mkdir -p target/notices/nsh-tv
cargo about generate --fail --locked --config about.toml tools/ci/notices.hbs > target/notices/THIRD-PARTY.html
cargo metadata --locked --format-version 1 | python3 tools/ci/component-inventory.py Cargo.lock > target/notices/components.json
cp LICENSE crates/iptv-app/fonts/OFL.txt target/notices/
cp vendor/nsh-tv/LICENSE vendor/nsh-tv/README.md target/notices/nsh-tv/
python3 tools/ci/crypto-native-notices.py target/notices/native-crypto
if [ -f crates/iptv-app/.nsh/rust/Cargo.toml ]; then
  bash tools/ci/audit-runners.sh
  cargo about generate --fail --locked --config about.toml --manifest-path crates/iptv-app/.nsh/rust/Cargo.toml tools/ci/notices.hbs > target/notices/NSH-RUNNERS.html
  cargo metadata --locked --manifest-path crates/iptv-app/.nsh/rust/Cargo.toml --format-version 1 | python3 tools/ci/component-inventory.py tools/ci/package.Cargo.lock > target/notices/runner-components.json
fi
