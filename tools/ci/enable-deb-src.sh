#!/usr/bin/env bash
# Add Ubuntu's source package indexes so `apt-get source` can fetch the exact
# sources of bundled libraries (tools/ci/native-sources.py). Run only on an
# ephemeral Ubuntu runner or container: it edits the system's apt sources.
set -euo pipefail
sudo=()
[ "$(id -u)" -eq 0 ] || sudo=(sudo)
if [ -f /etc/apt/sources.list.d/ubuntu.sources ]; then
  # Ubuntu 24.04 and later: one deb822 file.
  "${sudo[@]}" sed -i 's/^Types: deb$/Types: deb deb-src/' /etc/apt/sources.list.d/ubuntu.sources
else
  "${sudo[@]}" sed -i -E 's/^# *(deb-src .*)$/\1/' /etc/apt/sources.list
fi
"${sudo[@]}" timeout --kill-after=15 120 apt-get update
# Fails when no source index is configured after all.
timeout 30 apt-get source --print-uris base-files > /dev/null
