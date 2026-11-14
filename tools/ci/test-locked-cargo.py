#!/usr/bin/env python3
"""A stale runner lock must fail before any compilation/network resolution."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory() as directory:
    package = Path(directory)
    (package / "src").mkdir()
    (package / "src/lib.rs").write_text("")
    (package / "Cargo.toml").write_text('[package]\nname="lock-check"\nversion="0.1.0"\nedition="2024"\n')
    subprocess.run([os.environ["IPTV_REAL_CARGO"], "generate-lockfile", "--offline"], cwd=package, check=True)
    manifest = package / "Cargo.toml"
    manifest.write_text(manifest.read_text().replace('version="0.1.0"', 'version="0.2.0"'))
    result = subprocess.run([str(root / "tools/ci/locked-cargo.py"), "check", "--offline"], cwd=package, capture_output=True, text=True)
    assert result.returncode != 0 and "lock" in result.stderr.lower(), result.stderr
print("stale lock refused")
# Exercise the generated-workspace seeding path, including an equals-style
# manifest argument and a stale committed runner lock.
import shutil
with tempfile.TemporaryDirectory() as directory:
    fixture = Path(directory)
    scripts = fixture / "tools/ci"
    scripts.mkdir(parents=True)
    wrapper = scripts / "locked-cargo.py"
    shutil.copyfile(root / "tools/ci/locked-cargo.py", wrapper)
    wrapper.chmod(0o755)
    package = fixture / "crates/iptv-app/.nsh/rust"
    (package / "src").mkdir(parents=True)
    (package / "src/lib.rs").write_text("")
    manifest = package / "Cargo.toml"
    manifest.write_text('[package]\nname="runner-lock-check"\nversion="0.1.0"\nedition="2024"\n')
    subprocess.run([os.environ["IPTV_REAL_CARGO"], "generate-lockfile", "--offline"], cwd=package, check=True)
    shutil.copyfile(package / "Cargo.lock", scripts / "package.Cargo.lock")
    (package / "Cargo.lock").unlink()
    command = [str(wrapper), "--color", "never", "--quiet", "check", "--offline", "--manifest-path=" + str(manifest)]
    subprocess.run(command, check=True)
    manifest.write_text(manifest.read_text().replace('version="0.1.0"', 'version="0.2.0"'))
    result = subprocess.run(command, capture_output=True, text=True)
    assert result.returncode != 0 and "lock" in result.stderr.lower(), result.stderr
# A rustc separator must not swallow the wrapper's lock flag.
    result = subprocess.run([str(wrapper), "rustc", "--offline", "--manifest-path", str(manifest), "--", "--cfg", "synthetic"], capture_output=True, text=True)
    assert result.returncode != 0 and "lock" in result.stderr.lower(), result.stderr
print("generated runner lock seeded; stale runner lock refused")
# A nested command in the sibling NSH workspace must not use its/default Rust.
import tomllib
expected = tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
environment = dict(os.environ, RUSTUP_TOOLCHAIN="synthetic-wrong-toolchain")
environment["IPTV_REAL_CARGO"] = "/synthetic/wrong/cargo"
result = subprocess.run([
    "bash", "-c", 'source "$1"; printf "%s\\n%s\\n" "$RUSTUP_TOOLCHAIN" "$IPTV_REAL_CARGO"',
    "locked-environment", str(root / "tools/ci/locked-build-env.sh")
], cwd=tempfile.gettempdir(), env=environment, text=True, capture_output=True, check=True)
channel, executable = result.stdout.strip().splitlines()
assert channel == expected, result.stdout
assert Path(executable).is_absolute() and Path(executable).is_file(), result.stdout
assert executable != str(root / "tools/ci/locked-cargo.py")
print("nested toolchain pinned and real Cargo resolved outside the repository")

# The same environment with and without rustup. rustup's choice of the pinned
# release wins; without it the Cargo on PATH is used, and no Cargo at all fails.
def locked_environment(tools):
    with tempfile.TemporaryDirectory() as directory:
        bin_directory = Path(directory) / "bin"
        bin_directory.mkdir()
        # Wrappers, not links: some interpreters locate themselves through argv[0].
        real_binaries = {"bash": shutil.which("bash"), "python3": sys.executable, "dirname": shutil.which("dirname")}
        tools = {name: f'exec "{real_binaries[name]}" "$@"' for name in ("bash", "python3", "dirname")} | tools
        for name, script in tools.items():
            (bin_directory / name).write_text("#!/bin/sh\n" + script + "\n")
            (bin_directory / name).chmod(0o755)
        return subprocess.run([
            "bash", "-c", 'source "$1" || exit 7; printf "%s\n%s\n%s\n" "$RUSTUP_TOOLCHAIN" "$IPTV_REAL_CARGO" "$CARGO"',
            "locked-environment", str(root / "tools/ci/locked-build-env.sh")
        ], cwd=tempfile.gettempdir(), env={"PATH": str(bin_directory)}, text=True, capture_output=True), bin_directory

path_cargo = 'echo "cargo 0.0.0 (synthetic)"'
result, directory = locked_environment({
    "cargo": path_cargo,
    "rustup": '[ "$1 $2 $3 $4" = "which --toolchain ' + expected + ' cargo" ] && echo /synthetic/toolchains/pinned/bin/cargo',
})
assert result.returncode == 0, result.stderr
assert result.stdout.splitlines() == [expected, "/synthetic/toolchains/pinned/bin/cargo", str(root / "tools/ci/locked-cargo.py")], result.stdout
assert result.stderr == "", result.stderr
result, directory = locked_environment({"cargo": path_cargo})
assert result.returncode == 0, result.stderr
assert result.stdout.splitlines() == [expected, str(directory / "cargo"), str(root / "tools/ci/locked-cargo.py")], result.stdout
assert "rustup not found" in result.stderr and "cargo 0.0.0 (synthetic)" in result.stderr and expected in result.stderr, result.stderr
result, directory = locked_environment({})
assert result.returncode == 7 and "neither rustup nor cargo" in result.stderr, (result.returncode, result.stderr)
print("pinned Cargo chosen through rustup; Cargo on PATH used without rustup; no Cargo refused")

# Runner graphs can contain multiple versions, so bare references must not be
# ambiguous. Exercise this without invoking another Cargo/build executable.
import runpy
validate = runpy.run_path(str(root / "tools/ci/check-package-lock.py"))["validate_references"]
packages = [
    {"name": "synthetic-app", "version": "1", "dependencies": ["base64"]},
    {"name": "base64", "version": "0.21.0"},
    {"name": "base64", "version": "0.22.1"},
]
try:
    validate(packages)
except SystemExit as error:
    assert "ambiguous" in str(error)
else:
    raise AssertionError("ambiguous runner dependency accepted")
packages[0]["dependencies"] = ["base64 0.22.1"]
validate(packages)
packages[0]["dependencies"] = ["base64 0.22.1", "base64 0.21.0"]
try:
    validate(packages)
except SystemExit as error:
    assert "unsorted" in str(error)
else:
    raise AssertionError("noncanonical runner dependencies accepted")
print("runner dependency references qualified and ordered")
