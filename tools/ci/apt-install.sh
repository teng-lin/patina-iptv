#!/usr/bin/env bash
# Install Linux build packages with bounded, retried apt calls (issue #159).
#
# usage: apt-install.sh STATE_DIR RESTORED PACKAGE...
#
# STATE_DIR holds apt's package lists and downloaded .debs so the setup action
# can cache them. RESTORED=true tries the restored snapshot offline,
# including on a newer image of the same Ubuntu release. Any missing package
# or version conflict falls through to the refreshed retry. Package
# downloads are bounded independently of dpkg. A failed mirror fetch is
# retried once using Canonical's archive when the runner uses a cloud mirror.
set -euo pipefail
if [ "$#" -lt 3 ]; then
  echo "usage: $0 STATE_DIR RESTORED PACKAGE..." >&2
  exit 2
fi
state=$1
restored=$2
shift 2
packages=("$@")
download_seconds=${PATINA_APT_DOWNLOAD_SECONDS:-240}
install_seconds=${PATINA_APT_INSTALL_SECONDS:-180}
refresh_seconds=${PATINA_APT_REFRESH_SECONDS:-90}
conf=${PATINA_APT_CONF:-/etc/apt/apt.conf.d/90patina-ci}
sources=${PATINA_APT_SOURCES:-/etc/apt}
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

switch_mirror() {
  local source
  # Keep the distro, pockets and Signed-By from the image's original entries.
  local mirror='https?://(azure|([a-z0-9-]+\.)?(ec2|gce))\.archive\.ubuntu\.com/ubuntu([[:space:]/]|$)'
  local mirrorlist='mirror\+file:/{1,3}etc/apt/apt-mirrors\.txt([[:space:]]|$)'
  for source in "$sources/sources.list" "$sources/sources.list.d/"*.list "$sources/sources.list.d/"*.sources; do
    if [ -f "$source" ] && grep -Eq "$mirror|$mirrorlist" "$source"; then
      as_root sed -i -E \
        -e "s@${mirror}@https://archive.ubuntu.com/ubuntu\\4@g" \
        -e "s@${mirrorlist}@https://archive.ubuntu.com/ubuntu\\1@g" "$source"
      echo "Using archive.ubuntu.com for $source"
    fi
  done
}

fetch_packages() {
  bounded "$refresh_seconds" apt-get update -q -o APT::Update::Error-Mode=any &&
    bounded "$download_seconds" apt-get install -q --download-only "${packages[@]}"
}

install_packages() {
  bounded "$install_seconds" apt-get install -q --no-download "$@" "${packages[@]}"
}

repair_packages() {
  bounded "$refresh_seconds" dpkg --configure -a || true
}

as_root mkdir -p "$state/lists/partial" "$state/archives/partial"
# Every later apt call in the job (workflow steps included) shares these
# lists, retries and timeouts. The HTTP timeouts cover connecting and stalled
# transfers; a mirror that trickles data is caught by the attempt bound.
# The state lives under the runner's home, which apt's _apt sandbox user
# cannot reach, so downloads run as root on this disposable machine.
as_root tee "$conf" > /dev/null <<EOF
Acquire::Retries "2";
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

refreshed=false
if [ "$restored" != true ] || ! install_packages; then
  refreshed=true
  if [ "$restored" = true ]; then
    echo "::warning::cached apt install failed; refreshing the package lists and downloads"
    repair_packages
  fi
  if ! fetch_packages; then
    echo "::warning::apt fetch failed or timed out; trying the Ubuntu archive"
    switch_mirror
    fetch_packages || { status=$?; echo "::error::apt downloads failed after mirror retry ($status)"; exit "$status"; }
  fi
  if ! install_packages; then
    echo "::warning::offline apt install failed; repairing dpkg and retrying"
    repair_packages
    install_packages --fix-broken
  fi
fi
# Leave no truncated downloads in the cache, and give its save step (run as
# the runner user) a tree it can read.
as_root find "$state/lists/partial" "$state/archives/partial" -mindepth 1 -delete
as_root chown -R "$(id -u):$(id -g)" "$state"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  echo "refreshed=$refreshed" >> "$GITHUB_OUTPUT"
fi
