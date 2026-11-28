#!/usr/bin/env bash
# Idempotent developer setup for Patina IPTV on Linux and macOS.
# Checks system packages, Rust + wasm32 target, the pinned sibling NSH checkout
# and nsh-cli, the SHA-256-verified guarded libmpv runtime, bundled Roboto
# fonts, target/dev-env.sh, and a Cargo smoke check.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: tools/bootstrap.sh [options]

Prepare a Linux or macOS checkout (or git worktree) to build and test Patina.

Options:
  --install-packages  Install missing OS packages (runs sudo on Linux if non-root)
  --skip-nsh          Skip cloning/updating ../nsh-patina and building nsh-cli
  --skip-runtime      Skip fetching the pinned guarded libmpv runtime
  --skip-smoke        Skip the final cargo check / core unit-test smoke run
  --check-only        Report prerequisite status without modifying anything
  --allow-minimal-toolchain
                      Allow missing clippy/rustfmt/wasm32 target in --check-only (native-only CI)
  --direnv            Also write .envrc sourcing target/dev-env.sh
  -h, --help          Show this help text
EOF
}

install_packages=false
skip_nsh=false
skip_runtime=false
skip_smoke=false
check_only=false
allow_minimal_toolchain=false
write_direnv=false

while [ $# -gt 0 ]; do
  case "$1" in
    --install-packages) install_packages=true ;;
    --skip-nsh) skip_nsh=true ;;
    --skip-runtime) skip_runtime=true ;;
    --skip-smoke) skip_smoke=true ;;
    --check-only) check_only=true ;;
    --allow-minimal-toolchain) allow_minimal_toolchain=true ;;
    --direnv) write_direnv=true ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

if [ "$check_only" = true ] && { [ "$install_packages" = true ] || [ "$write_direnv" = true ]; }; then
  echo "error: --check-only cannot be combined with --install-packages or --direnv" >&2
  exit 2
fi

# Keep helper Python imports and git status probes free of side-effect writes.
export PYTHONDONTWRITEBYTECODE=1
export GIT_OPTIONAL_LOCKS=0

caller_pwd=$(pwd)
resolve_caller_path() {
  local p
  case "$1" in
    /*) p="$1" ;;
    *) p="$caller_pwd/$1" ;;
  esac
  while [ "$p" != "/" ] && [ "${p%/}" != "$p" ]; do
    p="${p%/}"
  done
  printf '%s\n' "$p"
}

root=$(cd "$(dirname "$0")/.." && pwd)
default_nsh_dir="$(cd "$root/.." && pwd)/nsh-patina"
if [ -n "${PATINA_NSH_DIR:-}" ]; then
  nsh_dir=$(resolve_caller_path "$PATINA_NSH_DIR")
else
  nsh_dir="$default_nsh_dir"
fi
if [ "$nsh_dir" = "/" ]; then
  echo "error: NSH directory path must not be '/'" >&2
  exit 2
fi

case "${PATINA_HOST_OS:-$(uname -s)}" in
  Linux) platform=linux ;;
  Darwin) platform=macos ;;
  *)
    echo "error: tools/bootstrap.sh supports Linux and macOS; run ./tools/bootstrap.ps1 on Windows." >&2
    exit 2
    ;;
esac

if [ -n "${PATINA_MPV_RUNTIME_DIR:-}" ]; then
  runtime_dir=$(resolve_caller_path "$PATINA_MPV_RUNTIME_DIR")
else
  runtime_dir="$root/target/mpv-runtime-$platform"
fi

case "$runtime_dir" in
  /)
    echo "error: runtime directory path must not be '/'" >&2
    exit 2
    ;;
  *:*)
    echo "error: runtime directory path must not contain ':': $runtime_dir" >&2
    exit 2
    ;;
  *$'\n'*)
    echo "error: runtime directory path must not contain newlines" >&2
    exit 2
    ;;
esac

canonical_parent_and_name() {
  local dir parent base
  dir="$1"
  parent=$(dirname "$dir")
  base=$(basename "$dir")
  if [ -d "$parent" ]; then
    printf '%s/%s\n' "$(cd "$parent" && pwd -P)" "$base"
  else
    printf '%s\n' "$dir"
  fi
}

if [ "$check_only" = false ] && [ "$skip_smoke" = false ] && [ "$skip_nsh" = false ]; then
  expected_nsh_target=$(canonical_parent_and_name "$default_nsh_dir")
  requested_nsh_target=$(canonical_parent_and_name "$nsh_dir")
  if [ -d "$default_nsh_dir" ] && [ -d "$nsh_dir" ]; then
    expected_nsh_target=$(cd "$default_nsh_dir" && pwd -P)
    requested_nsh_target=$(cd "$nsh_dir" && pwd -P)
  fi
  if [ "$requested_nsh_target" != "$expected_nsh_target" ]; then
    echo "error: PATINA_NSH_DIR ($nsh_dir) differs from ../nsh-patina ($default_nsh_dir), which Cargo.toml resolves; pass --skip-smoke when testing an isolated NSH directory." >&2
    exit 1
  fi
fi

cd "$root"

failures=0
record_missing() {
  echo "  [missing] $1" >&2
  if [ $# -ge 2 ] && [ -n "$2" ]; then
    echo "            $2" >&2
  fi
  failures=$((failures + 1))
}

sudo_prefix() {
  if [ "${PATINA_EFFECTIVE_UID:-$(id -u)}" -eq 0 ]; then
    printf ''
  else
    printf 'sudo '
  fi
}

run_root() {
  if [ "${PATINA_EFFECTIVE_UID:-$(id -u)}" -eq 0 ]; then
    "$@"
  else
    if ! command -v sudo >/dev/null 2>&1; then
      echo "error: sudo is not available to install system packages; run the package command as root." >&2
      exit 1
    fi
    sudo "$@"
  fi
}

read_apt_packages() {
  local pkg_file="$root/tools/ci/linux-packages.txt"
  if [ ! -f "$pkg_file" ]; then
    echo "error: missing $pkg_file" >&2
    exit 1
  fi
  sed 's/#.*//' "$pkg_file" | tr -s '[:space:]' '\n' | sed '/^$/d'
}

python_ok=false
probe_python311() {
  command -v python3 >/dev/null 2>&1 && \
    python3 -B -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 11) else 1)' >/dev/null 2>&1
}

check_base_commands() {
  local cmd
  for cmd in git curl; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
      record_missing "$cmd is not on PATH" "Install $cmd with your OS package manager first."
    fi
  done
  if probe_python311; then
    python_ok=true
  elif [ "$platform" = linux ]; then
    if ! command -v python3 >/dev/null 2>&1; then
      record_missing "python3 (>= 3.11) is not on PATH" "Install python3 >= 3.11 with your OS package manager."
    else
      record_missing "python3 >= 3.11 is required" "Found $(python3 --version 2>&1)"
    fi
  elif [ "$install_packages" = true ] && [ "$check_only" = false ]; then
    record_missing "python3 >= 3.11 is still not first on PATH after brew install" \
      "Ensure Homebrew's bin directory precedes /usr/bin on PATH."
  fi
}

append_missing_base_linux_pkgs() {
  local mgr="$1"
  command -v git >/dev/null 2>&1 || missing+=(git)
  if ! command -v curl >/dev/null 2>&1; then
    if [ "$mgr" = apt ]; then
      missing+=(curl ca-certificates)
    else
      missing+=(curl)
    fi
  fi
  if ! probe_python311; then
    if [ "$mgr" = pacman ]; then
      missing+=(python)
    else
      missing+=(python3)
    fi
  fi
}

check_linux_packages() {
  local manager="${PATINA_PKG_MANAGER:-}"
  if [ -z "$manager" ]; then
    if command -v apt-get >/dev/null 2>&1 && command -v dpkg-query >/dev/null 2>&1; then
      manager=apt
    elif command -v dnf >/dev/null 2>&1 && command -v rpm >/dev/null 2>&1; then
      manager=dnf
    elif command -v pacman >/dev/null 2>&1; then
      manager=pacman
    else
      manager=generic
    fi
  fi

  local missing=()
  local pkg prefix cmd
  prefix=$(sudo_prefix)

  case "$manager" in
    apt)
      local apt_pkgs=()
      local pkg_list deb_arch status_lines
      pkg_list=$(read_apt_packages)
      while IFS= read -r pkg; do
        [ -n "$pkg" ] && apt_pkgs+=("$pkg")
      done <<EOF
$pkg_list
EOF
      if [ "${#apt_pkgs[@]}" -eq 0 ]; then
        echo "error: tools/ci/linux-packages.txt listed no packages" >&2
        exit 1
      fi
      deb_arch=$(dpkg --print-architecture 2>/dev/null || echo amd64)
      for pkg in "${apt_pkgs[@]}"; do
        # shellcheck disable=SC2016
        status_lines=$(dpkg-query -W -f='${Architecture}:${db:Status-Status}\n' "$pkg" 2>/dev/null || true)
        if ! printf '%s\n' "$status_lines" | grep -Fqx "${deb_arch}:installed" && \
           ! printf '%s\n' "$status_lines" | grep -Fqx "all:installed"; then
          missing+=("$pkg")
        fi
      done
      if [ "$install_packages" = true ] && [ "$check_only" = false ]; then
        append_missing_base_linux_pkgs apt
      fi
      if [ "${#missing[@]}" -eq 0 ]; then
        echo "  [ok] All ${#apt_pkgs[@]} Debian/Ubuntu packages from tools/ci/linux-packages.txt are installed"
        return 0
      fi
      cmd="${prefix}apt-get update && ${prefix}apt-get install -y ${missing[*]}"
      if [ "$install_packages" = true ] && [ "$check_only" = false ]; then
        echo "  [install] Installing missing apt packages: ${missing[*]}"
        run_root env DEBIAN_FRONTEND=noninteractive apt-get update
        run_root env DEBIAN_FRONTEND=noninteractive apt-get install -y "${missing[@]}"
        echo "  [ok] Installed Debian/Ubuntu packages"
        return 0
      fi
      record_missing "Missing Debian/Ubuntu packages: ${missing[*]}" "Run: $cmd"
      ;;
    dnf)
      local dnf_pkgs=(
        clang cmake dbus-daemon dbus-devel gcc gstreamer1-devel
        gstreamer1-plugins-base-devel gtk4-devel libva mpv-libs-devel
        ninja-build openssl-devel pipewire-libs pkgconf-pkg-config
      )
      for pkg in "${dnf_pkgs[@]}"; do
        if ! rpm -q "$pkg" >/dev/null 2>&1; then
          missing+=("$pkg")
        fi
      done
      if ! command -v ffmpeg >/dev/null 2>&1 && ! rpm -q ffmpeg-free >/dev/null 2>&1 && ! rpm -q ffmpeg >/dev/null 2>&1; then
        missing+=(ffmpeg-free)
      fi
      if [ "$install_packages" = true ] && [ "$check_only" = false ]; then
        append_missing_base_linux_pkgs dnf
      fi
      if [ "${#missing[@]}" -eq 0 ]; then
        echo "  [ok] All Fedora/RHEL host packages are installed"
        return 0
      fi
      cmd="${prefix}dnf install -y ${missing[*]}"
      if [ "$install_packages" = true ] && [ "$check_only" = false ]; then
        echo "  [install] Installing missing dnf packages: ${missing[*]}"
        run_root dnf install -y "${missing[@]}"
        echo "  [ok] Installed Fedora/RHEL packages"
        return 0
      fi
      record_missing "Missing Fedora/RHEL packages: ${missing[*]}" "Run: $cmd"
      ;;
    pacman)
      local pacman_pkgs=(
        clang cmake dbus ffmpeg gcc gst-plugins-base gstreamer
        gtk4 libva mpv ninja openssl pipewire pkgconf
      )
      for pkg in "${pacman_pkgs[@]}"; do
        if ! pacman -Qi "$pkg" >/dev/null 2>&1; then
          missing+=("$pkg")
        fi
      done
      if [ "$install_packages" = true ] && [ "$check_only" = false ]; then
        append_missing_base_linux_pkgs pacman
      fi
      if [ "${#missing[@]}" -eq 0 ]; then
        echo "  [ok] All Arch Linux host packages are installed"
        return 0
      fi
      cmd="${prefix}pacman -S --needed ${missing[*]}"
      if [ "$install_packages" = true ] && [ "$check_only" = false ]; then
        echo "  [install] Installing missing pacman packages: ${missing[*]}"
        run_root pacman -S --needed --noconfirm "${missing[@]}"
        echo "  [ok] Installed Arch Linux packages"
        return 0
      fi
      record_missing "Missing Arch Linux packages: ${missing[*]}" "Run: $cmd"
      ;;
    *)
      for pkg in pkg-config clang cmake ffmpeg; do
        command -v "$pkg" >/dev/null 2>&1 || missing+=("$pkg")
      done
      if ! command -v ninja >/dev/null 2>&1 && ! command -v ninja-build >/dev/null 2>&1; then
        missing+=(ninja)
      fi
      if command -v pkg-config >/dev/null 2>&1; then
        for pkg in gtk4 mpv openssl dbus-1 gstreamer-1.0 gstreamer-base-1.0; do
          pkg-config --exists "$pkg" 2>/dev/null || missing+=("pkg-config($pkg)")
        done
      fi
      if [ "${#missing[@]}" -eq 0 ]; then
        echo "  [ok] Host build tools and pkg-config libraries are available"
        return 0
      fi
      if [ "$install_packages" = true ]; then
        echo "error: unsupported Linux package manager for --install-packages; install equivalents of tools/ci/linux-packages.txt manually." >&2
        exit 1
      fi
      record_missing "Missing Linux tools/libraries: ${missing[*]}" "Install your distro's equivalents of tools/ci/linux-packages.txt."
      ;;
  esac
}

check_macos_packages() {
  if ! xcode-select -p >/dev/null 2>&1; then
    record_missing "Xcode Command Line Tools are not installed" "Run: xcode-select --install"
  else
    echo "  [ok] Xcode Command Line Tools found"
  fi

  local missing=()
  if [ "$install_packages" = true ] && [ "$check_only" = false ]; then
    command -v git >/dev/null 2>&1 || missing+=(git)
    command -v curl >/dev/null 2>&1 || missing+=(curl)
  fi
  if [ "$python_ok" = false ]; then
    missing+=(python)
  fi
  command -v pkg-config >/dev/null 2>&1 || missing+=(pkgconf)
  command -v ffmpeg >/dev/null 2>&1 || missing+=(ffmpeg)

  if [ "${#missing[@]}" -eq 0 ]; then
    echo "  [ok] Homebrew prerequisites (python >= 3.11, pkg-config, ffmpeg) are installed"
    return 0
  fi

  if ! command -v brew >/dev/null 2>&1; then
    record_missing "Missing macOS tools (${missing[*]}) and Homebrew is not on PATH" \
      "Install Homebrew (https://brew.sh) then run: brew install ${missing[*]}"
    return 0
  fi

  if [ "$install_packages" = true ] && [ "$check_only" = false ]; then
    echo "  [install] Installing missing Homebrew packages: ${missing[*]}"
    brew install "${missing[@]}"
    local brew_prefix
    brew_prefix=$(brew --prefix 2>/dev/null || true)
    if [ -n "$brew_prefix" ] && [ -d "$brew_prefix/bin" ]; then
      export PATH="$brew_prefix/bin:$PATH"
    fi
    echo "  [ok] Installed Homebrew packages"
    return 0
  fi
  record_missing "Missing Homebrew packages: ${missing[*]}" "Run: brew install ${missing[*]}"
}

echo "==> Checking prerequisites ($platform)"
if [ "$install_packages" = true ] && [ "$check_only" = false ]; then
  if probe_python311; then
    python_ok=true
  fi
  if [ "$platform" = linux ]; then
    check_linux_packages
  else
    check_macos_packages
  fi
  check_base_commands
else
  check_base_commands
  if [ "$platform" = linux ]; then
    check_linux_packages
  else
    check_macos_packages
  fi
fi

if [ "$check_only" = false ] && [ "$failures" -gt 0 ]; then
  echo "error: missing prerequisites above (re-run with --install-packages where supported)." >&2
  exit 1
fi

nsh_rev=""
toolchain=""
if [ "$python_ok" = true ]; then
  pin_output=$(python3 -B "$root/tools/ci/pins.py")
  while IFS='=' read -r key value; do
    case "$key" in
      nsh) nsh_rev="$value" ;;
      toolchain) toolchain="$value" ;;
    esac
  done <<EOF
$pin_output
EOF
else
  # Fallback for --check-only when Python < 3.11 (e.g. stock macOS Python 3.9).
  if [ -f "$root/nsh.rev" ]; then
    nsh_rev=$(tr -d '[:space:]' < "$root/nsh.rev")
  fi
  if [ -f "$root/rust-toolchain.toml" ]; then
    toolchain=$(sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$root/rust-toolchain.toml" | head -n 1)
  fi
fi

if [ -z "$nsh_rev" ] || [ -z "$toolchain" ]; then
  echo "error: could not read nsh or toolchain pin" >&2
  exit 1
fi

echo "==> Checking Rust toolchain ($toolchain)"
if [ -n "${HOME:-}" ] && [ -x "$HOME/.cargo/bin/rustup" ] && ! command -v rustup >/dev/null 2>&1; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

verify_standalone_rust() {
  local rustc_ver wasm_libdir wasm_ready rlib
  local missing_components=()
  rustc_ver=$(rustc --version 2>/dev/null || true)
  case "$rustc_ver" in
    "rustc $toolchain "|"rustc $toolchain ("*|"rustc $toolchain")
      ;;
    *)
      record_missing "Standalone Rust compiler (${rustc_ver:-rustc not found}) does not match pinned toolchain $toolchain" \
        "Install rustup from https://rustup.rs or Rust $toolchain."
      return 1
      ;;
  esac

  command -v cargo-clippy >/dev/null 2>&1 || missing_components+=(clippy)
  command -v rustfmt >/dev/null 2>&1 || missing_components+=(rustfmt)
  if [ "${#missing_components[@]}" -gt 0 ]; then
    if [ "$check_only" = true ] && [ "$allow_minimal_toolchain" = true ]; then
      echo "  [note] Optional minimal-toolchain components (${missing_components[*]}) are not installed"
    else
      record_missing "Rust components (${missing_components[*]}) are missing for standalone Rust $toolchain" \
        "Install rustup or the ${missing_components[*]} tools for Rust $toolchain."
    fi
  fi

  wasm_libdir=$(rustc --print target-libdir --target wasm32-unknown-unknown 2>/dev/null || true)
  wasm_ready=false
  if [ -n "$wasm_libdir" ] && [ -d "$wasm_libdir" ]; then
    for rlib in "$wasm_libdir"/libcore*.rlib; do
      if [ -f "$rlib" ]; then
        wasm_ready=true
        break
      fi
    done
  fi
  if [ "$wasm_ready" = true ]; then
    echo "  [ok] Standalone Rust $toolchain and wasm32-unknown-unknown target verified"
  elif [ "$check_only" = true ] && [ "$allow_minimal_toolchain" = true ]; then
    echo "  [ok] Standalone Rust $toolchain verified"
    echo "  [note] Optional minimal-toolchain target wasm32-unknown-unknown is not installed"
  else
    record_missing "Rust target wasm32-unknown-unknown is missing for standalone Rust $toolchain" \
      "Install rustup or add the wasm32-unknown-unknown target sysroot for Rust $toolchain."
  fi
}

if command -v rustup >/dev/null 2>&1; then
  toolchain_ready=false
  if RUSTUP_AUTO_INSTALL=0 rustup which --toolchain "$toolchain" cargo >/dev/null 2>&1; then
    toolchain_ready=true
    echo "  [ok] Rust $toolchain is installed"
  elif [ "$check_only" = true ]; then
    record_missing "Rust toolchain $toolchain is not installed" \
      "Run: rustup toolchain install $toolchain --component clippy,rustfmt"
  else
    echo "  [install] Installing Rust $toolchain"
    rustup toolchain install "$toolchain" --component clippy,rustfmt
    toolchain_ready=true
  fi

  if [ "$toolchain_ready" = true ]; then
    missing_components=()
    RUSTUP_AUTO_INSTALL=0 rustup which --toolchain "$toolchain" cargo-clippy >/dev/null 2>&1 || missing_components+=(clippy)
    RUSTUP_AUTO_INSTALL=0 rustup which --toolchain "$toolchain" rustfmt >/dev/null 2>&1 || missing_components+=(rustfmt)
    if [ "${#missing_components[@]}" -eq 0 ]; then
      echo "  [ok] Components clippy and rustfmt are installed for $toolchain"
    elif [ "$check_only" = true ] && [ "$allow_minimal_toolchain" = true ]; then
      echo "  [note] Optional minimal-toolchain components (${missing_components[*]}) are not installed"
    elif [ "$check_only" = true ]; then
      record_missing "Rust components (${missing_components[*]}) are missing for $toolchain" \
        "Run: rustup component add --toolchain $toolchain ${missing_components[*]}"
    else
      echo "  [install] Adding Rust components (${missing_components[*]}) to $toolchain"
      rustup component add --toolchain "$toolchain" "${missing_components[@]}"
    fi

    if RUSTUP_AUTO_INSTALL=0 rustup target list --installed --toolchain "$toolchain" 2>/dev/null | grep -qx 'wasm32-unknown-unknown'; then
      echo "  [ok] Target wasm32-unknown-unknown is installed for $toolchain"
    elif [ "$check_only" = true ] && [ "$allow_minimal_toolchain" = true ]; then
      echo "  [note] Optional minimal-toolchain target wasm32-unknown-unknown is not installed"
    elif [ "$check_only" = true ]; then
      record_missing "Rust target wasm32-unknown-unknown is missing for $toolchain" \
        "Run: rustup target add --toolchain $toolchain wasm32-unknown-unknown"
    else
      echo "  [install] Adding target wasm32-unknown-unknown to $toolchain"
      rustup target add --toolchain "$toolchain" wasm32-unknown-unknown
    fi
  fi
elif command -v cargo >/dev/null 2>&1; then
  echo "  [note] rustup not found; verifying standalone $(command -v cargo)"
  verify_standalone_rust || true
  if [ "$check_only" = false ] && [ "$failures" -gt 0 ]; then
    echo "error: standalone Rust installation does not satisfy pinned toolchain $toolchain." >&2
    exit 1
  fi
else
  record_missing "Neither rustup nor cargo is on PATH" \
    "Install rustup from https://rustup.rs and re-run tools/bootstrap.sh"
  if [ "$check_only" = false ]; then
    exit 1
  fi
fi

nsh_bin="$nsh_dir/target/release/nsh"
nsh_stamp="$nsh_dir/target/release/.patina-nsh-cli.rev"

resolve_nsh_git_root() {
  if [ ! -d "$nsh_dir" ] || [ ! -e "$nsh_dir/.git" ]; then
    return 1
  fi
  (cd "$nsh_dir" && pwd -P)
}

check_nsh_pin() {
  local real_nsh ceiling
  if ! real_nsh=$(resolve_nsh_git_root); then
    echo "$nsh_dir is not a Git checkout, but nsh.rev pins $nsh_rev."
    return 1
  fi
  ceiling=$(dirname "$real_nsh")
  if [ "$python_ok" = true ]; then
    GIT_CEILING_DIRECTORIES="$ceiling" python3 -B "$root/tools/ci/pins.py" check-nsh "$real_nsh"
    return $?
  fi
  local actual dirty
  actual=$(GIT_CEILING_DIRECTORIES="$ceiling" git -C "$real_nsh" rev-parse HEAD 2>/dev/null || true)
  if [ "$actual" != "$nsh_rev" ] && [ "${IPTV_ALLOW_NSH_DRIFT:-0}" != 1 ]; then
    echo "$nsh_dir is at ${actual:-unknown}, but nsh.rev pins $nsh_rev."
    return 1
  fi
  dirty=$(GIT_CEILING_DIRECTORIES="$ceiling" git -C "$real_nsh" status --porcelain 2>/dev/null || echo "status-error")
  if [ -n "$dirty" ] && [ "${IPTV_ALLOW_NSH_DRIFT:-0}" != 1 ]; then
    echo "$nsh_dir has uncommitted changes."
    return 1
  fi
  return 0
}

if [ "$skip_nsh" = true ]; then
  echo "==> Skipping NSH checkout and nsh-cli (--skip-nsh)"
else
  echo "==> Checking pinned NSH checkout ($nsh_dir @ ${nsh_rev:0:12})"
  if [ "$check_only" = true ]; then
    if nsh_err=$(check_nsh_pin 2>&1); then
      if [ -n "$nsh_err" ]; then
        printf '%s\n' "$nsh_err" >&2
      fi
      echo "  [ok] NSH checkout matches nsh.rev"
    else
      record_missing "NSH checkout is missing, drifted, or dirty" "$nsh_err"
    fi
    desired_stamp="$nsh_rev:$toolchain"
    if [ ! -x "$nsh_bin" ]; then
      record_missing "nsh-cli binary is not built at $nsh_bin" \
        "Run: cargo build --release --locked --manifest-path \"$nsh_dir/Cargo.toml\" --target-dir \"$nsh_dir/target\" -p nsh-cli"
    elif [ "${IPTV_ALLOW_NSH_DRIFT:-0}" != 1 ] && { [ ! -f "$nsh_stamp" ] || [ "$(cat "$nsh_stamp")" != "$desired_stamp" ]; }; then
      record_missing "nsh-cli binary at $nsh_bin is missing a clean stamp for ${nsh_rev:0:12} ($toolchain)" \
        "Re-run tools/bootstrap.sh to verify and stamp nsh-cli."
    else
      echo "  [ok] nsh-cli binary found at $nsh_bin"
    fi
  else
    just_cloned=false
    if [ ! -e "$nsh_dir" ] && [ ! -L "$nsh_dir" ]; then
      mkdir -p "$(dirname "$nsh_dir")"
      clone_lock="$(cd "$(dirname "$nsh_dir")" && pwd -P)/.$(basename "$nsh_dir").patina-clone.lock"
      exec 8>"$clone_lock"
      python3 -B -c 'import fcntl; fcntl.flock(8, fcntl.LOCK_EX)'
      if [ ! -e "$nsh_dir" ] && [ ! -L "$nsh_dir" ]; then
        echo "  [clone] Cloning teng-lin/nsh (branch patina) into $nsh_dir"
        git clone --branch patina https://github.com/teng-lin/nsh.git "$nsh_dir" 8>&-
        just_cloned=true
      fi
      exec 8>&-
    fi
    if ! real_nsh=$(resolve_nsh_git_root); then
      echo "error: $nsh_dir exists and is not a Git repository" >&2
      exit 1
    fi
    nsh_ceiling=$(dirname "$real_nsh")
    nsh_git_common=$(GIT_CEILING_DIRECTORIES="$nsh_ceiling" python3 -B - "$real_nsh" <<'PY'
import os
import subprocess
import sys

real_nsh = sys.argv[1]
common = subprocess.check_output(["git", "-C", real_nsh, "rev-parse", "--git-common-dir"], text=True).strip()
print(os.path.realpath(os.path.join(real_nsh, common)))
PY
    )
    nsh_lock_file="$nsh_git_common/patina-bootstrap.lock"
    exec 9>"$nsh_lock_file"
    python3 -B -c 'import fcntl; fcntl.flock(9, fcntl.LOCK_EX)'

    if [ "$just_cloned" = true ]; then
      GIT_CEILING_DIRECTORIES="$nsh_ceiling" git -C "$real_nsh" checkout --detach "$nsh_rev" 9>&-
    else
      dirty=$(GIT_CEILING_DIRECTORIES="$nsh_ceiling" git -C "$real_nsh" status --porcelain 9>&-)
      current_rev=$(GIT_CEILING_DIRECTORIES="$nsh_ceiling" git -C "$real_nsh" rev-parse HEAD 9>&-)
      if [ -n "$dirty" ]; then
        if [ "${IPTV_ALLOW_NSH_DRIFT:-0}" = 1 ]; then
          echo "  [warn] IPTV_ALLOW_NSH_DRIFT=1: leaving dirty NSH checkout untouched at $current_rev" >&2
        else
          GIT_CEILING_DIRECTORIES="$nsh_ceiling" python3 -B "$root/tools/ci/pins.py" check-nsh "$real_nsh" 9>&-
          exec 9>&-
          exit 1
        fi
      elif [ "$current_rev" != "$nsh_rev" ]; then
        if [ "${IPTV_ALLOW_NSH_DRIFT:-0}" = 1 ]; then
          echo "  [warn] IPTV_ALLOW_NSH_DRIFT=1: leaving NSH checkout at $current_rev (nsh.rev pins $nsh_rev)" >&2
        else
          echo "  [update] Checking out pinned NSH commit $nsh_rev"
          if ! GIT_CEILING_DIRECTORIES="$nsh_ceiling" git -C "$real_nsh" cat-file -e "${nsh_rev}^{commit}" 2>/dev/null 9>&-; then
            GIT_CEILING_DIRECTORIES="$nsh_ceiling" git -C "$real_nsh" fetch origin "$nsh_rev" 9>&-
          fi
          GIT_CEILING_DIRECTORIES="$nsh_ceiling" git -C "$real_nsh" checkout --detach "$nsh_rev" 9>&-
        fi
      else
        echo "  [ok] NSH checkout already clean at $nsh_rev"
      fi
    fi

    GIT_CEILING_DIRECTORIES="$nsh_ceiling" python3 -B "$root/tools/ci/pins.py" check-nsh "$real_nsh" 9>&-
    actual_nsh_rev=$(GIT_CEILING_DIRECTORIES="$nsh_ceiling" git -C "$real_nsh" rev-parse HEAD 9>&-)
    desired_stamp="$actual_nsh_rev:$toolchain"
    nsh_target_dir="$real_nsh/target"
    nsh_bin="$nsh_target_dir/release/nsh"
    nsh_stamp="$nsh_target_dir/release/.patina-nsh-cli.rev"
    if [ "${IPTV_ALLOW_NSH_DRIFT:-0}" != 1 ] && [ -x "$nsh_bin" ] && [ -f "$nsh_stamp" ] && [ "$(cat "$nsh_stamp")" = "$desired_stamp" ]; then
      echo "  [ok] nsh-cli already built for ${actual_nsh_rev:0:12}"
    else
      if [ "${IPTV_ALLOW_NSH_DRIFT:-0}" = 1 ]; then
        rm -f "$nsh_stamp"
      fi
      echo "  [build] Building nsh-cli (--release --locked)"
      if command -v rustup >/dev/null 2>&1; then
        RUSTUP_TOOLCHAIN="$toolchain" cargo build --release --locked --manifest-path "$real_nsh/Cargo.toml" --target-dir "$nsh_target_dir" -p nsh-cli 9>&-
      else
        cargo build --release --locked --manifest-path "$real_nsh/Cargo.toml" --target-dir "$nsh_target_dir" -p nsh-cli 9>&-
      fi
      if [ ! -x "$nsh_bin" ]; then
        echo "error: expected nsh-cli binary at $nsh_bin after build" >&2
        exec 9>&-
        exit 1
      fi
      if [ "${IPTV_ALLOW_NSH_DRIFT:-0}" != 1 ]; then
        tmp_stamp=$(mktemp "${nsh_stamp}.tmp.XXXXXX")
        printf '%s\n' "$desired_stamp" > "$tmp_stamp"
        mv "$tmp_stamp" "$nsh_stamp"
      fi
      echo "  [ok] Built $nsh_bin"
    fi
    exec 9>&-
  fi
fi

verify_existing_runtime() {
  python3 -B - "$root" "$platform" "$runtime_dir" <<'PY'
from pathlib import Path
import importlib.util
import json
import sys

root = Path(sys.argv[1])
platform = sys.argv[2]
dest = Path(sys.argv[3])
try:
    if dest.is_symlink():
        raise SystemExit(1)
    spec = importlib.util.spec_from_file_location("mpv_runtime_unix", root / "tools/mpv-runtime-unix/runtime.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    expected = mod.read_manifest(platform, root / "tools/mpv-runtime-unix" / mod.names(platform)["manifest"])
    installed_manifest = dest / "runtime.json"
    if not installed_manifest.is_file():
        raise SystemExit(1)
    if json.loads(installed_manifest.read_text(encoding="utf-8")) != expected:
        raise SystemExit(1)
    library = dest / "lib" / expected["library"]
    link = dest / "lib" / mod.PLATFORMS[platform]["link"]
    pkg = dest / "lib/pkgconfig/mpv.pc"
    if not library.is_file() or not link.is_symlink() or link.resolve() != library.resolve() or not pkg.is_file():
        raise SystemExit(1)
    if mod.prepare.digest(library) != expected["library_sha256"]:
        raise SystemExit(1)
except (OSError, UnicodeDecodeError, ValueError, KeyError):
    raise SystemExit(1)
PY
}

check_runtime_arch() {
  local arch="${PATINA_HOST_ARCH:-$(uname -m)}"
  case "$platform:$arch" in
    linux:x86_64|macos:arm64) return 0 ;;
    *)
      record_missing "Prebuilt guarded libmpv runtime targets linux/x86_64 and macos/arm64 (found $platform/$arch)" \
        "Pass --skip-runtime when working only on portable crates."
      return 1
      ;;
  esac
}

can_replace_runtime_dir() {
  python3 -B - "$root" "$platform" "$runtime_dir" <<'PY'
from pathlib import Path
import importlib.util
import sys

ALLOWED_ENTRIES = {
    "lib",
    "include",
    "licenses",
    "build-config",
    "NOTICE.txt",
    "SOURCE.txt",
    "components.json",
    "sources.lock.json",
    "runtime.json",
}

root = Path(sys.argv[1])
platform = sys.argv[2]
dest = Path(sys.argv[3])
try:
    if dest.is_symlink():
        raise SystemExit(1)
    if not dest.exists():
        raise SystemExit(0)
    if not dest.is_dir():
        raise SystemExit(1)
    entries = {entry.name for entry in dest.iterdir()}
    if not entries:
        raise SystemExit(0)
    if not entries <= ALLOWED_ENTRIES:
        raise SystemExit(1)
    manifest_file = dest / "runtime.json"
    if not manifest_file.is_file() or not (dest / "lib/pkgconfig/mpv.pc").is_file() or not (dest / "components.json").is_file():
        raise SystemExit(1)
    spec = importlib.util.spec_from_file_location("mpv_runtime_unix", root / "tools/mpv-runtime-unix/runtime.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    mod.read_manifest(platform, manifest_file)
except (OSError, UnicodeDecodeError, ValueError, KeyError):
    raise SystemExit(1)
PY
}

install_staged_runtime() {
  local runtime_parent runtime_base canonical_parent canonical_dest staging_dir backup_dir archive
  runtime_parent=$(dirname "$runtime_dir")
  runtime_base=$(basename "$runtime_dir")
  mkdir -p "$runtime_parent"
  canonical_parent=$(cd "$runtime_parent" && pwd -P)
  canonical_dest="$canonical_parent/$runtime_base"
  staging_dir="$canonical_parent/.${runtime_base}.patina-staging"
  backup_dir="$canonical_parent/.${runtime_base}.patina-previous"
  if [ -L "$staging_dir" ] || [ -L "$backup_dir" ] || [ -L "$canonical_dest" ]; then
    echo "error: refusing symlinked runtime or staging path under $canonical_parent" >&2
    exit 1
  fi
  rm -rf "$staging_dir"
  if [ ! -e "$canonical_dest" ] && [ -e "$backup_dir" ]; then
    rm -rf "$backup_dir"
  fi
  archive=$(python3 -B "$root/tools/ci/native_candidates.py" "$platform")
  python3 -B "$root/tools/mpv-runtime-unix/runtime.py" fetch --platform "$platform" --archive "$archive" --destination "$staging_dir"
  python3 -B - "$staging_dir/lib/pkgconfig/mpv.pc" "$canonical_dest" <<'PY'
from pathlib import Path
import sys

pc_path = Path(sys.argv[1])
final_dest = sys.argv[2]
lines = pc_path.read_text(encoding="utf-8").splitlines()
rewritten = [f"prefix={final_dest}" if line.startswith("prefix=") else line for line in lines]
pc_path.write_text("\n".join(rewritten) + "\n", encoding="utf-8")
PY
  if [ -d "$canonical_dest" ]; then
    rmdir "$canonical_dest" 2>/dev/null || true
  fi
  if [ -d "$canonical_dest" ]; then
    rm -rf "$backup_dir"
    mv "$canonical_dest" "$backup_dir"
  fi
  mv "$staging_dir" "$canonical_dest"
  rm -rf "$backup_dir"
  if ! verify_existing_runtime; then
    echo "error: staged runtime verification failed after publishing to $runtime_dir" >&2
    exit 1
  fi
}

if [ "$skip_runtime" = true ]; then
  echo "==> Skipping guarded libmpv runtime (--skip-runtime)"
else
  echo "==> Checking guarded libmpv runtime ($runtime_dir)"
  if [ -L "$runtime_dir" ]; then
    if [ "$check_only" = true ]; then
      record_missing "Refusing symlinked runtime destination: $runtime_dir" \
        "Use a real directory or pass PATINA_MPV_RUNTIME_DIR=<real-dir>."
    else
      echo "error: refusing symlinked runtime destination: $runtime_dir (copy the directory or set PATINA_MPV_RUNTIME_DIR to the target path)" >&2
      exit 1
    fi
  elif check_runtime_arch; then
    if [ "$python_ok" = false ]; then
      record_missing "Cannot verify guarded libmpv runtime without python3 >= 3.11"
    elif verify_existing_runtime; then
      echo "  [ok] Verified SHA-256 of pinned guarded libmpv runtime in $runtime_dir"
    elif [ "$check_only" = true ]; then
      record_missing "Guarded libmpv runtime is missing or outdated in $runtime_dir" \
        "Run: archive=\$(python3 tools/ci/native_candidates.py $platform) && python3 tools/mpv-runtime-unix/runtime.py fetch --platform $platform --archive \"\$archive\" --destination \"$runtime_dir\""
    else
      if ! can_replace_runtime_dir; then
        echo "error: refusing to overwrite non-empty directory that is not a recognized Patina runtime: $runtime_dir" >&2
        exit 1
      fi
      echo "  [fetch] Fetching and verifying pinned guarded libmpv runtime for $platform"
      install_staged_runtime
      echo "  [ok] Unpacked verified runtime into $runtime_dir"
    fi
  elif [ "$check_only" = false ]; then
    exit 1
  fi
fi

echo "==> Checking bundled Roboto fonts"
missing_fonts=()
for font_file in Roboto-Regular.woff2 Roboto-Medium.woff2 RobotoCondensed-Regular.woff2 OFL.txt; do
  if [ ! -f "$root/crates/iptv-app/fonts/$font_file" ]; then
    missing_fonts+=("$font_file")
  fi
done
if [ "${#missing_fonts[@]}" -eq 0 ]; then
  echo "  [ok] Bundled Roboto fonts are present"
elif [ "$check_only" = true ]; then
  record_missing "Missing bundled font files: ${missing_fonts[*]}" \
    "Restore with 'git checkout -- crates/iptv-app/fonts' or run: bash tools/fetch-fonts.sh"
else
  missing_font_tools=()
  for tool in curl unzip sha256sum uvx; do
    command -v "$tool" >/dev/null 2>&1 || missing_font_tools+=("$tool")
  done
  if [ "${#missing_font_tools[@]}" -gt 0 ]; then
    echo "error: bundled font files (${missing_fonts[*]}) are missing and tools/fetch-fonts.sh requires: ${missing_font_tools[*]} (restore tracked files with 'git checkout -- crates/iptv-app/fonts')" >&2
    exit 1
  fi
  echo "  [fetch] Rebuilding bundled fonts (${missing_fonts[*]})"
  bash "$root/tools/fetch-fonts.sh"
fi

if [ "$check_only" = true ]; then
  if [ "$failures" -gt 0 ]; then
    echo "bootstrap check failed: $failures prerequisite issue(s) found." >&2
    exit 1
  fi
  echo "bootstrap check passed: all checked prerequisites are ready."
  exit 0
fi

write_result=""
write_if_changed() {
  local target_file="$1"
  local content="$2"
  if [ -L "$target_file" ] || [ -d "$target_file" ]; then
    echo "error: refusing to write through symlink or directory $target_file" >&2
    return 1
  fi
  mkdir -p "$(dirname "$target_file")"
  local tmp_file
  tmp_file=$(mktemp "${target_file}.tmp.XXXXXX")
  if ! printf '%s' "$content" > "$tmp_file"; then
    rm -f "$tmp_file"
    return 1
  fi
  if [ -f "$target_file" ] && cmp -s "$tmp_file" "$target_file"; then
    rm -f "$tmp_file"
    write_result=unchanged
    return 0
  fi
  if ! mv "$tmp_file" "$target_file"; then
    rm -f "$tmp_file"
    return 1
  fi
  write_result=updated
  return 0
}

env_file="$root/target/dev-env.sh"
if [ "$skip_runtime" = false ]; then
  echo "==> Writing $env_file"
  lib_dir="$runtime_dir/lib"
  lib_escaped=${lib_dir//\'/\'\\\'\'}
  env_content="# Generated by tools/bootstrap.sh. Source in shells that build or run iptv-app:
#   source target/dev-env.sh
_patina_prepend_lib() {
  _patina_rem=\"\${1:-}:\"
  _patina_out=\"\$IPTV_RS_MPV_LIB_DIR\"
  while [ -n \"\$_patina_rem\" ]; do
    _patina_seg=\"\${_patina_rem%%:*}\"
    _patina_rem=\"\${_patina_rem#*:}\"
    if [ -n \"\$_patina_seg\" ] && [ \"\$_patina_seg\" != \"\$IPTV_RS_MPV_LIB_DIR\" ]; then
      _patina_out=\"\$_patina_out:\$_patina_seg\"
    fi
  done
  unset _patina_rem _patina_seg
  printf '%s' \"\$_patina_out\"
}
export IPTV_RS_MPV_LIB_DIR='${lib_escaped}'
export LIBRARY_PATH=\"\$(_patina_prepend_lib \"\${LIBRARY_PATH:-}\")\"
export LD_LIBRARY_PATH=\"\$(_patina_prepend_lib \"\${LD_LIBRARY_PATH:-}\")\"
export DYLD_LIBRARY_PATH=\"\$(_patina_prepend_lib \"\${DYLD_LIBRARY_PATH:-}\")\"
unset _patina_out
unset -f _patina_prepend_lib
"
  write_if_changed "$env_file" "$env_content"
  if [ "$write_result" = updated ]; then
    echo "  [write] Updated $env_file"
  else
    echo "  [ok] $env_file already up to date"
  fi

  if [ "$write_direnv" = true ]; then
    envrc_file="$root/.envrc"
    direnv_line="source_env target/dev-env.sh"
    if [ -L "$envrc_file" ] || [ -d "$envrc_file" ]; then
      echo "error: refusing to write through symlink or directory $envrc_file" >&2
      exit 1
    fi
    if [ -f "$envrc_file" ] && grep -Fqx "$direnv_line" "$envrc_file"; then
      echo "  [ok] $envrc_file already sources target/dev-env.sh"
    elif [ -f "$envrc_file" ]; then
      printf '\n%s\n' "$direnv_line" >> "$envrc_file"
      echo "  [write] Appended '$direnv_line' to $envrc_file"
    else
      printf '%s\n' "$direnv_line" > "$envrc_file"
      echo "  [write] Created $envrc_file"
    fi
    if command -v direnv >/dev/null 2>&1; then
      echo "  [hint] Run 'direnv allow' to trust $envrc_file"
    fi
  fi
elif [ "$write_direnv" = true ]; then
  echo "  [note] Skipping target/dev-env.sh and .envrc because --skip-runtime was set"
fi

if [ "$skip_smoke" = true ]; then
  echo "==> Skipping Cargo smoke check (--skip-smoke)"
else
  resolved_nsh=""
  resolved_default_nsh=""
  if [ -d "$nsh_dir" ]; then
    resolved_nsh=$(cd "$nsh_dir" && pwd -P)
  fi
  if [ -d "$default_nsh_dir" ]; then
    resolved_default_nsh=$(cd "$default_nsh_dir" && pwd -P)
  fi
  if [ "$skip_nsh" = false ] && [ "$resolved_nsh" != "$resolved_default_nsh" ]; then
    echo "error: PATINA_NSH_DIR ($nsh_dir) differs from ../nsh-patina ($default_nsh_dir), which Cargo.toml resolves; pass --skip-smoke when testing an isolated NSH directory." >&2
    exit 1
  fi

  echo "==> Running Cargo smoke checks"
  if [ -f "$env_file" ]; then
    # shellcheck disable=SC1090
    source "$env_file"
  fi
  # shellcheck disable=SC1091
  source "$root/tools/ci/locked-build-env.sh"
  if [ "$skip_nsh" = false ] && [ "$skip_runtime" = false ]; then
    cargo check --locked -p iptv-app
    echo "  [ok] cargo check --locked -p iptv-app passed"
  else
    echo "  [note] Skipping cargo check -p iptv-app because --skip-nsh or --skip-runtime was set"
  fi
  if [ "$skip_nsh" = false ] || [ -d "$default_nsh_dir/crates/nsh-core" ]; then
    cargo test --locked -p iptv-core -p iptv-presentation
    echo "  [ok] cargo test --locked -p iptv-core -p iptv-presentation passed"
  else
    echo "  [note] Skipping workspace cargo test because $default_nsh_dir is absent (--skip-nsh)"
  fi
fi

echo ""
echo "Bootstrap complete."
if [ -f "$env_file" ]; then
  echo "Load the native runtime paths in your shell before running Cargo:"
  echo "  source target/dev-env.sh"
fi
