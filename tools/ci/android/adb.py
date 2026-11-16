#!/usr/bin/env python3
"""Bound one adb command on Linux/macOS without requiring GNU timeout.

The command inherits stdin/stdout/stderr. Diagnostics identify only its smoke
phase and command category, never shell arguments or device content.
"""
import argparse
import math
import os
import signal
import subprocess
import sys
import time


def positive(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("must be finite and positive")
    return number


def signal_group(process, signum):
    try:
        os.killpg(process.pid, signum)
        return True
    except ProcessLookupError:
        return False


def stop(process, grace):
    # The adb client may exit on TERM while an owned descendant ignores it.
    # Retain a bounded grace for the whole group, then kill any survivors.
    deadline = time.monotonic() + grace
    signal_group(process, signal.SIGTERM)
    try:
        process.wait(timeout=grace)
    except subprocess.TimeoutExpired:
        pass
    remaining = deadline - time.monotonic()
    if remaining > 0 and signal_group(process, 0):
        time.sleep(remaining)
    signal_group(process, signal.SIGKILL)
    try:
        process.wait(timeout=grace)
    except subprocess.TimeoutExpired:
        pass


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=positive, default=15)
    parser.add_argument("--grace", type=positive, default=2)
    parser.add_argument("--phase", required=True)
    parser.add_argument("--category", required=True)
    parser.add_argument("--diagnostic-fd", type=int, default=2)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    options = parser.parse_args(argv)
    command = options.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("a command is required after --")
    if os.name != "posix":
        parser.error("the Android smoke process-group guard requires a Unix host")

    def diagnostic(message):
        line = f"adb guard: phase={options.phase!r} category={options.category!r}: {message}\n"
        try:
            os.write(options.diagnostic_fd, line.encode())
        except OSError:
            sys.stderr.write(line)

    interrupted_by = None

    def interrupted(signum, _frame):
        nonlocal interrupted_by
        interrupted_by = signum

    previous = {sig: signal.signal(sig, interrupted) for sig in (signal.SIGINT, signal.SIGTERM)}
    process = None
    terminate = False
    try:
        process = subprocess.Popen(command, start_new_session=True)
        deadline = time.monotonic() + options.timeout
        while True:
            if interrupted_by is not None:
                terminate = True
                diagnostic("interrupted; terminating command group")
                return 128 + interrupted_by
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                terminate = True
                diagnostic(f"timed out after {options.timeout:g}s; terminating command group")
                return 124
            try:
                status = process.wait(timeout=min(remaining, 0.1))
                return 128 - status if status < 0 else status
            except subprocess.TimeoutExpired:
                pass
    except OSError:
        diagnostic("could not start command")
        return 127
    finally:
        for sig in previous:
            signal.signal(sig, signal.SIG_IGN)
        if terminate and process is not None:
            stop(process, options.grace)
        for sig, handler in previous.items():
            signal.signal(sig, handler)


if __name__ == "__main__":
    sys.exit(main())
