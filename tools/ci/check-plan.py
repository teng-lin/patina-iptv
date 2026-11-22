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
INSPECTION = "foundation_inspection::a_hundred_thousand_channel_analysis_keeps_display_reads_responsive"
LATENCY = "store_latency::guide_input_stays_under_budget_while_the_database_writer_is_locked"
BUSY_READER = "store_latency::a_store_read_does_not_wait_for_a_busy_writer"
GTK_LIFECYCLE = "native_gtk_player_shares_pip_frames_and_recreates_only_after_context_loss"
COVERAGE_FEATURES = "iptv-runtime/test-support,iptv-app/test-support"
WASM_CLIPPY = ("cargo", "clippy", "--locked", "--target", "wasm32-unknown-unknown", "--", "-D", "warnings")


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


def plan(root=ROOT, release=False, tier="full", host="linux"):
    if tier not in ("pr", "full", "native") or host not in ("linux", "macos", "windows"):
        raise ValueError("unsupported validation tier or host")
    workspace = package_args(workspace_crates(root))
    test_args = ["--locked", "--features", COVERAGE_FEATURES, *workspace]
    app_build = step("app-build", "cargo", "build", "--locked", "-p", "iptv-app", "--timings",
                     *(["--release"] if release else []))
    if tier == "native":
        return [
            step("native-test-compilation", "cargo", "check", "--locked", "--workspace", "--all-targets",
                 "--features", COVERAGE_FEATURES),
            step("native-player-tests", "cargo", "test", "--locked", "-p", "iptv-player-mpv", "--lib",
                 "--", "--format=pretty"),
            app_build,
        ]
    memory = [argument for module in ("catalog_stream_memory::", "epg_memory::", "epg_stream_memory::")
              for argument in ("--skip", module)] if tier == "pr" else []
    steps = [
        step("workspace-clippy", "cargo", "clippy", "--locked", "--workspace", "--all-targets",
             "--features", COVERAGE_FEATURES, "--", "-D", "warnings"),
        # One feature-unified graph, without linking the examples Clippy checks.
        step("workspace-tests", "cargo", "test", *test_args, "--tests", "--timings", "--", "--format=pretty", *memory),
        step("repair-display-independence", "cargo", "test", *test_args,
             "--lib", REPAIR, "--", "--ignored", "--exact", "--format=pretty"),
        step("inspection-display-budget", "cargo", "test", *test_args,
             "--test", "it", INSPECTION, "--", "--ignored", "--exact", "--format=pretty"),
    ]
    steps.extend(
        step(f"wasm-{crate}", "cargo", "build", "--locked", "-p", crate, "--target", "wasm32-unknown-unknown")
        for crate in PORTABLE if host == "linux"
    )
    if host == "linux":
        # The Tizen host's core; its TV artifact is built by hosts/tizen/build.sh.
        steps.append(step("web-core-wasm", "cargo", "build", "--locked", "-p", "iptv-web",
                          "--target", "wasm32-unknown-unknown"))
        # The SQLite store's browser build: its tests compile for wasm32, then
        # run in Node (needs a clang with the wasm32 target, and Node).
        steps.append(step("store-wasm-check", "cargo", "check", "--locked", "-p", "iptv-store-sqlite",
                          "--target", "wasm32-unknown-unknown", "--lib", "--tests"))
        steps.append(step("store-wasm-clippy", "cargo", "clippy", "--locked", "-p", "iptv-store-sqlite",
                          "--target", "wasm32-unknown-unknown", "--lib", "--tests", "--", "-D", "warnings"))
        steps.append(step("store-wasm-tests", "node", "tools/wasm-test.mjs", "--cargo", "--locked",
                          "-p", "iptv-store-sqlite", "--lib", "--test", "it"))
        # The application stack's browser build: the runtime without its
        # native services, and the shell with its web host, over the store's
        # browser build. Clippy, so what only that build compiles is linted.
        steps.append(step("runtime-wasm", *WASM_CLIPPY[:3], "-p", "iptv-runtime", "--no-default-features",
                          *WASM_CLIPPY[3:]))
        steps.append(step("app-wasm", *WASM_CLIPPY[:3], "-p", "iptv-app", *WASM_CLIPPY[3:]))
        # The browser TV package's feature set (hosts/tizen-nsh): linted for
        # wasm32, and its scenario run natively (no way into the screens it
        # leaves out).
        steps.append(step("app-wasm-tv", *WASM_CLIPPY[:3], "-p", "iptv-app", "--no-default-features",
                          "--features", "browser-package", *WASM_CLIPPY[3:]))
        steps.append(step("app-tv-feature-set", "cargo", "test", "--locked", "-p", "iptv-app",
                          "--no-default-features", "--features", "test-support", "--test", "app",
                          "tv_feature_set", "--", "--format=pretty"))
        # The adapters the shell links, without the Live TV bridge.
        steps.append(step("web-adapters-wasm", *WASM_CLIPPY[:3], "-p", "iptv-web", "--no-default-features",
                          *WASM_CLIPPY[3:]))
        # The Tizen runner of the NSH interface (hosts/tizen-nsh, its own
        # workspace and lock) on the pinned toolchain; its TV module, with
        # the rebuilt std, and its size are built by hosts/tizen-nsh/build.sh.
        steps.append(step("tizen-nsh-wasm", *WASM_CLIPPY[:3], "--manifest-path", "hosts/tizen-nsh/Cargo.toml",
                          *WASM_CLIPPY[3:]))
        # The runtime's unit tests without its native services, on the host.
        steps.append(step("runtime-portable-tests", "cargo", "test", "--locked", "-p", "iptv-runtime",
                          "--no-default-features", "--lib", "--", "--format=pretty"))
    steps.extend([
        step("guide-input-budget", "cargo", "test", *test_args,
             "--test", "app", LATENCY, "--", "--ignored", "--exact", "--format=pretty"),
        step("busy-store-reader", "cargo", "test", *test_args,
             "--test", "app", BUSY_READER, "--", "--ignored", "--exact", "--format=pretty"),
    ])
    if host == "linux":
        # Documentation and portable wasm output are independent of the host OS.
        steps.append({**step("workspace-rustdoc", "cargo", "doc", "--locked", "--workspace", "--no-deps"),
                      "env": {"RUSTDOCFLAGS": "-D warnings"}})
        steps.append(step("workspace-doctests", "cargo", "test", *test_args, "--doc", "--", "--format=pretty"))
        steps.append(step("native-gtk-lifecycle", sys.executable, "tools/ci/run-xvfb.py",
                          "cargo", "test", *test_args, "--test", "native_player", GTK_LIFECYCLE,
                          "--", "--ignored", "--exact", "--format=pretty"))
    # Test-support builds must never replace the final production configuration.
    steps.append(app_build)
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
        if check.get("env"):
            options["env"] = dict(os.environ, **check["env"])
        # A protected macOS Python may strip the shell's DYLD_* variables.
        # Restore the selected runtime for Cargo without changing our environment.
        if sys.platform == "darwin" and (directory := os.environ.get("IPTV_RS_MPV_LIB_DIR")):
            options["env"] = dict(options.get("env", os.environ), DYLD_LIBRARY_PATH=directory)
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
    for action in (listing, running):
        action.add_argument("--tier", choices=("pr", "full", "native"), default="full")
        action.add_argument("--host", choices=("linux", "macos", "windows"), default="linux")
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
        return run(plan(release=options.release, tier=options.tier, host=options.host))
    else:
        checks = plan(release=options.release, tier=options.tier, host=options.host)
        if options.json:
            print(json.dumps(checks, indent=2))
        else:
            for check in checks:
                print(f"{check['name']}: {shlex.join(check['argv'])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
