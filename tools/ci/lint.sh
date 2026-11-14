#!/usr/bin/env bash
# Linters for shell, python and kotlin sources.
set -euo pipefail
cd "$(dirname "$0")/../.."

failed=0

echo "==> ShellCheck"
if command -v shellcheck >/dev/null 2>&1; then
  git ls-files '*.sh' | xargs shellcheck || failed=1
else
  echo "note: shellcheck not installed, skipping"
fi

echo "==> Ruff"
if command -v ruff >/dev/null 2>&1; then
  ruff check tools/ || failed=1
else
  echo "note: ruff not installed, skipping"
fi

echo "==> ktlint"
if command -v ktlint >/dev/null 2>&1; then
  ktlint --baseline=hosts/android/ktlint-baseline.xml "hosts/android/**/*.kt" || failed=1
else
  echo "note: ktlint not installed, skipping"
fi

exit "$failed"
