#!/usr/bin/env python3
"""Run an explicit native test on a private X11 display, with software GL."""
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile


def main(command):
    if not command:
        raise SystemExit("usage: run-xvfb.py COMMAND [ARG ...]")
    with tempfile.TemporaryDirectory(prefix="patina-native-display-") as directory:
        reader, writer = os.pipe()
        with Path(directory, "xvfb.log").open("w+") as log:
            display = subprocess.Popen(
                ["Xvfb", "-displayfd", str(writer), "-screen", "0", "1280x720x24", "-nolisten", "tcp"],
                pass_fds=(writer,), stdout=log, stderr=log,
            )
            os.close(writer)
            try:
                if not select.select([reader], [], [], 20)[0]:
                    raise RuntimeError("Xvfb did not acknowledge its private display")
                number = os.read(reader, 64).decode().strip()
                if not number.isdigit() or display.poll() is not None:
                    raise RuntimeError("Xvfb did not start successfully")
                env = os.environ.copy()
                env.update(DISPLAY=f":{number}", GDK_BACKEND="x11", GSK_RENDERER="gl", LIBGL_ALWAYS_SOFTWARE="1")
                env.pop("WAYLAND_DISPLAY", None)
                env.pop("HYPRLAND_INSTANCE_SIGNATURE", None)
                return subprocess.run(command, env=env).returncode
            except Exception:
                log.seek(0)
                print(log.read(), file=sys.stderr)
                raise
            finally:
                os.close(reader)
                display.terminate()
                try:
                    display.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    display.kill()
                    display.wait()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
