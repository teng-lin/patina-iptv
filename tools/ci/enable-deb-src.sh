#!/usr/bin/env bash
# Add Ubuntu's source package indexes so `apt-get source` can fetch the exact
# sources of bundled libraries (tools/ci/native-sources.py). Run only on an
# ephemeral Ubuntu runner or container: it edits the system's apt sources.
set -euo pipefail
sudo=()
[ "$(id -u)" -eq 0 ] || sudo=(sudo)
sources=${PATINA_APT_SOURCES:-/etc/apt}
ubuntu_source="$sources/sources.list.d/ubuntu.sources"
apt=(apt-get)
if [ -f "$ubuntu_source" ]; then
  # Ubuntu 24.04 and later: one deb822 file. Refresh it alone, so an outage of
  # one of the runner image's vendor feeds cannot fail the collection.
  "${sudo[@]}" sed -i 's/^Types: deb$/Types: deb deb-src/' "$ubuntu_source"
  apt+=(-o "Dir::Etc::sourcelist=$ubuntu_source" -o Dir::Etc::sourceparts=-)
else
  "${sudo[@]}" sed -i -E 's/^# *(deb-src .*)$/\1/' "$sources/sources.list"
fi
"${sudo[@]}" timeout --kill-after=15 120 "${apt[@]}" update
# Fails when no source index is configured after all.
timeout 30 apt-get source --print-uris base-files > /dev/null
