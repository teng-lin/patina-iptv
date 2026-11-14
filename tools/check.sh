#!/usr/bin/env bash
# Every host-side check: formatting, core tests, wasm builds of the portable
# crates, and the app build. Run before committing.
set -euo pipefail
cd "$(dirname "$0")/.."
# NSH is a path dependency, so Cargo.lock cannot pin it: nsh.rev does. Fail
# before compiling against another commit (IPTV_ALLOW_NSH_DRIFT=1 allows it).
python3 tools/ci/pins.py check-nsh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tools -p test_version.py
# Only our packages: `--all` would also format path dependencies (the NSH checkout).
cargo fmt -p iptv-core -p iptv-nav -p iptv-settings -p iptv-epg -p iptv-providers -p iptv-guide -p iptv-import -p iptv-net -p iptv-store-sqlite -p iptv-search -p iptv-player-mpv -p iptv-library -p iptv-runtime -p iptv-app --check
bash tools/ci/check-line-length.sh
cargo clippy --locked --workspace --all-targets --features iptv-app/test-support -- -D warnings
cargo deny --locked check
cargo test --locked -p iptv-core -p iptv-nav -p iptv-settings -p iptv-epg -p iptv-providers -p iptv-guide -p iptv-import -p iptv-net -p iptv-store-sqlite -p iptv-search -p iptv-player-mpv -p iptv-library -p iptv-runtime --quiet
for c in iptv-core iptv-nav iptv-settings iptv-epg iptv-providers iptv-guide iptv-import iptv-search iptv-library; do
  cargo build --locked -p "$c" --target wasm32-unknown-unknown --quiet
done
cargo build --locked -p iptv-app --quiet
# The app's own tests, among them headless runs of the whole app (tests/).
cargo test --locked -p iptv-app --features test-support --quiet
# Measure the wall-clock input budget without competing app scenarios. Keep
# the 50 ms limit: shared runners otherwise measure our parallel test load too.
cargo test --locked -p iptv-app --features test-support --quiet --test app \
  store_latency::guide_input_stays_under_budget_while_the_database_writer_is_locked \
  -- --ignored --exact
# A UI read must not queue behind a busy store writer (the read-only connection).
cargo test --locked -p iptv-app --features test-support --quiet --test app \
  store_latency::a_store_read_does_not_wait_for_a_busy_writer \
  -- --ignored --exact
IPTV_REAL_CARGO="$(command -v cargo)" python3 tools/ci/test-locked-cargo.py
python3 tools/ci/test-mpvkit-notices.py
python3 tools/ci/test-macos-source-notices.py
python3 tools/ci/test-linux-native-notices.py
python3 tools/ci/test-crypto-native-notices.py
python3 tools/ci/check-package-lock.py
python3 tools/ci/test-package-lock.py
python3 tools/ci/test-ios-entitlements.py
python3 tools/ci/test-unpack-signing-input.py
python3 tools/ci/test-nsh-tool-key.py
python3 tools/ci/test-pins.py
python3 tools/ci/test-android-changes.py
python3 tools/ci/test-publish-release.py
python3 tools/ci/test-sbom.py
python3 tools/ci/test-native-sources.py
python3 tools/ci/test-check-parity.py
python3 tools/ci/test-fuzz-targets.py
echo "all checks passed"
