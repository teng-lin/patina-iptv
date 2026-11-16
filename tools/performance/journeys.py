#!/usr/bin/env python3
"""Synthetic headless journeys, strict evidence parsing and base-policy comparison."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
CASES = ("cached-guide", "busy-guide", "selection")

TERMINALS = {"guide-mounted", "navigation-mounted", "acceptance-proxy", "ready-proxy", "size-proxy", "first-render"}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def load(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def natural(value):
    return type(value) is int and value >= 0


def evidence(sample, terminal, timeout_us):
    """Return duration only after validating every event, including stale extras."""
    if not isinstance(sample, dict):
        raise ValueError("sample must be an object")
    if sample.get("failure"):
        raise ValueError(sample["failure"])
    events = sample.get("events", [])
    if not isinstance(events, list) or not all(isinstance(e, dict) for e in events):
        raise ValueError("events must be an array of objects")
    starts = [e for e in events if e.get("kind") == "start"]
    ends = [e for e in events if e.get("kind") in TERMINALS]
    if len(starts) != 1 or len(ends) != 1:
        raise ValueError("missing or duplicate start/terminal evidence")
    start, end = starts[0], ends[0]
    if end["kind"] != terminal:
        raise ValueError(f"missing {terminal}; got {end['kind']}")
    identity = start.get("selection")
    if not natural(identity) or any(e.get("selection") != identity for e in events):
        raise ValueError("stale selection evidence")
    times = [e.get("at_us") for e in events]
    if not all(natural(t) for t in times) or times != sorted(times):
        raise ValueError("missing or non-monotonic event time")
    if events.index(start) >= events.index(end) or end["at_us"] < start["at_us"]:
        raise ValueError("terminal occurred before start")
    duration = end["at_us"] - start["at_us"]
    if duration > timeout_us:
        raise ValueError("journey timeout")
    if terminal in {"acceptance-proxy", "ready-proxy", "size-proxy", "first-render"}:
        if identity == 0 or not natural(end.get("token")) or end["token"] == 0:
            raise ValueError("missing selected source token")
        if start.get("token") != end["token"]:
            raise ValueError("stale source token evidence")
    counters = sample.get("counters", {})
    if not isinstance(counters, dict) or set(counters) != {"store_reads", "player_opens"} or not all(natural(v) for v in counters.values()):
        raise ValueError("missing or invalid work counters")
    return duration


def summary(samples, spec, baseline, repetitions, timeout_us, commit=None):
    result = {}
    errors = []
    if repetitions < 1:
        errors.append("zero samples requested")
    if set(baseline.get("journeys", {})) != set(CASES):
        errors.append("baseline journey set changed")
    for journey in CASES:
        selected = [s for s in samples if s.get("journey") == journey]
        values = []
        if len(selected) != repetitions or not selected:
            errors.append(f"{journey}: expected {repetitions} samples, got {len(selected)}")
        for index, sample in enumerate(selected):
            try:
                if sample.get("failure"):
                    raise ValueError(sample["failure"])
                if commit is not None:
                    if sample.get("build_commit") != commit[:8] or sample.get("build_mode") != "debug-test-support":
                        raise ValueError("stale executable commit or unexpected build mode")
                elapsed = evidence(sample, spec[journey]["terminal"], timeout_us)
                ceilings = baseline["journeys"][journey]
                if set(ceilings) != {"store_reads", "player_opens"}:
                    raise ValueError("baseline counter set changed")
                for counter, maximum in ceilings.items():
                    if not natural(maximum):
                        raise ValueError(f"invalid ceiling for {counter}")
                    value = sample["counters"][counter]
                    if value > maximum:
                        raise ValueError(f"{counter} regressed: {value} > {maximum}")
                values.append(elapsed)
            except (ValueError, KeyError, TypeError) as error:
                errors.append(f"{journey} sample {index + 1}: {error}")
        values.sort()
        result[journey] = {
            "sample_count": len(selected), "valid_evidence_count": len(values),
            "median_us": (values[(len(values) - 1) // 2] + values[len(values) // 2]) / 2 if values else None,
            "p95_us": values[math.ceil(len(values) * .95) - 1] if values else None,
            "max_us": max(values) if values else None,
            "terminal": spec[journey]["terminal"],
        }
    if any(s.get("journey") not in CASES for s in samples):
        errors.append("unknown journey evidence")
    return {"valid": not errors, "errors": errors, "journeys": result}


def policy(base, candidate, records, base_spec=None, candidate_spec=None):
    """Every removed/increased ceiling is weakening, even with a new name."""
    if base.get("schema") != 1 or candidate.get("schema") != 1:
        raise ValueError("unsupported baseline schema")
    weakened = []
    for journey, counters in base["journeys"].items():
        for counter, value in counters.items():
            new = candidate["journeys"].get(journey, {}).get(counter)
            if new is None or not natural(new) or new > value:
                weakened.append(f"{journey}.{counter}: {value} -> {new}")
    fixture_changed = base_spec is not None and base_spec != candidate_spec
    if fixture_changed:
        weakened.append("fixture specification or endpoint changed")
    if weakened:
        before = digest(json.dumps(base, sort_keys=True).encode())
        after = digest(json.dumps(candidate, sort_keys=True).encode())
        explained = any(r.get("base_sha256") == before and r.get("candidate_sha256") == after
                        and isinstance(r.get("rationale"), str) and len(r["rationale"].strip()) >= 30
                        and (not fixture_changed or
                             (r.get("fixture_base_sha256") == digest(json.dumps(base_spec, sort_keys=True).encode())
                              and r.get("fixture_candidate_sha256") == digest(json.dumps(candidate_spec, sort_keys=True).encode())))
                        for r in records)
        if not explained:
            raise ValueError("baseline weakening requires a new base-specific rationale: " + "; ".join(weakened))
    return weakened


def check_report(report, baseline):
    """Re-parse a persisted report; stored validity/summary cannot mask lost samples."""
    if not isinstance(report, dict) or not isinstance(baseline, dict):
        raise ValueError("report and baseline must be objects")
    if not isinstance(report.get("samples"), list) or not all(isinstance(s, dict) for s in report["samples"]):
        raise ValueError("samples must be an array of objects")
    if report.get("dirty") is not False:
        raise ValueError("dirty or unknown source state cannot establish a reproducible report")
    if report.get("schema") != 1 or baseline.get("schema") != 1:
        raise ValueError("unsupported report/baseline schema")
    if not re.fullmatch(r"[0-9a-f]{40}", report.get("commit", "")):
        raise ValueError("missing commit identity")
    for key in ("platform", "backend", "hardware_class", "build_mode"):
        if not isinstance(report.get(key), str) or not report[key]:
            raise ValueError(f"missing {key}")
    if report.get("fixture_sha256") != digest((HERE / "fixtures.json").read_bytes()):
        raise ValueError("fixture hash does not match the workload specification")
    if report.get("fixture") != load(HERE / "fixtures.json"):
        raise ValueError("fixture specification mismatch")
    if not natural(report.get("repetitions")) or not natural(report.get("timeout_us")) or report["timeout_us"] == 0:
        raise ValueError("invalid repetition count or deadline")
    result = summary(report.get("samples", []), report["fixture"], baseline,
                     report["repetitions"], report["timeout_us"], report["commit"])
    return result


def build_harness(target="app"):
    built = subprocess.run(["cargo", "test", "--locked", "-p", "iptv-app", "--features", "test-support",
                            "--test", target, "--no-run", "--message-format=json"],
                           cwd=ROOT, text=True, stdout=subprocess.PIPE, check=True)
    binaries = []
    for line in built.stdout.splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-artifact" and item.get("executable") and item["target"]["name"] == target:
            binaries.append(item["executable"])
    if len(binaries) != 1:
        raise ValueError("expected one app test executable")
    return binaries[0]


def scenario(command, env, timeout):
    """Timeouts terminate the isolated scenario tree, including its Xvfb display."""
    import os
    import signal
    process = subprocess.Popen(command, env=env, cwd=ROOT, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, start_new_session=True)
    try:
        stdout, _ = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        if os.name == "posix":
            os.killpg(process.pid, signal.SIGKILL)
        else:
            process.kill()
        process.communicate()
        raise
    return subprocess.CompletedProcess(command, process.returncode, stdout)


def update_report(report, baseline):
    result = summary(report["samples"], report["fixture"], baseline, report["repetitions"],
                     report["timeout_us"], report["commit"])
    if report["dirty"]:
        result["valid"] = False
        result["errors"].append("dirty source state cannot establish a reproducible report")
    report.update(result)


def run(args):
    import os
    import shutil
    spec = load(HERE / "fixtures.json")
    baseline = load(args.baseline)
    report = {
        "schema": 1,
        "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "dirty": subprocess.run(["git", "diff", "--quiet", "HEAD"], cwd=ROOT).returncode != 0,
        "platform": platform.system().lower(), "backend": "nsh-headless+gtk-app-mpv-render-submission",
        "hardware_class": "host", "build_mode": "debug-test-support", "fixture_sha256": digest((HERE / "fixtures.json").read_bytes()),
        "fixture": spec, "repetitions": args.repetitions, "timeout_us": args.timeout * 1_000_000,
        "samples": [], "valid": False, "errors": ["runner did not complete"],
    }
    output = Path(args.output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    try:
        if args.repetitions < 1 or args.timeout < 1:
            raise ValueError("repetitions and timeout must be positive")
        harness = args.harness or build_harness()
        with tempfile.TemporaryDirectory(prefix="patina-journeys-") as temporary:
            # Immutable executable: another worktree's build cannot change this run.
            executable = Path(temporary) / "app-tests"
            shutil.copy2(harness, executable)
            native = None
            if platform.system() == "Linux":
                native = Path(temporary) / "native-tests"
                shutil.copy2(args.native_harness or build_harness("native_player"), native)
            for journey in CASES:
                case = spec[journey]["scenario"]
                for repetition in range(args.repetitions):
                    destination = Path(temporary) / f"{journey}-{repetition}.json"
                    scratch = Path(temporary) / f"scratch-{journey}-{repetition}"
                    scratch.mkdir()
                    env = {**os.environ, "IPTV_PERF_SAMPLE": str(destination), "IPTV_RS_TEST_JOBS": "1", "TMPDIR": str(scratch)}
                    try:
                        if journey == "selection":
                            if native is None:
                                report["samples"].append({"journey": journey, "failure": "missing correlated native first-render backend"})
                                continue
                            command = [sys.executable, str(ROOT / "tools/ci/run-xvfb.py"), str(native), case,
                                       "--ignored", "--exact", "--nocapture", "--test-threads=1"]
                        else:
                            command = [str(executable), case, "--ignored" if journey == "busy-guide" else "--include-ignored",
                                       "--exact", "--nocapture", "--test-threads=1"]
                        process = scenario(command, env, args.timeout)
                        sample = load(destination) if destination.exists() else {"journey": journey, "failure": "missing terminal evidence"}
                        if not isinstance(sample, dict):
                            raise ValueError("sample file must contain an object")
                        if process.returncode:
                            sample["failure"] = f"scenario failed (exit {process.returncode})"
                            print(process.stdout, file=sys.stderr)
                    except subprocess.TimeoutExpired:
                        sample = {"journey": journey, "failure": "scenario timeout"}
                    except (ValueError, OSError, TypeError) as error:
                        sample = {"journey": journey, "failure": f"unreadable evidence: {error}"}
                    report["samples"].append(sample)
                    update_report(report, baseline)
                    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        update_report(report, baseline)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        report["valid"] = False
        report["errors"] = [str(error)]
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"valid": report["valid"], "errors": report["errors"], "report": str(output)}))
    return 0 if report["valid"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    runner = commands.add_parser("run")
    runner.add_argument("--output", default="target/performance/summary.json")
    runner.add_argument("--harness")
    runner.add_argument("--native-harness")
    runner.add_argument("--repetitions", type=int, default=3)
    runner.add_argument("--timeout", type=int, default=300)
    runner.add_argument("--baseline", default=str(HERE / "baseline.json"))
    checker = commands.add_parser("check")
    checker.add_argument("report")
    checker.add_argument("--baseline", default=str(HERE / "baseline.json"))
    compare = commands.add_parser("policy")
    compare.add_argument("base")
    compare.add_argument("--candidate", default=str(HERE / "baseline.json"))
    compare.add_argument("--records", default=str(HERE / "changes.json"))
    compare.add_argument("--base-fixtures")
    compare.add_argument("--candidate-fixtures", default=str(HERE / "fixtures.json"))
    compare.add_argument("--report")
    args = parser.parse_args()
    if args.command == "run":
        return run(args)
    try:
        if args.command == "check":
            result = check_report(load(args.report), load(args.baseline))
            print(json.dumps(result))
            return 0 if result["valid"] else 1
        print(json.dumps({"weakened": policy(load(args.base), load(args.candidate), load(args.records),
                                              load(args.base_fixtures) if args.base_fixtures else None,
                                              load(args.candidate_fixtures) if args.base_fixtures else None)}))
        return 0
    except (ValueError, OSError, KeyError, TypeError) as error:
        if args.command == "policy" and args.report:
            Path(args.report).write_text(json.dumps({"schema": 1, "valid": False, "errors": [str(error)]}) + "\n", encoding="utf-8")
        print(str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
