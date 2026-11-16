#!/usr/bin/env python3
"""Shared Rust checks; wrappers retain platform setup and native-only checks.

`list --json` exposes the exact argv plan without running Cargo. Commands run
in the repository root with inherited output/environment and stop on failure.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
PORTABLE = (
    "iptv-core", "iptv-nav", "iptv-settings", "iptv-epg", "iptv-providers",
    "iptv-guide", "iptv-import", "iptv-search", "iptv-library",
)
REPAIR = "guide_repair::tests::repair_analysis_on_100k_channels_and_1k_groups_keeps_display_lane_responsive"
LATENCY = "store_latency::guide_input_stays_under_budget_while_the_database_writer_is_locked"
BUSY_READER = "store_latency::a_store_read_does_not_wait_for_a_busy_writer"
COVERAGE_FEATURES = "iptv-runtime/test-support,iptv-app/test-support"


def workspace_crates(root=ROOT):
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    crates = tuple(
        tomllib.loads((root / member / "Cargo.toml").read_text(encoding="utf-8"))["package"]["name"]
        for member in manifest["workspace"]["members"]
    )
    # Coverage consumes these identifiers as whitespace-separated argv, never
    # shell code. Reject unexpected identifiers instead of changing its scope.
    if any(not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_-]*", crate) for crate in crates):
        raise ValueError("workspace package names must be plain Cargo identifiers")
    if len(crates) != len(set(crates)):
        raise ValueError("workspace package names must be unique")
    return crates


def package_args(crates):
    return [argument for crate in crates for argument in ("-p", crate)]


def step(name, *argv):
    return {"name": name, "argv": list(argv)}


def plan(root=ROOT, release=False):
    workspace = package_args(workspace_crates(root))
    test_args = ["--locked", "--features", COVERAGE_FEATURES, *workspace]
    steps = [
        step("workspace-clippy", "cargo", "clippy", "--locked", "--workspace", "--all-targets",
             "--features", COVERAGE_FEATURES, "--", "-D", "warnings"),
        # One feature-unified graph, without linking the examples Clippy checks.
        step("workspace-tests", "cargo", "test", *test_args, "--tests", "--timings", "--", "--format=pretty"),
        step("repair-display-independence", "cargo", "test", *test_args,
             "--lib", REPAIR, "--", "--ignored", "--exact", "--format=pretty"),
    ]
    steps.extend(
        step(f"wasm-{crate}", "cargo", "build", "--locked", "-p", crate, "--target", "wasm32-unknown-unknown")
        for crate in PORTABLE
    )
    steps.extend([
        step("guide-input-budget", "cargo", "test", *test_args,
             "--test", "app", LATENCY, "--", "--ignored", "--exact", "--format=pretty"),
        step("busy-store-reader", "cargo", "test", *test_args,
             "--test", "app", BUSY_READER, "--", "--ignored", "--exact", "--format=pretty"),
        # --tests excludes doctests; keep their compilation and execution explicit.
        step("workspace-doctests", "cargo", "test", *test_args, "--doc", "--", "--format=pretty"),
        # Leave the normal executable as the final artifact: integration tests
        # may build Cargo's app binary with test-support enabled.
        step("app-build", "cargo", "build", "--locked", "-p", "iptv-app", "--timings",
             *(["--release"] if release else [])),
    ])
    return steps


def format_plan(root=ROOT):
    # --all would also format the separate NSH path dependency checkout.
    return [step("workspace-format", "cargo", "fmt", *package_args(workspace_crates(root)), "--check")]


def coverage_args(root=ROOT):
    return ["--tests", "--features", COVERAGE_FEATURES, *package_args(workspace_crates(root))]


def run(steps, root=ROOT, runner=subprocess.run):
    for check in steps:
        print(f"==> {check['name']}", flush=True)
        # This is a display string only; execution always uses the argv array.
        print(shlex.join(check["argv"]), flush=True)
        options = {"cwd": root, "check": False}
        # A protected macOS Python may strip the shell's DYLD_* variables.
        # Restore the selected runtime for Cargo without changing our environment.
        if sys.platform == "darwin" and (directory := os.environ.get("IPTV_RS_MPV_LIB_DIR")):
            options["env"] = dict(os.environ, DYLD_LIBRARY_PATH=directory)
        try:
            result = runner(check["argv"], **options)
        except OSError as error:
            print(f"could not start {check['name']}: {error}", file=sys.stderr)
            return 127
        if result.returncode:
            print(f"{check['name']} failed (exit {result.returncode})", file=sys.stderr)
            return result.returncode
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    listing = actions.add_parser("list", help="show the common Rust plan without executing it")
    listing.add_argument("--json", action="store_true")
    listing.add_argument("--release", action="store_true", help="build the normal app in release mode")
    running = actions.add_parser("run", help="stream the common Rust checks, failing fast")
    running.add_argument("--release", action="store_true", help="build the normal app in release mode")
    actions.add_parser("format", help="check formatting of workspace packages only")
    inventory = actions.add_parser("inventory", help="print the shared crate inventory")
    inventory.add_argument("--kind", choices=("workspace", "libraries", "portable"), default="workspace")
    inventory.add_argument("--cargo-args", action="store_true")
    actions.add_parser("coverage-args", help="print default coverage package/feature arguments")
    options = parser.parse_args(argv)
    if options.action == "inventory":
        crates = PORTABLE if options.kind == "portable" else workspace_crates()
        if options.kind == "libraries":
            crates = tuple(crate for crate in crates if crate != "iptv-app")
        print(" ".join(package_args(crates)) if options.cargo_args else "\n".join(crates))
    elif options.action == "coverage-args":
        print(" ".join(coverage_args()))
    elif options.action == "format":
        return run(format_plan())
    elif options.action == "run":
        return run(plan(release=options.release))
    else:
        checks = plan(release=options.release)
        if options.json:
            print(json.dumps(checks, indent=2))
        else:
            for check in checks:
                print(f"{check['name']}: {shlex.join(check['argv'])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
