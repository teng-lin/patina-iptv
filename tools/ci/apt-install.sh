#!/usr/bin/env bash
# Install Linux build packages with bounded, retried apt calls (issue #159).
#
# usage: apt-install.sh STATE_DIR RESTORED PACKAGE...
#
# STATE_DIR holds apt's package lists and downloaded .debs so the setup action
# can cache them; RESTORED=true means that cache was restored for this exact
# runner image and package list, so the mirror is not contacted at all unless
# the cached install fails. Every apt call is bounded by
# PATINA_APT_ATTEMPT_SECONDS; a stalled or failed attempt is retried once
# after refreshing the lists, so a slow mirror fails the step in about ten
# minutes instead of consuming the job's timeout.
set -euo pipefail
if [ "$#" -lt 3 ]; then
  echo "usage: $0 STATE_DIR RESTORED PACKAGE..." >&2
  exit 2
fi
state=$1
restored=$2
shift 2
packages=("$@")
attempt_seconds=${PATINA_APT_ATTEMPT_SECONDS:-300}
conf=${PATINA_APT_CONF:-/etc/apt/apt.conf.d/90patina-ci}
initramfs_conf=${PATINA_INITRAMFS_CONF:-/etc/initramfs-tools/update-initramfs.conf}
man_db_trigger=${PATINA_MAN_DB_TRIGGER:-/var/lib/man-db/auto-update}

sudo=()
if [ "$(id -u)" != 0 ]; then sudo=(sudo); fi
as_root() { "${sudo[@]}" env DEBIAN_FRONTEND=noninteractive "$@"; }
# timeout(1) runs as root so it can stop root's apt/dpkg on a stall.
bounded() { as_root timeout --kill-after=30 "$attempt_seconds" "$@"; }

as_root mkdir -p "$state/lists/partial" "$state/archives/partial"
# Every later apt call in the job (workflow steps included) shares these
# lists, retries and timeouts. The HTTP timeouts cover connecting and stalled
# transfers; a mirror that trickles data is caught by the attempt bound.
as_root tee "$conf" > /dev/null <<EOF
Acquire::Retries "3";
Acquire::http::Timeout "30";
Acquire::https::Timeout "30";
Acquire::GzipIndexes "true";
Acquire::CompressionTypes::Order:: "gz";
Dir::State::Lists "$state/lists";
Dir::Cache::Archives "$state/archives";
APT::Keep-Downloaded-Packages "true";
Binary::apt-get::APT::Keep-Downloaded-Packages "true";
APT::Get::Assume-Yes "true";
Dpkg::Use-Pty "0";
Dpkg::Options { "--force-confdef"; "--force-confold"; };
EOF
# Installing GTK upgrades udev/systemd on the image, whose triggers rebuild
# the initramfs and the man-db index. A runner never boots that initramfs or
# reads man pages, and #132 stalled in update-initramfs.
if [ -f "$initramfs_conf" ]; then
  as_root sed -i 's/^update_initramfs=.*/update_initramfs=no/' "$initramfs_conf"
fi
if command -v debconf-set-selections > /dev/null 2>&1; then
  echo 'man-db man-db/auto-update boolean false' | as_root debconf-set-selections
fi
as_root rm -f "$man_db_trigger"

install() {
  if [ "$1" = refresh ]; then
    bounded apt-get update -q || return 1
  fi
  bounded apt-get install -q "${packages[@]}"
}

first=refresh
if [ "$restored" = true ]; then first=cached; fi
if ! install "$first"; then
  echo "::warning::apt did not finish within ${attempt_seconds}s or failed; retrying once with refreshed lists"
  bounded dpkg --configure -a || true
  install refresh
fi
# The cache save step runs as the runner user; apt leaves root-only files.
as_root chown -R "$(id -u):$(id -g)" "$state"
