#!/usr/bin/env bash
# Every host-side check: formatting, core tests, wasm builds of the portable
# crates, and the app build. Run before committing.
set -euo pipefail
cd "$(dirname "$0")/.."
tier=${1:-full}
case "$tier" in pr|full) ;; *) echo "usage: tools/check.sh [pr|full]" >&2; exit 2 ;; esac
case "$(uname -s)" in
  Linux) host=linux ;;
  Darwin) host=macos ;;
  *) echo 'tools/check.sh supports Linux and macOS; use tools/build-windows.ps1 on Windows' >&2; exit 2 ;;
esac
# macOS strips DYLD_* when starting a protected shell. Restore the selected
# runtime inside this shell; the Python planner also restores it for Cargo.
if [ "$(uname -s)" = Darwin ] && [ -n "${IPTV_RS_MPV_LIB_DIR:-}" ]; then
  export DYLD_LIBRARY_PATH="$IPTV_RS_MPV_LIB_DIR"
fi
# NSH is a path dependency, so Cargo.lock cannot pin it: nsh.rev does. Fail
# before compiling against another commit (IPTV_ALLOW_NSH_DRIFT=1 allows it).
python3 tools/ci/pins.py check-nsh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tools -p test_version.py
# Only our packages: `--all` would also format path dependencies (the NSH checkout).
python3 tools/ci/check-plan.py format
bash tools/ci/check-line-length.sh
bash tools/ci/lint.sh
cargo deny --locked check
python3 tools/ci/check-plan.py run --tier "$tier" --host "$host"
IPTV_REAL_CARGO="$(command -v cargo)" python3 tools/ci/test-locked-cargo.py
python3 tools/ci/test-mpvkit-notices.py
python3 tools/ci/test-macos-source-notices.py
python3 tools/ci/test-macos-dmg.py
python3 tools/ci/test-linux-native-notices.py
python3 tools/ci/test-crypto-native-notices.py
python3 tools/ci/check-package-lock.py
python3 tools/ci/test-package-lock.py
python3 tools/ci/test-ios-entitlements.py
python3 tools/ci/test-unpack-signing-input.py
python3 tools/ci/test-nsh-tool-key.py
python3 tools/ci/test-ci-cache.py
python3 tools/ci/test-pins.py
python3 tools/ci/test-android-changes.py
python3 tools/ci/test-adb-timeout.py
python3 tools/ci/test-android-lifecycle.py
python3 tools/ci/test-android-artifacts.py
python3 tools/ci/test-publish-release.py
python3 tools/ci/test-rehearsal.py
python3 tools/ci/test-release-workflow.py
python3 tools/ci/test-sign-android.py
python3 tools/ci/test-sbom.py
python3 tools/ci/test-coverage-floors.py
python3 tools/ci/test-native-sources.py
python3 tools/ci/test-native-candidates.py
python3 tools/mpv-runtime-ios/test_build.py
python3 tools/ci/test-check-parity.py
python3 tools/ci/test-check-plan.py
python3 tools/ci/test-validation-plan.py
python3 tools/ci/test-fuzz-targets.py
python3 tools/performance/test_journeys.py
python3 tools/ci/test-disposable-avd.py
python3 tools/ci/test-isolated-credentials.py
python3 tools/ci/test-release-contract.py
python3 tools/ci/test-repeat-hls-fixtures.py
python3 tools/ci/test-validation-workflow.py
python3 tools/ci/test-dependency-candidate.py
echo "all checks passed"
