#!/usr/bin/env python3
"""Advisory Linux structure comparisons; only --write can fail a measurement.

The reference is the successful master artifact at the branch's merge base.
Use --base explicitly to pay for a second, local index instead. No allow list
or committed baseline is read or written.
"""
import argparse
from collections import Counter, defaultdict
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import textwrap
import time

ROOT = Path(__file__).resolve().parents[2]
TARGET = "x86_64-unknown-linux-gnu"
INPUTS = ("nsh_rev", "toolchain", "rust_analyzer", "deslop", "target", "features", "cfg")
IGNORED_ENV = ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET",
               "CARGO_BUILD_RUSTFLAGS", "IPTV_ALLOW_NSH_DRIFT")
CYCLES = {"module-cycle", "tangle"}
BENIGN_ERROR = "ERROR Config Error(s) error_sink=ConfigErrors([])"
spec = importlib.util.spec_from_file_location("deslop_pins", Path(__file__).with_name("pins.py"))
pins = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pins)


def run(arguments, tree, environment=None):
    result = subprocess.run([str(arg) for arg in arguments], cwd=tree, env=environment,
                            capture_output=True, text=True)
    if result.returncode:
        output = "\n".join((result.stdout + result.stderr).splitlines()[-30:])
        raise RuntimeError(f"{shlex.join(map(str, arguments))} exited {result.returncode}\n{output}")
    return result.stdout.strip()


def clean_environment():
    environment = dict(os.environ)
    for name in IGNORED_ENV:
        if name in environment:
            del environment[name]
            print(f"Note: ignored {name} for a reproducible Linux measurement.")
    return environment


def write_wrapper(directory):
    """Tee the real analyzer's stderr without changing its stdout or stdin."""
    wrapper = Path(directory) / "rust-analyzer"
    wrapper.write_text(f"#!{sys.executable}\n" + textwrap.dedent('''\
        import os
        import shlex
        import subprocess
        import sys

        with open(os.environ["DESLOP_RA_LOG"], "ab", buffering=0) as log:
            log.write(("### argv: " + shlex.join(sys.argv[1:]) + "\\n").encode())
            try:
                child = subprocess.Popen([os.environ["DESLOP_REAL_RA"], *sys.argv[1:]],
                                         stderr=subprocess.PIPE)
                while chunk := child.stderr.read1(8192):
                    sys.stderr.buffer.write(chunk)
                    sys.stderr.buffer.flush()
                    log.write(chunk)
                code = child.wait()
                if code < 0:
                    code = 128 - code
            except OSError as error:
                message = (str(error) + "\\n").encode()
                sys.stderr.buffer.write(message)
                log.write(message)
                code = 127
            # Separate the marker even when the analyzer omitted its final newline.
            log.write(("\\n### exit: " + str(code) + "\\n").encode())
        sys.exit(code)
        '''))
    wrapper.chmod(0o755)
    return wrapper


def index_health(log_text):
    if not log_text.strip():
        return ["rust-analyzer log is missing or empty"]
    invocations = []
    current = None
    for line in log_text.splitlines():
        if line.startswith("### argv: "):
            try:
                arguments = shlex.split(line.removeprefix("### argv: "))
            except ValueError:
                return ["malformed rust-analyzer invocation header"]
            current = {"arguments": arguments, "stderr": [], "exit": None}
            invocations.append(current)
        elif current is not None:
            if line.startswith("### exit: "):
                current["exit"] = line.removeprefix("### exit: ")
            else:
                current["stderr"].append(line)
    scip = [item for item in invocations if item["arguments"][:1] == ["scip"]]
    if len(scip) != 1:
        return [f"expected exactly one rust-analyzer scip invocation, found {len(scip)}"]
    invocation = scip[0]
    lines = invocation["stderr"]
    problems = []
    if invocation["exit"] != "0":
        problems.append(f"rust-analyzer scip exit is {invocation['exit']}, expected 0")
    if not any(line.startswith("Generating SCIP finished") for line in lines):
        problems.append("rust-analyzer scip did not finish generating SCIP")
    if not any(line.startswith("rust-analyzer: Loading build script ") for line in lines):
        problems.append("rust-analyzer scip loaded no build script; the index may be incomplete")
    for line in lines:
        if (line.startswith("ERROR ") or " ERROR " in line) and line != BENIGN_ERROR:
            problems.append(f"rust-analyzer error: {line}")
    return problems


def diagnostics(value):
    """Validate identity and shape before any report is compared or published."""
    if not isinstance(value, list):
        raise ValueError("diagnostics must be a list")
    seen = set()
    for item in value:
        if not isinstance(item, dict) or any(
                not isinstance(item.get(field), str) or not item[field]
                for field in ("lint", "key", "message")):
            raise ValueError("each diagnostic needs a nonempty lint, key and message")
        identity = item["lint"], item["key"]
        if identity in seen:
            raise ValueError(f"duplicate diagnostic identity: {identity}")
        seen.add(identity)
        if (not isinstance(item.get("notes"), list)
                or not all(isinstance(note, str) for note in item["notes"])
                or (item.get("file") is not None and not isinstance(item["file"], str))
                or (item.get("line") is not None and type(item["line"]) is not int)):
            raise ValueError(f"invalid diagnostic fields: {identity}")
        if item["lint"] in CYCLES and any(not member for member in item["key"].split(",")):
            raise ValueError(f"empty cycle member: {identity}")
    return value


def parse_tool_output(text):
    value = json.loads(text)
    if not isinstance(value, dict):
        raise ValueError("deslop JSON must be an object")
    items = diagnostics(value.get("diagnostics"))
    if type(value.get("count")) is not int or value["count"] != len(items):
        raise ValueError("deslop count does not equal len(diagnostics)")
    return items


def validate_record(record):
    if not isinstance(record, dict) or record.get("schema") != 1:
        raise ValueError("unsupported or missing report schema (expected 1)")
    if (type(record.get("valid")) is not bool or type(record.get("dirty")) is not bool
            or not isinstance(record.get("problems"), list)
            or not all(isinstance(problem, str) for problem in record["problems"])
            or record["valid"] != (not record["problems"])):
        raise ValueError("report validity, problems and dirty fields are inconsistent")
    if not isinstance(record.get("commit"), str) or not record["commit"]:
        raise ValueError("report has no commit")
    diagnostics(record.get("diagnostics"))
    if record["valid"]:
        for field in (*INPUTS, "nsh_head"):
            if not isinstance(record.get(field), str) or not record[field]:
                raise ValueError(f"report is missing input {field}")
        if record["nsh_head"] != record["nsh_rev"]:
            raise ValueError("valid report has NSH HEAD different from its pin")
    seconds = record.get("seconds", {})
    for field in ("preflight", "index"):
        value = seconds.get(field) if isinstance(seconds, dict) else None
        if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
            raise ValueError(f"invalid timing: {field}")
    return record


def target_directory(tree, environment):
    return Path(json.loads(run(["cargo", "metadata", "--format-version", "1", "--no-deps"],
                               tree, environment))["target_directory"])


def measure(tree, nsh_checkout, *, target_dir=None):
    """Preflight and freshly index one tree with strict, recorded build inputs."""
    started = time.monotonic()
    tree, nsh_checkout = Path(tree), Path(nsh_checkout)
    environment = clean_environment()
    if target_dir is not None:
        environment["CARGO_TARGET_DIR"] = str(target_dir)
    record = {"schema": 1, "commit": "unknown", "dirty": False,
              **dict.fromkeys(INPUTS), "nsh_head": None, "target": TARGET,
              "features": "default", "valid": False, "problems": [],
              "seconds": {"preflight": 0.0, "index": 0.0}, "diagnostics": []}
    try:
        record["commit"] = run(["git", "rev-parse", "HEAD"], tree, environment)
        record["dirty"] = bool(run(["git", "status", "--porcelain", "--untracked-files=no"],
                                   tree, environment))
        record["nsh_rev"] = pins.nsh_revision(tree)
        problem = pins.check_nsh(nsh_checkout, tree, environment=environment)
        if problem:
            raise ValueError(problem)
        record["nsh_head"] = run(["git", "rev-parse", "HEAD"], nsh_checkout, environment)
        record["toolchain"] = pins.toolchain(tree)
        # A caller may have sourced locked-build-env.sh for another tree.
        environment["RUSTUP_TOOLCHAIN"] = record["toolchain"]
        before = time.monotonic()
        try:
            run(["cargo", "check", "--workspace", "--all-targets", "--locked",
                 "--target", TARGET], tree, environment)
        finally:
            record["seconds"]["preflight"] = time.monotonic() - before
        target = target_directory(tree, environment)
        for stamp in (target / "deslop-please").glob("*/index.stamp"):
            stamp.unlink()
        real_ra = run(["rustup", "which", "rust-analyzer"], tree, environment)
        record["rust_analyzer"] = run([real_ra, "--version"], tree, environment)
        record["deslop"] = run(["deslop-please", "--version"], tree, environment)
        # Keep the command's bytes (including its trailing newline) in the hash.
        cfg = subprocess.run(["rustc", "--print", "cfg", "--target", TARGET],
                             cwd=tree, env=environment, capture_output=True, check=True)
        record["cfg"] = hashlib.sha256(cfg.stdout).hexdigest()
        with tempfile.TemporaryDirectory(prefix="patina-deslop-ra-") as directory:
            write_wrapper(directory)
            log = Path(directory) / "analyzer.log"
            wrapped = {**environment, "PATH": directory + os.pathsep + environment.get("PATH", ""),
                       "DESLOP_REAL_RA": real_ra, "DESLOP_RA_LOG": str(log)}
            before = time.monotonic()
            try:
                result = subprocess.run(["deslop-please", "--json", "--target", TARGET],
                                        cwd=tree, env=wrapped, capture_output=True, text=True)
            finally:
                record["seconds"]["index"] = time.monotonic() - before
            if result.returncode not in (0, 1):
                raise ValueError(f"deslop-please exited {result.returncode}:\n{result.stderr}")
            record["diagnostics"] = parse_tool_output(result.stdout)
            record["problems"].extend(index_health(log.read_text() if log.exists() else ""))
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        record["problems"].append(str(error))
    record["seconds"]["total"] = time.monotonic() - started
    record["valid"] = not record["problems"]
    return record


def scope(item):
    if item["lint"] == "module-cycle":
        found = re.search(r"\bin `([^`]+)`", item["message"])
        if found:
            return found[1]
    return item["key"].split("::", 1)[0]


def count_table(reference, head=None):
    old = Counter(item["lint"] for item in reference["diagnostics"])
    new = Counter(item["lint"] for item in head["diagnostics"]) if head is not None else None
    rows = ["| Lint | Reference → HEAD |" if new is not None else "| Lint | Findings |",
            "| --- | ---: |"]
    for lint in sorted(old.keys() | (new.keys() if new is not None else set())):
        value = f"{old[lint]} → {new[lint]}" if new is not None else str(old[lint])
        rows.append(f"| {lint} | {value} |")
    total = f"{sum(old.values())} → {sum(new.values())}" if new is not None else str(sum(old.values()))
    return "\n".join([*rows, f"| **Total** | **{total}** |"])


def member_names(item):
    prefix = scope(item) + "::"
    return [member.removeprefix(prefix) if item["lint"] == "module-cycle" else member
            for member in item["key"].split(",")]


def cycle_summary(item):
    members = member_names(item)
    rest = f", +{len(members) - 10} more" if len(members) > 10 else ""
    unit = "modules" if item["lint"] == "module-cycle" else "items"
    return f"{scope(item)}: {len(members)} {unit} — {', '.join(members[:10])}{rest}"


def size(item):
    patterns = {"field-leak": r"(\d+) places in (\d+) modules",
                "shallow-module": r"shows (\d+) of its (\d+) functions",
                "long-file": r"has (\d+) lines",
                "wide-directory": r"holds (\d+) files and directories"}
    found = re.search(patterns.get(item["lint"], r"(?!)"), item["message"])
    return tuple(map(int, found.groups())) if found else item["message"]


def structural_diff(reference, head):
    sections = {name: defaultdict(list) for name in ("New", "Fixed", "Changed")}
    old = {(item["lint"], item["key"]): item for item in reference["diagnostics"]}
    new = {(item["lint"], item["key"]): item for item in head["diagnostics"]}
    for lint in sorted(CYCLES):
        groups = {scope(item) for item in [*old.values(), *new.values()] if item["lint"] == lint}
        for group in sorted(groups):
            left = [item for item in old.values() if item["lint"] == lint and scope(item) == group]
            right = [item for item in new.values() if item["lint"] == lint and scope(item) == group]
            candidates = []
            for before in left:
                for after in right:
                    overlap = len(set(before["key"].split(",")) & set(after["key"].split(",")))
                    if overlap:
                        candidates.append((-overlap, before["key"], after["key"]))
            paired_old, paired_new = set(), set()
            for _, before_key, after_key in sorted(candidates):
                if before_key in paired_old or after_key in paired_new:
                    continue
                paired_old.add(before_key)
                paired_new.add(after_key)
                if before_key == after_key:
                    continue
                before, after = old[lint, before_key], new[lint, after_key]
                unit = "modules" if lint == "module-cycle" else "items"
                lines = [f"{group}: {len(before_key.split(','))} → {len(after_key.split(','))} {unit}"]
                for marker, members in (("−", sorted(set(member_names(before)) - set(member_names(after)))),
                                        ("+", sorted(set(member_names(after)) - set(member_names(before))))):
                    if members:
                        lines.append(textwrap.fill(", ".join(members), width=100,
                                                   initial_indent=f"  {marker} ", subsequent_indent="    "))
                sections["Changed"][lint].append((before_key, "\n".join(lines)))
            for items, paired, section in ((left, paired_old, "Fixed"), (right, paired_new, "New")):
                for item in items:
                    if item["key"] not in paired:
                        sections[section][lint].append((item["key"], cycle_summary(item)))
    for identity in sorted(old.keys() | new.keys()):
        lint, key = identity
        if lint in CYCLES:
            continue
        if identity not in old:
            sections["New"][lint].append((key, f"{key}: {new[identity]['message']}"))
        elif identity not in new:
            sections["Fixed"][lint].append((key, f"{key}: {old[identity]['message']}"))
        elif size(old[identity]) != size(new[identity]):
            sections["Changed"][lint].append((key, f"{key}: {old[identity]['message']} → {new[identity]['message']}"))
    output = [count_table(reference, head)]
    for name, groups in sections.items():
        if groups:
            output.append(f"## {name}")
            for lint in sorted(groups, key=lambda lint: (lint not in CYCLES, lint)):
                output.append(f"### {lint}")
                for _, value in sorted(groups[lint]):
                    output.append(value)
    if not any(sections.values()):
        output.append("No structural changes.")
    return "\n\n".join(output)


def comparison(reference, head, merge_base, source):
    validate_record(reference)
    validate_record(head)
    output = [f"HEAD: `{head['commit']}`{' (dirty)' if head['dirty'] else ''}",
              f"Merge base: `{merge_base}`", f"Reference: `{reference['commit']}` ({source})",
              f"Reference valid: {reference['valid']}; HEAD valid: {head['valid']}"]
    if not reference["valid"] or not head["valid"]:
        for name, record in (("Reference", reference), ("HEAD", head)):
            output.extend(f"{name}: {problem}" for problem in record["problems"])
        output.append("Index not valid; no diff.")
    else:
        mismatches = [f"{field}: reference={reference.get(field)!r}, HEAD={head.get(field)!r}"
                      for field in INPUTS if reference.get(field) != head.get(field)]
        if mismatches:
            output.extend(["Inputs differ; no diff.", *mismatches])
        else:
            output.append(structural_diff(reference, head))
    return "\n\n".join(output)


def read_record(path):
    return validate_record(json.loads(Path(path).read_text()))


def downloaded_reference(tree, merge_base, directory):
    try:
        runs = json.loads(run(["gh", "run", "list", "--workflow", "deslop.yml", "--commit", merge_base,
                               "--status", "success", "--json", "databaseId", "--limit", "1"], tree))
        if not runs:
            print(f"No successful deslop.yml run at {merge_base}; use --base to index it locally.")
            return None
        identifier = str(runs[0]["databaseId"])
        run(["gh", "run", "download", identifier, "--name", "deslop-report", "--dir", directory], tree)
        record = read_record(Path(directory) / "report.json")
        if record["commit"] != merge_base:
            raise ValueError(f"artifact commit {record['commit']} is not merge base {merge_base}")
        return record, f"deslop-report artifact, run {identifier}"
    except (OSError, RuntimeError, ValueError, KeyError, TypeError) as error:
        print(f"No usable reference: {error}\nUse --base to index the merge base locally.")
        return None


def base_reference(tree, nsh_checkout, merge_base):
    environment = clean_environment()
    environment["RUSTUP_TOOLCHAIN"] = pins.toolchain(tree)
    target = target_directory(tree, environment) / "deslop-base"
    with tempfile.TemporaryDirectory(prefix="patina-deslop-base-") as directory:
        worktree = Path(directory) / "wt"
        run(["git", "worktree", "add", "--detach", worktree, merge_base], tree)
        try:
            base_pin, head_pin = pins.nsh_revision(worktree), pins.nsh_revision(tree)
            if base_pin != head_pin:
                # Give persistent preparation commands, not the temporary path
                # that the finally block is about to remove.
                checkout = Path(tree).parent / "nsh-deslop-base"
                print(f"NSH pins differ: base={base_pin}, HEAD={head_pin}; --base cannot continue.")
                print("Prepare a separate adjacent checkout for the base pin with:")
                print(f"git clone https://github.com/teng-lin/nsh.git {shlex.quote(str(checkout))}")
                print(f"git -C {shlex.quote(str(checkout))} checkout --detach {base_pin}")
                return None
            (Path(directory) / "nsh-patina").symlink_to(Path(nsh_checkout).resolve(), target_is_directory=True)
            environment["RUSTUP_TOOLCHAIN"] = pins.toolchain(worktree)
            try:
                run(["rustup", "which", "rust-analyzer"], worktree, environment)
            except (RuntimeError, OSError):
                print("Base analyzer unavailable; --base cannot continue. Install it with:")
                print(f"rustup component add rust-analyzer --toolchain {pins.toolchain(worktree)}")
                return None
            return measure(worktree, Path(directory) / "nsh-patina", target_dir=target), "local merge-base worktree"
        finally:
            try:
                run(["git", "worktree", "remove", "--force", worktree], tree)
            finally:
                run(["git", "worktree", "prune"], tree)


def print_timings(name, record):
    seconds = record["seconds"]
    total = seconds.get("total", seconds["preflight"] + seconds["index"])
    print(f"{name} seconds: preflight={seconds['preflight']:.2f}, index={seconds['index']:.2f}, total={total:.2f}")


def main(arguments=None, *, tree=ROOT):
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--base", action="store_true")
    group.add_argument("--reference", type=Path)
    group.add_argument("--write", type=Path)
    parser.add_argument("--upstream", default="origin/master")
    args = parser.parse_args(arguments)
    tree = Path(tree)
    nsh_checkout = tree.parent / "nsh-patina"
    started = time.monotonic()
    try:
        if args.write:
            head = measure(tree, nsh_checkout)
            if head["dirty"] or not head["valid"]:
                print("Cannot write report: tree is dirty or index not valid.")
                for problem in head["problems"]:
                    print(problem)
                return 1
            validate_record(head)
            args.write.parent.mkdir(parents=True, exist_ok=True)
            args.write.write_text(json.dumps(head, indent=2) + "\n")
            print(count_table(head))
            print_timings("HEAD", head)
            return 0
        head_commit = run(["git", "rev-parse", "HEAD"], tree)
        upstream = run(["git", "rev-parse", args.upstream], tree)
        print(f"Upstream {args.upstream}: `{upstream}`")
        timestamp = int(run(["git", "show", "-s", "--format=%ct", args.upstream], tree))
        if time.time() - timestamp > 86400:
            print(f"Upstream commit is older than a day; consider git fetch before comparing {args.upstream}.")
        merge_base = run(["git", "merge-base", "HEAD", args.upstream], tree)
        if not args.base and not args.reference and head_commit == merge_base:
            print("HEAD is on master: nothing to compare")
            return 0
        if args.reference:
            reference, source = read_record(args.reference), str(args.reference)
            if reference["commit"] != merge_base:
                print(f"Warning: reference commit {reference['commit']} is not merge base {merge_base}; consider --base.")
        elif args.base:
            result = base_reference(tree, nsh_checkout, merge_base)
            if result is None:
                return 0
            reference, source = result
        else:
            with tempfile.TemporaryDirectory(prefix="patina-deslop-reference-") as directory:
                result = downloaded_reference(tree, merge_base, directory)
            if result is None:
                return 0
            reference, source = result
        head = measure(tree, nsh_checkout)
        print(comparison(reference, head, merge_base, source))
        print_timings("Reference", reference)
        print_timings("HEAD", head)
        print(f"Total comparison seconds: {time.monotonic() - started:.2f}")
    except (OSError, ValueError, RuntimeError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(f"Structure report unavailable: {error}\nNo diff.")
        return 1 if args.write else 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
