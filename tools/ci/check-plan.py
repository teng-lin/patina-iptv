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
    "iptv-core", "iptv-presentation", "iptv-nav", "iptv-settings", "iptv-epg", "iptv-providers",
    "iptv-guide", "iptv-import", "iptv-search", "iptv-library",
)
REPAIR = "guide_repair::tests::repair_analysis_on_100k_channels_and_1k_groups_keeps_display_lane_responsive"
INSPECTION = "foundation_inspection::a_hundred_thousand_channel_analysis_keeps_display_reads_responsive"
LATENCY = "store_latency::guide_input_stays_under_budget_while_the_database_writer_is_locked"
BUSY_READER = "store_latency::a_store_read_does_not_wait_for_a_busy_writer"
GTK_LIFECYCLE = "native_gtk_player_shares_pip_frames_and_recreates_only_after_context_loss"
COVERAGE_FEATURES = "iptv-runtime/test-support,iptv-app/test-support"
WASM_CLIPPY = ("cargo", "clippy", "--locked", "--target", "wasm32-unknown-unknown", "--", "-D", "warnings")
PERFORMANCE_HARNESSES = {"app", "native_player"}
# CI can run the plan as two parallel parts. The app part owns iptv-app's two
# integration harnesses and everything that reuses them, plus the browser
# application-stack lints to even out the two jobs; the core part runs every
# other check. Together they run exactly the full plan. Listed in plan order.
PARTS = ("app", "core")
APP_HARNESSES = ("app", "native_player")
# Measured on master and PRs (2026-11-24): core 10.0-14.1 min, app 9.6-10.3 min.
# The four lints below average about 40 s on the core part.
APP_STACK_LINTS = ("runtime-wasm", "app-wasm", "web-adapters-wasm", "tizen-nsh-wasm")
APP_PART_STEPS = ("app-tests", *APP_STACK_LINTS, "busy-guide-navigation", "busy-store-reader", "native-gtk-lifecycle")
HARNESS_STEPS = ("workspace-tests", "app-tests")


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


def _members(root):
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    return [path for pattern in manifest["workspace"]["members"]
            for path in sorted(root.glob(pattern)) if (path / "Cargo.toml").is_file()]


def _enabled_features(package):
    """The package's own features the plan enables: default, plus test support, transitively."""
    plan_features = {}
    for feature in COVERAGE_FEATURES.split(","):
        crate, name = feature.split("/")
        plan_features.setdefault(crate, set()).add(name)
    features = package.get("features", {})
    pending = ["default", *plan_features.get(package["package"]["name"], ())]
    enabled = set()
    while pending:
        feature = pending.pop()
        if feature not in enabled:
            enabled.add(feature)
            # Only the package's own features; `dep:` and `crate/feature` entries enable other crates.
            pending.extend(item for item in features.get(feature, ()) if ":" not in item and "/" not in item)
    return enabled


def _tested_targets(root, kind):
    """Names of the `kind` ("test" or "bin") targets `cargo test --tests` builds for the plan.

    Follows Cargo's discovery: explicit targets, then auto-discovered files not
    already claimed by one (by path or name). An explicit target with
    `test = false`, or whose required features the plan leaves off, is skipped,
    as --tests skips it rather than failing.
    """
    names = set()
    for member in _members(root):
        package = tomllib.loads((member / "Cargo.toml").read_text(encoding="utf-8"))
        enabled = _enabled_features(package)
        claimed_paths, claimed_names = set(), set()
        for target in package.get(kind, []):
            claimed_names.add(target["name"])
            if target.get("path"):
                claimed_paths.add((member / target["path"]).resolve())
            if target.get("test", True) and set(target.get("required-features", ())) <= enabled:
                names.add(target["name"])
        if kind == "test" and package["package"].get("autotests", True):
            found = [(path.stem, path) for path in (member / "tests").glob("*.rs")]
            found += [(path.parent.name, path) for path in (member / "tests").glob("*/main.rs")]
        elif kind == "bin" and package["package"].get("autobins", True):
            main = member / "src/main.rs"
            found = [(package["package"]["name"], main)] if main.is_file() else []
            found += [(path.stem, path) for path in (member / "src/bin").glob("*.rs")]
            found += [(path.parent.name, path) for path in (member / "src/bin").glob("*/main.rs")]
        else:
            found = []
        names.update(name for name, path in found
                     if name not in claimed_names and path.resolve() not in claimed_paths)
    return sorted(names)


def integration_tests(root=ROOT):
    """Names of the integration test targets `cargo test --tests` builds for the plan."""
    names = _tested_targets(root, "test")
    if not set(APP_HARNESSES) <= set(names):
        raise ValueError("the app part's integration harnesses are missing")
    return names


def tested_binaries(root=ROOT):
    """Names of the binary targets whose unit tests `cargo test --tests` runs.

    Named one by one: `--bins` would also build binaries marked `test = false`.
    """
    return _tested_targets(root, "bin")


def step(name, *argv):
    return {"name": name, "argv": list(argv)}


def plan(root=ROOT, release=False, tier="full", host="linux", test_threads=None, skip_app_build=False, part=None):
    if tier not in ("pr", "full", "native") or host not in ("linux", "macos", "windows"):
        raise ValueError("unsupported validation tier or host")
    if part is not None and (part not in PARTS or tier == "native"):
        raise ValueError("plan parts split the pr and full tiers only")
    workspace = package_args(workspace_crates(root))
    test_args = ["--locked", "--features", COVERAGE_FEATURES, *workspace]
    app_build = step("app-build", "cargo", "build", "--locked", "-p", "iptv-app", "--timings",
                     *(["--release"] if release else []))
    app_build_steps = [] if skip_app_build else [app_build]
    if tier == "native":
        return [
            step("native-test-compilation", "cargo", "check", "--locked", "--timings", "--workspace", "--all-targets",
                 "--features", COVERAGE_FEATURES),
            step("native-player-tests", "cargo", "test", "--locked", "--timings", "-p", "iptv-player-mpv", "--lib",
                 "--", "--format=pretty"),
            *([step("windows-updater-tests", "cargo", "test", "--locked", "--timings",
                    "-p", "iptv-runtime", "--lib", "app_updates::windows::tests", "--", "--format=pretty")]
              if host == "windows" else []),
            *app_build_steps,
        ]
    memory = [argument for module in ("catalog_stream_memory::", "epg_memory::", "epg_stream_memory::")
              for argument in ("--skip", module)] if tier == "pr" else []
    workspace_test_options = ["--format=pretty", *memory]
    if test_threads is not None:
        workspace_test_options.extend(("--test-threads", str(test_threads)))
    if part is None:
        # One feature-unified graph, without linking the examples Clippy checks.
        tests = [step("workspace-tests", "cargo", "test", *test_args, "--tests", "--timings",
                      "--", *workspace_test_options)]
    else:
        # The same packages and features as --tests, so both parts build one
        # dependency graph; only the selected test targets differ.
        others = [argument for name in integration_tests(root) if name not in APP_HARNESSES
                  for argument in ("--test", name)]
        harnesses = [argument for name in APP_HARNESSES for argument in ("--test", name)]
        binaries = [argument for name in tested_binaries(root) for argument in ("--bin", name)]
        tests = [
            step("app-tests", "cargo", "test", *test_args, *harnesses, "--timings", "--", *workspace_test_options),
            step("core-tests", "cargo", "test", *test_args, "--lib", *binaries, *others, "--timings",
                 "--", *workspace_test_options),
        ]
    steps = [
        step("workspace-clippy", "cargo", "clippy", "--locked", "--workspace", "--all-targets",
             "--features", COVERAGE_FEATURES, "--", "-D", "warnings"),
        *tests,
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
        step("busy-guide-navigation", "cargo", "test", *test_args,
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
    steps.extend(app_build_steps)
    if part == "app":
        return [check for check in steps if check["name"] in APP_PART_STEPS]
    if part == "core":
        return [check for check in steps if check["name"] not in APP_PART_STEPS]
    return steps


def format_plan(root=ROOT):
    # --all would also format the separate NSH path dependency checkout.
    return [step("workspace-format", "cargo", "fmt", *package_args(workspace_crates(root)), "--check")]


def coverage_args(root=ROOT):
    return ["--tests", "--features", COVERAGE_FEATURES, *package_args(workspace_crates(root))]


def record_harnesses(argv, options, output, root):
    """Keep the workspace test output and record the exact Cargo executables."""
    separator = argv.index("--")
    argv = [*argv[:separator], "--message-format=json-render-diagnostics", *argv[separator:]]
    harnesses = {}
    with subprocess.Popen(argv, cwd=options["cwd"], env=options.get("env"),
                          stdout=subprocess.PIPE, text=True) as process:
        for line in process.stdout:
            try:
                message = json.loads(line) if line.startswith("{") else {}
            except ValueError:
                message = {}
            reason = message.get("reason")
            if reason == "compiler-artifact":
                target = message["target"]
                if (target["name"] in PERFORMANCE_HARNESSES and "test" in target["kind"]
                        and Path(message["manifest_path"]) == root / "crates/iptv-app/Cargo.toml"
                        and message.get("executable")):
                    name = target["name"]
                    executable = message["executable"]
                    if name in harnesses and harnesses[name] != executable:
                        raise ValueError(f"Cargo reported two executables for {name}")
                    harnesses[name] = executable
            elif reason not in ("build-finished", "build-script-executed", "compiler-message"):
                print(line, end="", flush=True)
        status = process.wait()
    if status == 0:
        if harnesses.keys() != PERFORMANCE_HARNESSES:
            raise ValueError("Cargo did not report both Patina performance harnesses")
        output.write_text(json.dumps(harnesses), encoding="utf-8")
    return status


def run(steps, root=ROOT, runner=subprocess.run, harness_output=None):
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
            if harness_output and check["name"] in HARNESS_STEPS:
                status = record_harnesses(check["argv"], options, harness_output, root)
            else:
                status = runner(check["argv"], **options).returncode
        except (OSError, ValueError) as error:
            print(f"could not start {check['name']}: {error}", file=sys.stderr)
            return 127
        if status:
            print(f"{check['name']} failed (exit {status})", file=sys.stderr)
            return status
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    listing = actions.add_parser("list", help="show the common Rust plan without executing it")
    listing.add_argument("--json", action="store_true")
    listing.add_argument("--release", action="store_true", help="build the normal app in release mode")
    running = actions.add_parser("run", help="stream the common Rust checks, failing fast")
    running.add_argument("--release", action="store_true", help="build the normal app in release mode")
    running.add_argument("--harness-output", type=Path, default=os.environ.get("IPTV_CI_HARNESSES") or None,
                         help="record workspace test executables for performance journeys")
    for action in (listing, running):
        action.add_argument("--skip-app-build", action="store_true", help="reuse the app build already verified by packaging")
        action.add_argument("--test-threads", type=int, default=os.environ.get("IPTV_CI_TEST_THREADS") or None,
                            help="override the Rust test threads for the workspace test step")
        action.add_argument("--tier", choices=("pr", "full", "native"), default="full")
        action.add_argument("--host", choices=("linux", "macos", "windows"), default="linux")
        action.add_argument("--part", choices=PARTS,
                            help="run one of the two parallel CI parts instead of the whole plan")
    actions.add_parser("format", help="check formatting of workspace packages only")
    inventory = actions.add_parser("inventory", help="print the shared crate inventory")
    inventory.add_argument("--kind", choices=("workspace", "libraries", "portable"), default="workspace")
    inventory.add_argument("--cargo-args", action="store_true")
    actions.add_parser("coverage-args", help="print default coverage package/feature arguments")
    options = parser.parse_args(argv)
    if getattr(options, "test_threads", None) is not None and options.test_threads < 1:
        parser.error("--test-threads must be positive")
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
        if options.harness_output and (options.tier == "native" or options.part == "core"):
            parser.error("this plan does not build the performance harnesses")
        if options.part and options.tier == "native":
            parser.error("plan parts split the pr and full tiers only")
        return run(plan(release=options.release, tier=options.tier, host=options.host, test_threads=options.test_threads,
                        skip_app_build=options.skip_app_build, part=options.part),
                   harness_output=options.harness_output)
    else:
        if options.part and options.tier == "native":
            parser.error("plan parts split the pr and full tiers only")
        checks = plan(release=options.release, tier=options.tier, host=options.host, test_threads=options.test_threads,
                      skip_app_build=options.skip_app_build, part=options.part)
        if options.json:
            print(json.dumps(checks, indent=2))
        else:
            for check in checks:
                print(f"{check['name']}: {shlex.join(check['argv'])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
