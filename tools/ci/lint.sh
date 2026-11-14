#!/usr/bin/env bash
# Linters for shell, python and kotlin sources.
set -euo pipefail
cd "$(dirname "$0")/../.."

failed=0
missing() {
  if [ "${IPTV_REQUIRE_LINTERS:-0}" = 1 ]; then
    echo "error: required linter $1 is not installed" >&2
    failed=1
  else
    echo "note: $1 not installed, skipping"
  fi
}

echo "==> ShellCheck"
if command -v shellcheck >/dev/null 2>&1; then
  git ls-files -z '*.sh' | xargs -0 shellcheck || failed=1
else
  missing shellcheck
fi

echo "==> Ruff"
if command -v ruff >/dev/null 2>&1; then
  ruff check tools/ || failed=1
else
  missing ruff
fi

echo "==> ktlint"
if command -v ktlint >/dev/null 2>&1; then
  ktlint --baseline=hosts/android/ktlint-baseline.xml "hosts/android/**/*.kt" || failed=1
else
  missing ktlint
fi

exit "$failed"
