#!/usr/bin/env python3
"""NSH's Cargo entry point: lock generated runners as well as the source tree."""
import os
from pathlib import Path
import tempfile
import sys

root = Path(__file__).resolve().parents[2]
args = sys.argv[1:]
real_cargo = os.environ["IPTV_REAL_CARGO"]
if Path(real_cargo).resolve() == Path(__file__).resolve():
    raise SystemExit("IPTV_REAL_CARGO must identify the real Cargo executable")
command = ""
index = 0
while index < len(args):
    argument = args[index]
    if argument in {"--color", "--config", "-C", "-Z"}:
        index += 2
        continue
    if argument.startswith(("-", "+")):
        index += 1
        continue
    command = argument
    break
if command in {"build", "check", "test", "run", "metadata", "rustc", "clippy"}:
    inline = next((a.split("=", 1)[1] for a in args if a.startswith("--manifest-path=")), None)
    if inline:
        manifest = Path(inline).resolve()
    elif "--manifest-path" in args:
        manifest = Path(args[args.index("--manifest-path") + 1]).resolve()
    else:
        manifest = Path.cwd() / "Cargo.toml"
    generated = root / "crates/iptv-app/.nsh/rust"
    if manifest.is_relative_to(generated):
        if command == "build" and os.environ.get("IPTV_CARGO_TIMINGS") == "1" and "--timings" not in args:
            args.insert(index + 1, "--timings")
        # Cargo itself rejects stale dependencies; never resolve updates here.
        contents = (root / "tools/ci/package.Cargo.lock").read_bytes()
        destination = generated / "Cargo.lock"
        if not destination.exists() or destination.read_bytes() != contents:
            with tempfile.NamedTemporaryFile(dir=generated, delete=False) as temporary:
                temporary.write(contents)
                temporary.flush()
                os.fsync(temporary.fileno())
            os.replace(temporary.name, destination)
    if "--locked" not in args:
        args.insert(index + 1, "--locked")
os.execv(real_cargo, [real_cargo, *args])
