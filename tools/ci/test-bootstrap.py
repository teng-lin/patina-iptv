#!/usr/bin/env python3
"""Verify Linux package-list parity, bootstrap flags, sudo safety and idempotence."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tempfile
import textwrap
import tomllib
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
BOOTSTRAP = ROOT / "tools/bootstrap.sh"
BOOTSTRAP_PS1 = ROOT / "tools/bootstrap.ps1"
PACKAGES_FILE = ROOT / "tools/ci/linux-packages.txt"
SETUP_ACTION = ROOT / ".github/actions/setup/action.yml"
README = ROOT / "README.md"

SCRUBBED_ENV_KEYS = {
    "IPTV_ALLOW_NSH_DRIFT",
    "PATINA_NSH_DIR",
    "PATINA_MPV_RUNTIME_DIR",
    "PATINA_HOST_OS",
    "PATINA_HOST_ARCH",
    "PATINA_PKG_MANAGER",
    "PATINA_EFFECTIVE_UID",
}


def read_linux_packages(path=PACKAGES_FILE):
    entries = []
    for raw in Path(path).read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if line:
            entries.extend(line.split())
    return entries


def read_readme_linux_packages(text):
    match = re.search(
        r"<!-- linux-packages:start -->(.*?)<!-- linux-packages:end -->",
        text,
        re.S,
    )
    if match is None:
        raise AssertionError("README.md is missing the <!-- linux-packages:start/end --> block")
    return re.findall(r"`([A-Za-z0-9.+_-]+)`", match.group(1))


def read_readme_development_paths(text):
    section = re.search(r"^## Development\s*$(.*?)(?=^## |\Z)", text, re.M | re.S)
    if section is None:
        raise AssertionError("README.md is missing the ## Development section")
    return set(re.findall(r"^\|\s*`([^`]+)`\s*\|", section.group(1), re.M))


def read_markdown_link_targets(text):
    return {
        target.removeprefix("./")
        for target in re.findall(r"\[[^\]]+\]\(([^)#\s]+)(?:#[^)\s]*)?\)", text)
    }


def load_pins():
    spec = importlib.util.spec_from_file_location("pins", ROOT / "tools/ci/pins.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.nsh_revision(ROOT), mod.toolchain(ROOT)


def clean_base_env():
    return {
        k: v
        for k, v in os.environ.items()
        if k not in SCRUBBED_ENV_KEYS and not k.startswith("FAKE_")
    }


class StaticBootstrapContractTests(unittest.TestCase):
    """Cross-platform checks that run on Linux, macOS and Windows."""

    def test_linux_packages_match_readme(self):
        pkgs = read_linux_packages()
        self.assertGreaterEqual(len(pkgs), 14)
        self.assertEqual(pkgs, sorted(set(pkgs)), "tools/ci/linux-packages.txt must stay sorted and unique")
        for required in ("pkg-config", "libgtk-4-dev", "libmpv-dev", "libssl-dev", "libdbus-1-dev",
                         "libgstreamer1.0-dev", "libgstreamer-plugins-base1.0-dev", "clang", "cmake", "ninja-build"):
            self.assertIn(required, pkgs)

        readme_pkgs = read_readme_linux_packages(README.read_text(encoding="utf-8"))
        self.assertEqual(sorted(readme_pkgs), pkgs, "README.md Linux package list drifted from tools/ci/linux-packages.txt")

        drifted = README.read_text(encoding="utf-8").replace("`pkg-config`", "`missing-pkg`", 1)
        self.assertNotEqual(sorted(read_readme_linux_packages(drifted)), pkgs)

        setup = SETUP_ACTION.read_text(encoding="utf-8")
        self.assertIn("tools/ci/linux-packages.txt", setup)

    def test_readme_workspace_table_and_docs_index_stay_complete(self):
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        readme = README.read_text(encoding="utf-8")
        dev_paths = read_readme_development_paths(readme)
        for member in manifest["workspace"]["members"]:
            self.assertIn(member, dev_paths, f"README.md Development table is missing {member}")
        removed_row = re.sub(r"^\| `crates/iptv-app` \|.*$\n?", "", readme, count=1, flags=re.M)
        self.assertIn("`iptv-app`", removed_row)
        self.assertNotIn("crates/iptv-app", read_readme_development_paths(removed_row))

        index_text = (ROOT / "docs/index.md").read_text(encoding="utf-8")
        index_links = read_markdown_link_targets(index_text)
        for doc in sorted((ROOT / "docs").glob("*.md")):
            if doc.name != "index.md":
                self.assertIn(doc.name, index_links, f"docs/index.md is missing a navigable link to {doc.name}")
        unlinked = index_text.replace("[Architecture](architecture.md)", "architecture.md", 1)
        self.assertIn("architecture.md", unlinked)
        self.assertNotIn("architecture.md", read_markdown_link_targets(unlinked))

    def test_windows_bootstrap_and_validation_routing(self):
        ps1 = BOOTSTRAP_PS1.read_text(encoding="utf-8")
        for token in ("[switch]$CheckOnly", "[switch]$SkipNsh", "[switch]$SkipRuntime",
                      "function Invoke-NativeProbe", "$savedPath = $env:PATH", "$env:PATH = $savedPath",
                      "$env:RUSTUP_TOOLCHAIN = $savedToolchain",
                      "vswhere.exe", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                      "um/Windows.h", "fetch-mpv-windows.ps1",
                      "cargo check --locked -p iptv-app"):
            self.assertIn(token, ps1)

        windows_yml = (ROOT / ".github/workflows/windows.yml").read_text(encoding="utf-8")
        self.assertIn("./tools/bootstrap.ps1 -CheckOnly -SkipNsh -AllowMinimalToolchain", windows_yml)
        ci_yml = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn("/bin/bash tools/bootstrap.sh --check-only --skip-nsh", ci_yml)
        self.assertIn("if: matrix.part != 'app'", ci_yml)
        package_yml = (ROOT / ".github/workflows/package-check.yml").read_text(encoding="utf-8")
        self.assertIn("/bin/bash tools/bootstrap.sh --check-only --skip-nsh --allow-minimal-toolchain", package_yml)

        spec = importlib.util.spec_from_file_location("validation_plan", ROOT / "tools/ci/validation-plan.py")
        vp = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(vp)

        readme_plan = vp.select(["README.md"])
        self.assertTrue(readme_plan["helpers_full"])
        self.assertFalse(any(readme_plan[lane] for lane in vp.LANES))

        pkg_plan = vp.select(["tools/ci/linux-packages.txt"])
        self.assertTrue(all(pkg_plan[lane] for lane in vp.LANES))

        sh_plan = vp.select(["tools/bootstrap.sh"])
        self.assertTrue(sh_plan["host"])
        self.assertTrue(sh_plan["helpers_full"])

        ps1_plan = vp.select(["tools/bootstrap.ps1"])
        self.assertTrue(ps1_plan["windows"])
        self.assertFalse(ps1_plan["host"])
        self.assertTrue(ps1_plan["helpers_full"])


@unittest.skipUnless(os.name == "posix" and shutil.which("bash"), "bootstrap.sh requires POSIX Bash")
class BootstrapPosixTests(unittest.TestCase):
    def setUp(self):
        self.nsh_rev, self.toolchain = load_pins()
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.bin_dir = self.tmp / "bin"
        self.bin_dir.mkdir()
        self.call_log = self.tmp / "calls.log"

    def write_stub(self, name, body):
        path = self.bin_dir / name
        path.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + textwrap.dedent(body), encoding="utf-8")
        path.chmod(0o755)
        return path

    def install_base_stubs(
        self,
        missing_pkgs=(),
        foreign_only_pkgs=(),
        multiarch_pkgs=(),
        toolchain_installed=True,
        components_installed=True,
        wasm_installed=True,
    ):
        missing_str = " ".join(missing_pkgs)
        foreign_str = " ".join(foreign_only_pkgs)
        multiarch_str = " ".join(multiarch_pkgs)
        self.write_stub(
            "sudo",
            f"""\
            echo "sudo $*" >> "{self.call_log}"
            if [ "${{FAKE_FORBID_SUDO:-0}}" = 1 ]; then
              echo "unexpected sudo invocation: $*" >&2
              exit 99
            fi
            exec "$@"
            """,
        )
        self.write_stub(
            "apt-get",
            f"""\
            echo "apt-get $*" >> "{self.call_log}"
            exit 0
            """,
        )
        self.write_stub(
            "dpkg",
            """\
            if [ "$1" = --print-architecture ]; then echo amd64; fi
            """,
        )
        self.write_stub(
            "dpkg-query",
            f"""\
            pkg="${{@: -1}}"
            for m in {missing_str}; do
              if [ "$pkg" = "$m" ]; then
                exit 1
              fi
            done
            for f in {foreign_str}; do
              if [ "$pkg" = "$f" ]; then
                printf 'i386:installed\\n'
                exit 0
              fi
            done
            for ma in {multiarch_str}; do
              if [ "$pkg" = "$ma" ]; then
                printf 'amd64:installed\\ni386:installed\\n'
                exit 0
              fi
            done
            if [ "$pkg" = "pkg-config" ]; then
              printf 'all:installed\\n'
            else
              printf 'amd64:installed\\n'
            fi
            """,
        )
        self.write_stub(
            "sha256sum",
            """\
            python3 -B -c '
            import hashlib, sys
            data = sys.stdin.buffer.read() if len(sys.argv) == 1 else open(sys.argv[1], "rb").read()
            print(hashlib.sha256(data).hexdigest() + "  -")
            ' "$@"
            """,
        )
        self.write_stub(
            "rustup",
            f"""\
            echo "rustup auto=${{RUSTUP_AUTO_INSTALL:-unset}} $*" >> "{self.call_log}"
            case "$1" in
              which)
                bin_name="${{@: -1}}"
                if [ "$bin_name" = "cargo" ]; then
                  if [ "${{FAKE_TOOLCHAIN_OK:-{'1' if toolchain_installed else '0'}}}" = 1 ]; then
                    echo "{self.bin_dir}/cargo"
                    exit 0
                  fi
                  exit 1
                fi
                if [ "${{FAKE_COMPONENTS_OK:-{'1' if components_installed else '0'}}}" = 1 ]; then
                  echo "{self.bin_dir}/$bin_name"
                  exit 0
                fi
                exit 1
                ;;
              target)
                if [ "$2" = list ] && [ "${{FAKE_WASM_OK:-{'1' if wasm_installed else '0'}}}" = 1 ]; then
                  echo "wasm32-unknown-unknown"
                fi
                exit 0
                ;;
              toolchain|component)
                exit 0
                ;;
            esac
            exit 0
            """,
        )
        self.write_stub(
            "cargo",
            f"""\
            echo "cargo $*" >> "{self.call_log}"
            for ((i=1; i<=$#; i++)); do
              if [ "${{!i}}" = "--manifest-path" ]; then
                next=$((i + 1))
                manifest="${{!next}}"
                out="$(dirname "$manifest")/target/release/nsh"
                mkdir -p "$(dirname "$out")"
                printf '#!/bin/sh\\nexit 0\\n' > "$out"
                chmod +x "$out"
              fi
            done
            exit 0
            """,
        )

    def make_nsh_repo(self, nsh_dir=None, commit=None, dirty=False, with_cli=False, stamp=None):
        nsh_dir = Path(nsh_dir) if nsh_dir else (self.tmp / "nsh-patina")
        nsh_dir.mkdir(parents=True, exist_ok=True)
        (nsh_dir / "crates/nsh-core").mkdir(parents=True, exist_ok=True)
        git_env = {
            "PATH": os.environ["PATH"],
            "HOME": str(self.tmp),
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "Synthetic",
            "GIT_AUTHOR_EMAIL": "synthetic@example.test",
            "GIT_COMMITTER_NAME": "Synthetic",
            "GIT_COMMITTER_EMAIL": "synthetic@example.test",
        }
        subprocess.run(["git", "-C", str(nsh_dir), "init", "--quiet"], env=git_env, check=True)
        (nsh_dir / ".gitignore").write_text("/target\n", encoding="utf-8")
        (nsh_dir / "Cargo.toml").write_text('[package]\nname = "nsh-cli"\nversion = "0.1.0"\n', encoding="utf-8")
        subprocess.run(["git", "-C", str(nsh_dir), "add", "."], env=git_env, check=True)
        subprocess.run(["git", "-C", str(nsh_dir), "commit", "--quiet", "-m", "synthetic"], env=git_env, check=True)
        actual = subprocess.check_output(["git", "-C", str(nsh_dir), "rev-parse", "HEAD"], text=True).strip()
        if dirty:
            (nsh_dir / "Cargo.toml").write_text('[package]\nname = "nsh-dirty"\nversion = "0.1.0"\n', encoding="utf-8")
        if with_cli:
            cli = nsh_dir / "target/release/nsh"
            cli.parent.mkdir(parents=True, exist_ok=True)
            cli.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            cli.chmod(0o755)
            if stamp is not None:
                (nsh_dir / "target/release/.patina-nsh-cli.rev").write_text(stamp + "\n", encoding="utf-8")
        return nsh_dir, commit or actual

    def make_fake_runtime(self, platform="linux", name=None):
        runtime_dir = self.tmp / (name or f"mpv-runtime-{platform}")
        (runtime_dir / "lib/pkgconfig").mkdir(parents=True)
        manifest = json.loads((ROOT / f"tools/mpv-runtime-unix/runtime-{platform}.json").read_text(encoding="utf-8"))
        lib_bytes = b"synthetic-guarded-libmpv"
        lib_hash = hashlib.sha256(lib_bytes).hexdigest()
        patched = {**manifest, "library_sha256": lib_hash}
        lib_file = runtime_dir / "lib" / manifest["library"]
        lib_file.write_bytes(lib_bytes)
        link_name = "libmpv.so" if platform == "linux" else "libmpv.dylib"
        (runtime_dir / "lib" / link_name).symlink_to(manifest["library"])
        (runtime_dir / "lib/pkgconfig/mpv.pc").write_text("Name: mpv\n", encoding="utf-8")
        (runtime_dir / "runtime.json").write_text(json.dumps(patched, indent=2) + "\n", encoding="utf-8")
        return runtime_dir, patched

    def run_bootstrap(self, *args, cwd=ROOT, extra_env=None):
        base = clean_base_env()
        env = {
            **base,
            "PATH": f"{self.bin_dir}{os.pathsep}{base['PATH']}",
            "PATINA_HOST_OS": "Linux",
            "PATINA_HOST_ARCH": "x86_64",
            "PATINA_PKG_MANAGER": "apt",
            "PATINA_EFFECTIVE_UID": "1000",
        }
        if extra_env:
            env.update(extra_env)
        return subprocess.run(
            ["bash", str(Path(cwd) / "tools/bootstrap.sh"), *args],
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
        )

    def test_setup_action_host_list_matches_linux_packages_on_any_posix_host(self):
        pkgs = read_linux_packages()
        self.install_base_stubs()
        spaced_dir = self.tmp / "fixture with spaces"
        spaced_dir.mkdir()
        os_release = spaced_dir / "os-release"
        os_release.write_text('ID=ubuntu\nVERSION_ID="24.04"\n', encoding="utf-8")
        setup = SETUP_ACTION.read_text(encoding="utf-8")
        section = setup.split("    - name: Identify the Linux packages\n", 1)[1]
        raw_script = textwrap.dedent(section.split("      run: |\n", 1)[1].split("    - uses:", 1)[0])
        script = raw_script.replace("/etc/os-release", shlex.quote(str(os_release)))
        out = self.tmp / "github-output"
        out.write_text("", encoding="utf-8")
        base = clean_base_env()
        subprocess.run(
            ["bash", "-euo", "pipefail", "-c", script],
            cwd=ROOT,
            check=True,
            env={
                **base,
                "PATH": f"{self.bin_dir}{os.pathsep}{base['PATH']}",
                "PREREQUISITES": "host",
                "EXTRA_PACKAGES": "",
                "RUNNER_TEMP": str(self.tmp),
                "GITHUB_OUTPUT": str(out),
            },
        )
        outputs = dict(line.split("=", 1) for line in out.read_text(encoding="utf-8").splitlines())
        self.assertEqual(sorted(outputs["packages"].split()), pkgs)

        # A manifest with only tab-indented comments must be rejected even when EXTRA_PACKAGES is non-empty.
        empty_pkg_ws = self.tmp / "empty-pkg-ws"
        (empty_pkg_ws / "tools/ci").mkdir(parents=True)
        (empty_pkg_ws / "tools/ci/linux-packages.txt").write_text("\t# comment only\n   \t  \n", encoding="utf-8")
        bad_manifest = subprocess.run(
            ["bash", "-euo", "pipefail", "-c", script],
            cwd=empty_pkg_ws,
            capture_output=True,
            text=True,
            env={
                **base,
                "PATH": f"{self.bin_dir}{os.pathsep}{base['PATH']}",
                "PREREQUISITES": "host",
                "EXTRA_PACKAGES": "xvfb",
                "RUNNER_TEMP": str(self.tmp),
                "GITHUB_OUTPUT": str(out),
            },
        )
        self.assertNotEqual(bad_manifest.returncode, 0)
        self.assertIn("linux-packages.txt is empty", bad_manifest.stderr)

    def test_help_and_flag_validation(self):
        res = self.run_bootstrap("--help")
        self.assertEqual(res.returncode, 0, res.stderr)
        for flag in ("--install-packages", "--skip-nsh", "--skip-runtime", "--check-only", "--direnv"):
            self.assertIn(flag, res.stdout)

        bad = self.run_bootstrap("--not-a-flag")
        self.assertEqual(bad.returncode, 2)
        self.assertIn("unknown option", bad.stderr)

        for combo in (("--check-only", "--install-packages"), ("--check-only", "--direnv")):
            conflict = self.run_bootstrap(*combo)
            self.assertEqual(conflict.returncode, 2)
            self.assertIn("cannot be combined", conflict.stderr)

        win = self.run_bootstrap("--check-only", extra_env={"PATINA_HOST_OS": "MINGW64_NT"})
        self.assertEqual(win.returncode, 2)
        self.assertIn("bootstrap.ps1", win.stderr)

        colon = self.run_bootstrap("--check-only", extra_env={"PATINA_MPV_RUNTIME_DIR": "/bad:path"})
        self.assertEqual(colon.returncode, 2)
        self.assertIn("must not contain ':'", colon.stderr)

    def test_check_only_reports_missing_and_multiarch_packages_without_sudo_or_bytecode(self):
        self.install_base_stubs(
            missing_pkgs=("libgtk-4-dev",),
            foreign_only_pkgs=("libva-drm2",),
            multiarch_pkgs=("libpipewire-0.3-0t64",),
        )
        res = self.run_bootstrap(
            "--check-only", "--skip-nsh", "--skip-runtime",
            extra_env={"FAKE_FORBID_SUDO": "1"},
        )
        self.assertEqual(res.returncode, 1)
        # libpipewire-0.3-0t64 (amd64+i386) passes; libgtk-4-dev (missing) and libva-drm2 (i386-only) fail.
        self.assertIn("sudo apt-get update && sudo apt-get install -y libgtk-4-dev libva-drm2", res.stderr)
        self.assertNotIn("libpipewire-0.3-0t64", res.stderr)
        calls = self.call_log.read_text(encoding="utf-8") if self.call_log.exists() else ""
        self.assertNotIn("sudo", calls)
        self.assertNotIn("apt-get", calls)

        root_res = self.run_bootstrap(
            "--check-only", "--skip-nsh", "--skip-runtime",
            extra_env={"PATINA_EFFECTIVE_UID": "0", "FAKE_FORBID_SUDO": "1"},
        )
        self.assertEqual(root_res.returncode, 1)
        self.assertIn("Run: apt-get update && apt-get install -y libgtk-4-dev libva-drm2", root_res.stderr)
        self.assertNotIn("sudo apt-get", root_res.stderr)

    def test_existing_minimal_toolchain_checks_and_adds_clippy_and_rustfmt(self):
        self.install_base_stubs(toolchain_installed=False, components_installed=False, wasm_installed=False)
        missing_tc = self.run_bootstrap("--check-only", "--skip-nsh", "--skip-runtime")
        self.assertEqual(missing_tc.returncode, 1)
        self.assertIn(f"rustup auto=0 which --toolchain {self.toolchain} cargo", self.call_log.read_text(encoding="utf-8"))

        self.install_base_stubs(toolchain_installed=True, components_installed=False, wasm_installed=False)
        check = self.run_bootstrap("--check-only", "--skip-nsh", "--skip-runtime")
        self.assertEqual(check.returncode, 1)
        self.assertIn(f"rustup component add --toolchain {self.toolchain} clippy rustfmt", check.stderr)
        self.assertIn(f"rustup target add --toolchain {self.toolchain} wasm32-unknown-unknown", check.stderr)

        # Minimal-toolchain mode (used by macOS packaging runner) relaxes optional components AND wasm32 target in --check-only.
        minimal_check = self.run_bootstrap(
            "--check-only", "--skip-nsh", "--skip-runtime", "--allow-minimal-toolchain",
        )
        self.assertEqual(minimal_check.returncode, 0, minimal_check.stderr)
        self.assertIn("Optional minimal-toolchain components", minimal_check.stdout)
        self.assertIn("Optional minimal-toolchain target wasm32-unknown-unknown", minimal_check.stdout)

        self.call_log.write_text("", encoding="utf-8")
        fix = self.run_bootstrap("--skip-nsh", "--skip-runtime", "--skip-smoke")
        self.assertEqual(fix.returncode, 0, fix.stderr)
        log_text = self.call_log.read_text(encoding="utf-8")
        self.assertIn(f"rustup auto=unset component add --toolchain {self.toolchain} clippy rustfmt", log_text)
        self.assertIn(f"rustup auto=unset target add --toolchain {self.toolchain} wasm32-unknown-unknown", log_text)

    def test_standalone_cargo_without_rustup_validates_version_and_targets(self):
        no_rustup_bin = self.tmp / "no-rustup-bin"
        no_rustup_bin.mkdir()
        for util in ("bash", "sh", "uname", "id", "sed", "tr", "grep", "cat", "dirname", "basename", "pwd", "head", "git", "curl", "python3"):
            found = shutil.which(util)
            if found:
                (no_rustup_bin / util).symlink_to(found)
        self.install_base_stubs()
        for stub in ("dpkg", "dpkg-query", "sudo"):
            (no_rustup_bin / stub).symlink_to(self.bin_dir / stub)
        (no_rustup_bin / "cargo").symlink_to(self.bin_dir / "cargo")

        wasm_lib = self.tmp / "standalone-wasm-lib"
        wasm_lib.mkdir()
        rustc_stub = no_rustup_bin / "rustc"
        rustc_stub.write_text(
            f"#!/bin/sh\n"
            f'if [ "$1" = "--version" ]; then echo "rustc ${{FAKE_RUSTC_VER:-1.75.0}} (synthetic)"; exit 0; fi\n'
            f'if [ "$1" = "--print" ]; then echo "{wasm_lib}"; exit 0; fi\n'
            f"exit 0\n",
            encoding="utf-8",
        )
        rustc_stub.chmod(0o755)

        def run_standalone(*extra_args, extra_env=None):
            env = {
                "PATH": str(no_rustup_bin),
                "PATINA_HOST_OS": "Linux",
                "PATINA_HOST_ARCH": "x86_64",
                "PATINA_PKG_MANAGER": "apt",
                "PATINA_EFFECTIVE_UID": "1000",
            }
            if extra_env:
                env.update(extra_env)
            return subprocess.run(
                ["bash", str(BOOTSTRAP), "--check-only", "--skip-nsh", "--skip-runtime", *extra_args],
                cwd=ROOT,
                env=env,
                capture_output=True,
                text=True,
            )

        wrong_ver = run_standalone()
        self.assertEqual(wrong_ver.returncode, 1)
        self.assertIn("does not match pinned toolchain", wrong_ver.stderr)

        no_wasm = run_standalone(extra_env={"FAKE_RUSTC_VER": self.toolchain})
        self.assertEqual(no_wasm.returncode, 1)
        self.assertIn("wasm32-unknown-unknown is missing for standalone Rust", no_wasm.stderr)

        minimal_ok = run_standalone("--allow-minimal-toolchain", extra_env={"FAKE_RUSTC_VER": self.toolchain})
        self.assertEqual(minimal_ok.returncode, 0, minimal_ok.stderr)

        for comp in ("cargo-clippy", "rustfmt"):
            p = no_rustup_bin / comp
            p.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            p.chmod(0o755)
        (wasm_lib / "libcore-synthetic.rlib").write_bytes(b"rlib")
        full_ok = run_standalone(extra_env={"FAKE_RUSTC_VER": self.toolchain})
        self.assertEqual(full_ok.returncode, 0, full_ok.stderr)

    def test_check_only_supports_dnf_pacman_macos_and_old_python(self):
        self.install_base_stubs()
        self.write_stub("rpm", 'if [ "$2" = "gtk4-devel" ]; then exit 1; fi; exit 0\n')
        self.write_stub("dnf", "exit 0\n")
        self.write_stub("ffmpeg", "exit 0\n")
        dnf_res = self.run_bootstrap("--check-only", "--skip-nsh", "--skip-runtime", extra_env={"PATINA_PKG_MANAGER": "dnf"})
        self.assertEqual(dnf_res.returncode, 1)
        self.assertIn("sudo dnf install -y gtk4-devel", dnf_res.stderr)

        self.write_stub("pacman", 'if [ "$2" = "mpv" ]; then exit 1; fi; exit 0\n')
        pac_res = self.run_bootstrap("--check-only", "--skip-nsh", "--skip-runtime", extra_env={"PATINA_PKG_MANAGER": "pacman"})
        self.assertEqual(pac_res.returncode, 1)
        self.assertIn("sudo pacman -S --needed mpv", pac_res.stderr)

        # Isolated macOS PATH without pkg-config or ffmpeg, plus Python 3.9 stub.
        mac_bin = self.tmp / "mac-bin"
        mac_bin.mkdir()
        for util in ("bash", "sh", "uname", "id", "sed", "tr", "grep", "cat", "dirname", "pwd", "head", "git", "curl"):
            found = shutil.which(util)
            if found:
                (mac_bin / util).symlink_to(found)
        for stub in ("rustup", "cargo", "sudo"):
            (mac_bin / stub).symlink_to(self.bin_dir / stub)
        py39 = mac_bin / "python3"
        py39.write_text(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'Python 3.9.6'; exit 0; fi\nexit 1\n",
            encoding="utf-8",
        )
        py39.chmod(0o755)
        xc = mac_bin / "xcode-select"
        xc.write_text("#!/bin/sh\necho /Library/Developer/CommandLineTools\n", encoding="utf-8")
        xc.chmod(0o755)
        brew = mac_bin / "brew"
        brew.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        brew.chmod(0o755)

        mac_res = subprocess.run(
            ["bash", str(BOOTSTRAP), "--check-only", "--skip-nsh", "--skip-runtime"],
            cwd=ROOT,
            env={
                "PATH": str(mac_bin),
                "PATINA_HOST_OS": "Darwin",
                "PATINA_HOST_ARCH": "arm64",
                "PATINA_EFFECTIVE_UID": "501",
            },
            capture_output=True,
            text=True,
        )
        self.assertEqual(mac_res.returncode, 1)
        self.assertIn("brew install python pkgconf ffmpeg", mac_res.stderr)
        self.assertIn("bootstrap check failed:", mac_res.stderr)
        self.assertNotIn("Traceback", mac_res.stderr)

    def test_install_packages_only_runs_when_explicitly_passed(self):
        self.install_base_stubs(missing_pkgs=("pkg-config",))
        without_flag = self.run_bootstrap(
            "--skip-nsh", "--skip-runtime", "--skip-smoke",
            extra_env={"FAKE_FORBID_SUDO": "1"},
        )
        self.assertNotEqual(without_flag.returncode, 0)
        calls = self.call_log.read_text(encoding="utf-8") if self.call_log.exists() else ""
        self.assertNotIn("apt-get", calls)

        with_flag = self.run_bootstrap("--install-packages", "--skip-nsh", "--skip-runtime", "--skip-smoke")
        self.assertEqual(with_flag.returncode, 0, with_flag.stderr)
        calls = self.call_log.read_text(encoding="utf-8")
        self.assertIn("sudo env DEBIAN_FRONTEND=noninteractive apt-get update", calls)
        self.assertIn("sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y pkg-config", calls)

        # Missing curl before --install-packages is installed via apt and rechecked afterward rather than aborting pre-install.
        no_curl_bin = self.tmp / "no-curl-bin"
        no_curl_bin.mkdir()
        for util in ("bash", "sh", "uname", "id", "sed", "tr", "grep", "cat", "dirname", "basename", "pwd", "head", "git", "python3", "env", "chmod"):
            found = shutil.which(util)
            if found:
                (no_curl_bin / util).symlink_to(found)
        for stub in ("dpkg", "dpkg-query", "sudo", "rustup", "cargo"):
            (no_curl_bin / stub).symlink_to(self.bin_dir / stub)
        apt_installs_curl = no_curl_bin / "apt-get"
        apt_installs_curl.write_text(
            f"#!/usr/bin/env bash\n"
            f'echo "apt-get $*" >> "{self.call_log}"\n'
            f'if [ "${{1:-}}" = "install" ]; then\n'
            f'  printf "#!/bin/sh\\nexit 0\\n" > "{no_curl_bin}/curl"\n'
            f'  chmod +x "{no_curl_bin}/curl"\n'
            f"fi\n",
            encoding="utf-8",
        )
        apt_installs_curl.chmod(0o755)
        self.call_log.write_text("", encoding="utf-8")
        recheck = subprocess.run(
            ["bash", str(BOOTSTRAP), "--install-packages", "--skip-nsh", "--skip-runtime", "--skip-smoke"],
            cwd=ROOT,
            env={
                "PATH": str(no_curl_bin),
                "PATINA_HOST_OS": "Linux",
                "PATINA_HOST_ARCH": "x86_64",
                "PATINA_PKG_MANAGER": "apt",
                "PATINA_EFFECTIVE_UID": "1000",
            },
            capture_output=True,
            text=True,
        )
        self.assertEqual(recheck.returncode, 0, recheck.stderr)
        self.assertIn("apt-get install -y pkg-config curl ca-certificates", self.call_log.read_text(encoding="utf-8"))

    def test_symlinked_runtime_and_ancestor_git_repo_are_refused_safely(self):
        self.install_base_stubs()
        shared_runtime, _ = self.make_fake_runtime("linux", name="shared-cache")
        sentinel = shared_runtime / "keep.txt"
        sentinel.write_text("do-not-delete\n", encoding="utf-8")
        symlink_runtime = self.tmp / "symlink-runtime"
        symlink_runtime.symlink_to(shared_runtime)

        for candidate_path in (str(symlink_runtime), str(symlink_runtime) + "/"):
            sym_res = self.run_bootstrap(
                "--skip-nsh", "--skip-smoke",
                extra_env={"PATINA_MPV_RUNTIME_DIR": candidate_path},
            )
            self.assertNotEqual(sym_res.returncode, 0, candidate_path)
            self.assertIn("refusing symlinked runtime destination", sym_res.stderr, candidate_path)
            self.assertTrue(sentinel.is_file(), "shared symlink target must remain untouched")

        # Non-empty unrelated directory (even with an arbitrary runtime.json) is never deleted before fetch.
        unrelated = self.tmp / "unrelated-dir"
        unrelated.mkdir()
        keep_unrelated = unrelated / "important.txt"
        keep_unrelated.write_text("keep\n", encoding="utf-8")
        (unrelated / "runtime.json").write_text('{"schema": 1, "unrelated": true}\n', encoding="utf-8")
        unrel_res = self.run_bootstrap(
            "--skip-nsh", "--skip-smoke",
            extra_env={"PATINA_MPV_RUNTIME_DIR": str(unrelated)},
        )
        self.assertNotEqual(unrel_res.returncode, 0)
        self.assertIn("refusing to overwrite non-empty directory", unrel_res.stderr)
        self.assertTrue(keep_unrelated.is_file())

        # Even a valid Patina runtime directory polluted with unrelated user files is refused.
        mixed_runtime, _ = self.make_fake_runtime("linux", name="mixed-runtime")
        (mixed_runtime / "components.json").write_text("{}\n", encoding="utf-8")
        extra_note = mixed_runtime / "user-notes.txt"
        extra_note.write_text("do-not-delete\n", encoding="utf-8")
        (mixed_runtime / "lib/libmpv.so.2").write_bytes(b"outdated-bytes")
        mixed_res = self.run_bootstrap(
            "--skip-nsh", "--skip-smoke",
            extra_env={"PATINA_MPV_RUNTIME_DIR": str(mixed_runtime)},
        )
        self.assertNotEqual(mixed_res.returncode, 0)
        self.assertIn("refusing to overwrite non-empty directory", mixed_res.stderr)
        self.assertTrue(extra_note.is_file())

        # Non-repo directory nested inside an outer git repository must NOT walk up into the outer repo.
        outer = self.tmp / "outer-dotfiles"
        outer.mkdir()
        subprocess.run(["git", "-C", str(outer), "init", "--quiet"], check=True)
        non_repo = outer / "empty-nsh-dir"
        non_repo.mkdir()
        anc_res = self.run_bootstrap(
            "--skip-runtime", "--skip-smoke",
            extra_env={"PATINA_NSH_DIR": str(non_repo)},
        )
        self.assertNotEqual(anc_res.returncode, 0)
        self.assertIn("not a Git repository", anc_res.stderr)

    def test_synthetic_workspace_dirty_drift_stamp_worktree_env_and_idempotence(self):
        # Place synthetic patina and nsh-patina as siblings so ../nsh-patina matches Cargo.toml.
        ws = self.tmp / "ws"
        repo = ws / "patina"
        (repo / "tools/ci").mkdir(parents=True)
        (repo / "tools/mpv-runtime-unix").mkdir(parents=True)
        (repo / "tools/mpv-runtime").mkdir(parents=True)
        (repo / "crates/iptv-app/fonts").mkdir(parents=True)

        shutil.copy2(BOOTSTRAP, repo / "tools/bootstrap.sh")
        for rel in ("tools/ci/pins.py", "tools/ci/linux-packages.txt", "tools/ci/locked-build-env.sh",
                    "tools/ci/locked-cargo.py", "tools/mpv-runtime-unix/runtime.py",
                    "tools/mpv-runtime/prepare.py", "rust-toolchain.toml"):
            shutil.copy2(ROOT / rel, repo / rel)
        for font in ("Roboto-Regular.woff2", "Roboto-Medium.woff2", "RobotoCondensed-Regular.woff2", "OFL.txt"):
            (repo / "crates/iptv-app/fonts" / font).write_bytes(b"synthetic-font")

        nsh_dir, commit = self.make_nsh_repo(
            nsh_dir=ws / "nsh-patina",
            dirty=True,
            with_cli=True,
            stamp=f"stale:{self.toolchain}",
        )
        (repo / "nsh.rev").write_text(commit + "\n", encoding="utf-8")

        runtime_dir, patched_manifest = self.make_fake_runtime("linux")
        (repo / "tools/mpv-runtime-unix/runtime-linux.json").write_text(
            json.dumps(patched_manifest, indent=2) + "\n", encoding="utf-8"
        )

        self.install_base_stubs()

        # 1. Dirty NSH checkout is refused even if caller environment had IPTV_ALLOW_NSH_DRIFT=1.
        with mock.patch.dict(os.environ, {"IPTV_ALLOW_NSH_DRIFT": "1"}):
            dirty_res = self.run_bootstrap(
                "--skip-smoke",
                cwd=repo,
                extra_env={"PATINA_MPV_RUNTIME_DIR": str(runtime_dir)},
            )
        self.assertNotEqual(dirty_res.returncode, 0)
        self.assertIn("uncommitted changes", dirty_res.stderr + dirty_res.stdout)

        # 2. Explicit IPTV_ALLOW_NSH_DRIFT=1 builds nsh-cli WITHOUT leaving a clean stamp.
        drift_res = self.run_bootstrap(
            "--skip-smoke",
            cwd=repo,
            extra_env={"PATINA_MPV_RUNTIME_DIR": str(runtime_dir), "IPTV_ALLOW_NSH_DRIFT": "1"},
        )
        self.assertEqual(drift_res.returncode, 0, drift_res.stderr)
        self.assertFalse((nsh_dir / "target/release/.patina-nsh-cli.rev").exists(),
                         "drift build must invalidate .patina-nsh-cli.rev")

        # 3. Reverting dirty files and running --check-only detects the unstamped drift binary!
        subprocess.run(["git", "-C", str(nsh_dir), "checkout", "--", "Cargo.toml"], check=True)
        unstamped_check = self.run_bootstrap(
            "--check-only",
            cwd=repo,
            extra_env={"PATINA_MPV_RUNTIME_DIR": str(runtime_dir)},
        )
        self.assertEqual(unstamped_check.returncode, 1)
        self.assertIn("missing a clean stamp", unstamped_check.stderr)
        self.assertEqual(list((repo / "tools").rglob("__pycache__")), [],
                         "--check-only must not write __pycache__ directories")

        # 4. Clean full run rebuilds & stamps nsh-cli, writes dev-env.sh + .envrc, and runs smoke checks.
        self.call_log.write_text("", encoding="utf-8")
        first = self.run_bootstrap(
            "--direnv",
            cwd=repo,
            extra_env={"PATINA_MPV_RUNTIME_DIR": str(runtime_dir)},
        )
        self.assertEqual(first.returncode, 0, first.stderr)
        env_file = repo / "target/dev-env.sh"
        envrc_file = repo / ".envrc"
        self.assertTrue(env_file.is_file())
        self.assertTrue(envrc_file.is_file())
        for var in ("IPTV_RS_MPV_LIB_DIR", "LIBRARY_PATH", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"):
            self.assertIn(f"export {var}=", env_file.read_text(encoding="utf-8"))

        # 5. Switching A -> B -> A worktrees always keeps the most recently sourced runtime FIRST.
        other_env = self.tmp / "other-dev-env.sh"
        other_env.write_text(
            env_file.read_text(encoding="utf-8").replace(f"{runtime_dir}/lib", "/worktree-b/lib"),
            encoding="utf-8",
        )
        sourced = subprocess.check_output(
            [
                "bash",
                "-c",
                f'source "{env_file}" && source "{other_env}" && source "{env_file}" && '
                'printf "%s\\n%s\\n" "$IPTV_RS_MPV_LIB_DIR" "$LD_LIBRARY_PATH"',
            ],
            env={"PATH": os.environ["PATH"], "LD_LIBRARY_PATH": "/usr/lib"},
            text=True,
        ).splitlines()
        self.assertEqual(sourced[0], f"{runtime_dir}/lib")
        self.assertEqual(sourced[1], f"{runtime_dir}/lib:/worktree-b/lib:/usr/lib")

        # 6. --check-only now passes cleanly, and second full run skips nsh-cli rebuild and preserves mtime.
        clean_check = self.run_bootstrap(
            "--check-only",
            cwd=repo,
            extra_env={"PATINA_MPV_RUNTIME_DIR": str(runtime_dir)},
        )
        self.assertEqual(clean_check.returncode, 0, clean_check.stderr)

        mtime_before = env_file.stat().st_mtime_ns
        self.call_log.write_text("", encoding="utf-8")
        second = self.run_bootstrap(
            "--direnv",
            cwd=repo,
            extra_env={"PATINA_MPV_RUNTIME_DIR": str(runtime_dir)},
        )
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(env_file.stat().st_mtime_ns, mtime_before)
        self.assertNotIn("-p nsh-cli", self.call_log.read_text(encoding="utf-8"))

        # 7. Overriding PATINA_NSH_DIR away from ../nsh-patina fails closed unless --skip-smoke is passed.
        isolated_nsh, _ = self.make_nsh_repo(nsh_dir=self.tmp / "isolated-nsh")
        (repo / "nsh.rev").write_text(
            subprocess.check_output(["git", "-C", str(isolated_nsh), "rev-parse", "HEAD"], text=True),
            encoding="utf-8",
        )
        mismatch_smoke = self.run_bootstrap(
            cwd=repo,
            extra_env={"PATINA_NSH_DIR": str(isolated_nsh), "PATINA_MPV_RUNTIME_DIR": str(runtime_dir)},
        )
        self.assertNotEqual(mismatch_smoke.returncode, 0)
        self.assertIn("differs from ../nsh-patina", mismatch_smoke.stderr)

    def test_staged_runtime_retry_and_nsh_checkout_lock(self):
        ws = self.tmp / "staged-ws"
        repo = ws / "patina"
        (repo / "tools/ci").mkdir(parents=True)
        (repo / "tools/mpv-runtime-unix").mkdir(parents=True)
        (repo / "tools/mpv-runtime").mkdir(parents=True)
        (repo / "crates/iptv-app/fonts").mkdir(parents=True)
        shutil.copy2(BOOTSTRAP, repo / "tools/bootstrap.sh")
        for rel in ("tools/ci/pins.py", "tools/ci/linux-packages.txt", "tools/ci/locked-build-env.sh",
                    "tools/ci/locked-cargo.py", "tools/mpv-runtime-unix/runtime.py",
                    "tools/mpv-runtime/prepare.py", "rust-toolchain.toml"):
            shutil.copy2(ROOT / rel, repo / rel)
        for font in ("Roboto-Regular.woff2", "Roboto-Medium.woff2", "RobotoCondensed-Regular.woff2", "OFL.txt"):
            (repo / "crates/iptv-app/fonts" / font).write_bytes(b"synthetic-font")

        # Build a tiny synthetic runtime tarball matching runtime.py's fetch verification contract.
        spec = importlib.util.spec_from_file_location("mpv_runtime_unix", ROOT / "tools/mpv-runtime-unix/runtime.py")
        rt_mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(rt_mod)
        lib_bytes = b"synthetic-staged-libmpv"
        lib_sha = hashlib.sha256(lib_bytes).hexdigest()
        src_sha = "a" * 64
        comp_bytes = json.dumps({"build": {"library_sha256": lib_sha, "source_sha256": src_sha}}).encode()
        archive_path = self.tmp / "synthetic-runtime.tar.gz"
        rt_mod._tar(
            archive_path,
            {
                "lib/libmpv.so.2": (lib_bytes, 0o755),
                "components.json": (comp_bytes, 0o644),
                "sources.lock.json": (b"{}\n", 0o644),
                "SOURCE.txt": (b"synthetic\n", 0o644),
                "NOTICE.txt": (b"synthetic\n", 0o644),
            },
        )
        base_manifest = json.loads((ROOT / "tools/mpv-runtime-unix/runtime-linux.json").read_text(encoding="utf-8"))
        manifest = {
            **base_manifest,
            "sha256": hashlib.sha256(archive_path.read_bytes()).hexdigest(),
            "library_sha256": lib_sha,
            "source_sha256": src_sha,
        }
        (repo / "tools/mpv-runtime-unix/runtime-linux.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        (repo / "tools/ci/native_candidates.py").write_text(
            f"import sys\nprint({str(archive_path)!r})\n", encoding="utf-8"
        )
        self.install_base_stubs()

        # Simulate a previously interrupted staging directory alongside a missing destination: retry must succeed!
        dest_dir = repo / "target/mpv-runtime-linux"
        interrupted_staging = repo / "target/.mpv-runtime-linux.patina-staging/lib"
        interrupted_staging.mkdir(parents=True)
        (interrupted_staging / "partial.tmp").write_bytes(b"interrupted")
        (repo / "nsh.rev").write_text(self.nsh_rev + "\n", encoding="utf-8")

        retry_res = self.run_bootstrap("--skip-nsh", "--skip-smoke", cwd=repo)
        self.assertEqual(retry_res.returncode, 0, retry_res.stderr)
        self.assertFalse((repo / "target/.mpv-runtime-linux.patina-staging").exists())
        pc_text = (dest_dir / "lib/pkgconfig/mpv.pc").read_text(encoding="utf-8")
        self.assertIn(f"prefix={dest_dir.resolve()}\n", pc_text)

        # Outdated authentic Patina runtime in dest_dir is recognized and refreshed cleanly.
        (dest_dir / "lib/libmpv.so.2").write_bytes(b"stale-bytes")
        refresh_res = self.run_bootstrap("--skip-nsh", "--skip-smoke", cwd=repo)
        self.assertEqual(refresh_res.returncode, 0, refresh_res.stderr)
        self.assertEqual((dest_dir / "lib/libmpv.so.2").read_bytes(), lib_bytes)

        # Shared NSH checkout lock serializes concurrent worktree bootstrap runs with different nsh.rev pins.
        nsh_dir, commit1 = self.make_nsh_repo(nsh_dir=ws / "nsh-patina")
        git_env = {
            "PATH": os.environ["PATH"],
            "HOME": str(self.tmp),
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "Synthetic",
            "GIT_AUTHOR_EMAIL": "synthetic@example.test",
            "GIT_COMMITTER_NAME": "Synthetic",
            "GIT_COMMITTER_EMAIL": "synthetic@example.test",
        }
        (nsh_dir / "README.md").write_text("second commit\n", encoding="utf-8")
        subprocess.run(["git", "-C", str(nsh_dir), "add", "README.md"], env=git_env, check=True)
        subprocess.run(["git", "-C", str(nsh_dir), "commit", "--quiet", "-m", "second"], env=git_env, check=True)
        commit2 = subprocess.check_output(["git", "-C", str(nsh_dir), "rev-parse", "HEAD"], text=True).strip()

        repo2 = ws / "patina-wt2"
        shutil.copytree(repo, repo2, symlinks=True)
        (repo / "nsh.rev").write_text(commit1 + "\n", encoding="utf-8")
        (repo2 / "nsh.rev").write_text(commit2 + "\n", encoding="utf-8")

        # Record the exact NSH HEAD seen while cargo builds nsh-cli, with a brief pause to overlap without locking.
        self.write_stub(
            "cargo",
            """\
            for ((i=1; i<=$#; i++)); do
              if [ "${!i}" = "--manifest-path" ]; then
                next=$((i + 1))
                manifest="${!next}"
                repo_dir="$(dirname "$manifest")"
                rev_before=$(git -C "$repo_dir" rev-parse HEAD)
                sleep 0.25
                rev_after=$(git -C "$repo_dir" rev-parse HEAD)
                out="$repo_dir/target/release/nsh"
                mkdir -p "$(dirname "$out")"
                printf '%s:%s\\n' "$rev_before" "$rev_after" > "$out"
                chmod +x "$out"
              fi
            done
            exit 0
            """,
        )
        base = clean_base_env()
        common_env = {
            **base,
            "PATH": f"{self.bin_dir}{os.pathsep}{base['PATH']}",
            "PATINA_HOST_OS": "Linux",
            "PATINA_HOST_ARCH": "x86_64",
            "PATINA_PKG_MANAGER": "apt",
            "PATINA_EFFECTIVE_UID": "1000",
        }
        p1 = subprocess.Popen(
            ["bash", str(repo / "tools/bootstrap.sh"), "--skip-runtime", "--skip-smoke"],
            cwd=repo, env=common_env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        p2 = subprocess.Popen(
            ["bash", str(repo2 / "tools/bootstrap.sh"), "--skip-runtime", "--skip-smoke"],
            cwd=repo2, env=common_env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        _, err1 = p1.communicate(timeout=20)
        _, err2 = p2.communicate(timeout=20)
        self.assertEqual(p1.returncode, 0, err1)
        self.assertEqual(p2.returncode, 0, err2)
        final_head = subprocess.check_output(["git", "-C", str(nsh_dir), "rev-parse", "HEAD"], text=True).strip()
        built_revs = (nsh_dir / "target/release/nsh").read_text(encoding="utf-8").strip()
        stamp_val = (nsh_dir / "target/release/.patina-nsh-cli.rev").read_text(encoding="utf-8").strip()
        self.assertEqual(built_revs, f"{final_head}:{final_head}", "checkout changed mid-build despite lock")
        self.assertEqual(stamp_val, f"{final_head}:{self.toolchain}", "stamp drifted from final built binary")


if __name__ == "__main__":
    unittest.main()
