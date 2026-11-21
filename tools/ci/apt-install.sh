#!/usr/bin/env bash
# Install Linux build packages with bounded, retried apt calls (issue #159).
#
# usage: apt-install.sh STATE_DIR RESTORED PACKAGE...
#
# STATE_DIR holds apt's package lists and downloaded .debs so the setup action
# can cache them. RESTORED=true means that cache was restored for this exact
# runner image and package list: the install then never contacts the mirror,
# and any gap in the cache fails straight into the refreshed retry. Package
# installs are bounded by PATINA_APT_INSTALL_SECONDS and list refreshes and
# dpkg repairs by PATINA_APT_REFRESH_SECONDS; a stalled or failed attempt is
# retried once after refreshing the lists. A stalled mirror therefore fails
# the step within about twelve minutes instead of consuming the job's timeout.
set -euo pipefail
if [ "$#" -lt 3 ]; then
  echo "usage: $0 STATE_DIR RESTORED PACKAGE..." >&2
  exit 2
fi
state=$1
restored=$2
shift 2
packages=("$@")
install_seconds=${PATINA_APT_INSTALL_SECONDS:-240}
refresh_seconds=${PATINA_APT_REFRESH_SECONDS:-90}
conf=${PATINA_APT_CONF:-/etc/apt/apt.conf.d/90patina-ci}
initramfs_conf=${PATINA_INITRAMFS_CONF:-/etc/initramfs-tools/update-initramfs.conf}
man_db_trigger=${PATINA_MAN_DB_TRIGGER:-/var/lib/man-db/auto-update}

sudo=()
if [ "$(id -u)" != 0 ]; then sudo=(sudo); fi
as_root() { "${sudo[@]}" env DEBIAN_FRONTEND=noninteractive "$@"; }
# timeout(1) runs as root so it can stop root's apt/dpkg on a stall.
bounded() {
  local seconds=$1
  shift
  as_root timeout --kill-after=15 "$seconds" "$@"
}

as_root mkdir -p "$state/lists/partial" "$state/archives/partial"
# Every later apt call in the job (workflow steps included) shares these
# lists, retries and timeouts. The HTTP timeouts cover connecting and stalled
# transfers; a mirror that trickles data is caught by the attempt bound.
# The state lives under the runner's home, which apt's _apt sandbox user
# cannot reach, so downloads run as root on this disposable machine.
as_root tee "$conf" > /dev/null <<EOF
Acquire::Retries "3";
Acquire::http::Timeout "30";
Acquire::https::Timeout "30";
Acquire::GzipIndexes "true";
Acquire::CompressionTypes::Order:: "gz";
APT::Sandbox::User "root";
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

if [ "$restored" = true ] && bounded "$install_seconds" apt-get install -q --no-download "${packages[@]}"; then
  :
elif [ "$restored" != true ] && bounded "$refresh_seconds" apt-get update -q &&
    bounded "$install_seconds" apt-get install -q "${packages[@]}"; then
  :
else
  echo "::warning::apt failed or exceeded its time bound; retrying once with refreshed lists"
  # A killed install can leave packages unpacked but unconfigured.
  bounded "$refresh_seconds" dpkg --configure -a || true
  bounded "$refresh_seconds" apt-get update -q
  bounded "$install_seconds" apt-get install -q --fix-broken "${packages[@]}"
fi
# Leave no truncated downloads in the cache, and give its save step (run as
# the runner user) a tree it can read.
as_root find "$state/lists/partial" "$state/archives/partial" -mindepth 1 -delete
as_root chown -R "$(id -u):$(id -g)" "$state"
