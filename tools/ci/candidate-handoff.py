#!/usr/bin/env python3
"""Isolated Windows pilot: prove candidate inputs and bytes before staging an exe."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tempfile
import tomllib
import zipfile
from datetime import datetime

ROOT = Path(__file__).resolve().parents[2]
REPOSITORY = "teng-lin/patina-iptv"
BRANCH = "ci/candidate-artifact-handoff-336"
WORKFLOW = ".github/workflows/candidate-handoff-pilot.yml"
PRODUCER = "Pilot producer"
FILES = {"manifest.json", "iptv-app.exe"}
MAX_EXE = 512 * 1024 * 1024
SHA = re.compile(r"[a-f0-9]{64}")


def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def encoded(data):
    return json.dumps(data, sort_keys=True, separators=(",", ":")).encode()


def write_json(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(encoded(data) + b"\n")


def run(args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def gh(endpoint):
    # Errors remain errors; a 404 is never silently treated as a cache miss.
    return json.loads(run(["gh", "api", endpoint]))


def pages(endpoint, key, api=gh):
    result = []
    for page in range(1, 1001):
        data = api(f"{endpoint}{'&' if '?' in endpoint else '?'}per_page=100&page={page}")
        rows = data[key]
        if not isinstance(rows, list):
            raise ValueError("Invalid paginated API response")
        result.extend(rows)
        if len(rows) < 100:
            return result
    raise ValueError("API pagination exceeded pilot bound")


def positive(value):
    if not re.fullmatch(r"[1-9][0-9]*", str(value)):
        raise ValueError("Invalid GitHub identifier")
    return int(value)


def checksum(value):
    if not isinstance(value, str) or not SHA.fullmatch(value):
        raise ValueError("Invalid SHA-256")
    return value


def tree_digest(directory):
    directory = Path(directory)
    if not directory.is_dir() or directory.is_symlink():
        raise ValueError("Native input directory missing or linked")
    value, count = hashlib.sha256(), 0
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError("Native toolchain links are unsupported in this pilot")
        if path.is_file():
            value.update(encoded([path.relative_to(directory).as_posix(), digest(path)]))
            count += 1
    if not count:
        raise ValueError("Empty native input tree")
    return {"sha256": value.hexdigest(), "files": count}


def validate_environment(environment):
    """Reject build overrides without recording values or unrelated secrets."""
    prefixes = ("CC_", "CXX_", "AR_", "LD_", "OPENSSL_", "DEP_", "BINDGEN_", "PKG_CONFIG", "CMAKE_", "SQLITE", "LIBSQLITE")
    cargo_allowed = {"CARGO_HOME", "CARGO_TERM_COLOR", "CARGO_NET_GIT_FETCH_WITH_CLI"}
    forbidden = {
        "RUSTFLAGS",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTDOCFLAGS",
        "CC",
        "CXX",
        "AR",
        "LD",
        "CFLAGS",
        "CXXFLAGS",
        "CPPFLAGS",
        "LDFLAGS",
        "CL",
        "_CL_",
        "LINK",
        "_LINK_",
        "LIB",
        "LIBPATH",
        "INCLUDE",
        "VCINSTALLDIR",
        "VCTOOLSINSTALLDIR",
        "WINDOWSSDKDIR",
        "WINDOWSSDKVERSION",
        "SOURCE_DATE_EPOCH",
        "CARGO",
        "MAKEFLAGS",
        "NUM_JOBS",
    }
    for name, value in environment.items():
        name = name.upper()
        if value and (name in forbidden or name.startswith(prefixes) or (name.startswith("CARGO_") and name not in cargo_allowed)):
            raise ValueError("Unsupported build environment override")


def inputs(root=ROOT):
    if os.name != "nt":
        raise ValueError("Pilot input capture requires native Windows")
    # The shipping script owns target/release. Do not let Cargo or native build
    # overrides silently redirect it or change the release recipe.
    validate_environment(os.environ)
    if run(["git", "status", "--porcelain", "--untracked-files=all"], root):
        raise ValueError("Pilot requires a clean checkout")
    nsh = root.parent / "nsh-patina"
    pin = (root / "nsh.rev").read_text().strip()
    if run(["git", "rev-parse", "HEAD"], nsh) != pin or run(["git", "status", "--porcelain"], nsh):
        raise ValueError("NSH differs from the exact clean pin")
    configs = []
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    for directory in (root, *root.parents, cargo_home):
        for filename in ("config", "config.toml") if directory == cargo_home else (".cargo/config", ".cargo/config.toml"):
            path = directory / filename
            if path.is_file():
                if path != root / ".cargo/config.toml":
                    raise ValueError("Unsupported ambient Cargo configuration")
                configs.append(digest(path))
    native = json.loads(run(["pwsh", "-NoProfile", "-File", str(root / "tools/ci/candidate-handoff.ps1"), "-Phase", "Inputs"], root))
    native["trees"] = {name: tree_digest(path) for name, path in native.pop("trees").items()}
    native["tools"] = {name: {"sha256": digest(path), "name": Path(path).name} for name, path in native.pop("tools").items()}
    rust_sysroot = Path(run(["rustc", "--print", "sysroot"], root))
    rustc = run(["rustc", "-vV"], root)
    channel = tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    if rustc.splitlines()[0].split()[1] != channel:
        raise ValueError("Compiler differs from the exact Rust pin")
    if "host: x86_64-pc-windows-msvc" not in rustc.splitlines():
        raise ValueError("Unexpected Rust host target")
    runtime = root / "target/mpv"
    tracked = run(["git", "ls-files"], root).splitlines()
    source = hashlib.sha256(encoded({name: digest(root / name) for name in tracked})).hexdigest()
    return {
        "schema": 1,
        "commit": run(["git", "rev-parse", "HEAD"], root),
        "source_sha256": source,
        "nsh": pin,
        "nsh_tree": run(["git", "rev-parse", "HEAD^{tree}"], nsh),
        "cargo_configs": configs,
        "recipe": ["tools/build-windows.ps1", "-Release"],
        "profile": "release",
        "features": "default; no test-support",
        "target": "x86_64-pc-windows-msvc",
        "version": tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"],
        "rustc": rustc,
        "cargo": run(["cargo", "-Vv"], root),
        "rust_toolchain": tree_digest(rust_sysroot / "lib"),
        "rustc_sha256": digest(rust_sysroot / "bin/rustc.exe"),
        "cargo_sha256": digest(rust_sysroot / "bin/cargo.exe"),
        "locks": {name: digest(root / name) for name in ("Cargo.lock", "tools/ci/package.Cargo.lock", "nsh.rev", "rust-toolchain.toml")},
        "runtime": {name: digest(runtime / name) for name in ("libmpv-2.dll", "components.json")},
        "native": native,
    }


def seal(expected, output, run_id, attempt, api=gh, root=ROOT):
    run_id, attempt = positive(run_id), positive(attempt)
    jobs = pages(f"repos/{REPOSITORY}/actions/runs/{run_id}/attempts/{attempt}/jobs", "jobs", api)
    jobs = [job for job in jobs if job.get("name") == PRODUCER and job.get("run_id") == run_id and job.get("run_attempt") == attempt]
    if len(jobs) != 1:
        raise ValueError("Producer job identity is ambiguous")
    identity = {"run_id": run_id, "attempt": attempt, "job_id": positive(jobs[0]["id"]), "repository": REPOSITORY, "workflow": WORKFLOW, "commit": expected["commit"]}
    output.mkdir(parents=True, exist_ok=False)
    shutil.copyfile(root / "target/release/iptv-app.exe", output / "iptv-app.exe")
    manifest = {"schema": 1, "inputs": expected, "producer": identity, "file": {"name": "iptv-app.exe", "sha256": digest(output / "iptv-app.exe"), "size": (output / "iptv-app.exe").stat().st_size}}
    write_json(output / "manifest.json", manifest)
    return {
        "run_id": str(run_id),
        "attempt": str(attempt),
        "job_id": str(identity["job_id"]),
        "inputs_sha256": hashlib.sha256(encoded(expected)).hexdigest(),
        "manifest_sha256": digest(output / "manifest.json"),
        "file_sha256": manifest["file"]["sha256"],
    }


def validate_producer(proof, expected, api):
    run_id, attempt, job_id = (positive(proof[key]) for key in ("run_id", "attempt", "job_id"))
    if checksum(proof["inputs_sha256"]) != hashlib.sha256(encoded(expected)).hexdigest():
        raise ValueError("Independent consumer inputs differ from producer")
    for key in ("manifest_sha256", "file_sha256", "artifact_digest"):
        checksum(proof[key])
    positive(proof["artifact_id"])
    observed = api(f"repos/{REPOSITORY}/actions/runs/{run_id}")
    if (
        observed.get("head_sha") != expected["commit"]
        or observed.get("head_branch") != BRANCH
        or observed.get("path") != WORKFLOW
        or observed.get("event") not in ("push", "workflow_dispatch")
        or observed.get("repository", {}).get("full_name") != REPOSITORY
        or observed.get("head_repository", {}).get("full_name") != REPOSITORY
    ):
        raise ValueError("Untrusted producer workflow run")
    job = api(f"repos/{REPOSITORY}/actions/jobs/{job_id}")
    if (
        job.get("run_id") != run_id
        or job.get("run_attempt") != attempt
        or job.get("name") != PRODUCER
        or job.get("head_sha") != expected["commit"]
        or job.get("status") != "completed"
        or job.get("conclusion") != "success"
    ):
        raise ValueError("Producer job did not complete successfully at the bound attempt")
    return run_id


def select_artifact(proof, expected, api=gh):
    run_id = validate_producer(proof, expected, api)
    rows = pages(f"repos/{REPOSITORY}/actions/runs/{run_id}/artifacts", "artifacts", api)
    name = f"pilot336-exe-{run_id}-{positive(proof['attempt'])}"
    matching = [row for row in rows if row.get("id") == positive(proof["artifact_id"]) or row.get("name") == name]
    if not matching:
        return "absent"
    if len(matching) != 1:
        raise ValueError("Artifact identity is ambiguous")
    row = matching[0]
    if (
        row.get("id") != positive(proof["artifact_id"])
        or row.get("name") != name
        or row.get("workflow_run", {}).get("id") != run_id
        or row.get("workflow_run", {}).get("head_sha") != expected["commit"]
        or row.get("digest") != "sha256:" + proof["artifact_digest"]
        or type(row.get("expired")) is not bool
    ):
        raise ValueError("Artifact metadata differs from trusted producer outputs")
    return "expired" if row["expired"] else "reuse"


def unpack(archive, expected, proof, destination):
    if destination.exists():
        raise ValueError("Consumer release destination is not fresh")
    if digest(archive) != checksum(proof["artifact_digest"]):
        raise ValueError("Artifact archive checksum mismatch")
    with zipfile.ZipFile(archive) as incoming:
        entries = incoming.infolist()
        if len(entries) != 2 or {item.filename for item in entries} != FILES:
            raise ValueError("Unexpected, duplicate or missing artifact members")
        for item in entries:
            mode = item.external_attr >> 16
            if (
                item.is_dir()
                or stat.S_ISLNK(mode)
                or (stat.S_IFMT(mode) not in (0, stat.S_IFREG))
                or item.flag_bits & 1
                or item.file_size < 1
                or item.file_size > (MAX_EXE if item.filename == "iptv-app.exe" else 2 * 1024 * 1024)
            ):
                raise ValueError("Unsafe artifact member")
        raw = incoming.read("manifest.json")
        if hashlib.sha256(raw).hexdigest() != checksum(proof["manifest_sha256"]):
            raise ValueError("Manifest checksum mismatch")
        manifest = json.loads(raw)
        identity = {
            "run_id": positive(proof["run_id"]),
            "attempt": positive(proof["attempt"]),
            "job_id": positive(proof["job_id"]),
            "repository": REPOSITORY,
            "workflow": WORKFLOW,
            "commit": expected["commit"],
        }
        file = manifest.get("file", {})
        if (
            manifest.get("schema") != 1
            or manifest.get("inputs") != expected
            or manifest.get("producer") != identity
            or file.get("name") != "iptv-app.exe"
            or file.get("sha256") != checksum(proof["file_sha256"])
        ):
            raise ValueError("Manifest provenance mismatch")
        with tempfile.TemporaryDirectory(dir=destination.parent) as temporary:
            staged = Path(temporary) / "iptv-app.exe"
            with incoming.open("iptv-app.exe") as source, staged.open("wb") as target:
                shutil.copyfileobj(source, target)
            if staged.stat().st_size != file.get("size") or digest(staged) != proof["file_sha256"]:
                raise ValueError("Executable size or checksum mismatch")
            destination.mkdir()
            staged.replace(destination / staged.name)


def acquire(expected, proof, destination, api=gh, download=None, cold=False):
    if destination.exists():
        raise ValueError("Consumer release destination is not fresh")
    if cold:
        return "explicit-cold"
    decision = select_artifact(proof, expected, api)
    if decision != "reuse":
        return decision
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent) as temporary:
        archive = Path(temporary) / "candidate.zip"
        if download:
            download(positive(proof["artifact_id"]), archive)
        else:
            with archive.open("wb") as output:
                subprocess.run(["gh", "api", f"repos/{REPOSITORY}/actions/artifacts/{positive(proof['artifact_id'])}/zip"], stdout=output, check=True)
        unpack(archive, expected, proof, destination)
    return "reuse"


def measurements(runs):
    # Include failed/canceled runs explicitly; successful durations alone must
    # never hide reliability failures or be called a Validation speedup.
    values = sorted(item["seconds"] for item in runs if item["conclusion"] == "success")
    return {
        "n": len(runs),
        "successful_n": len(values),
        "failures": len(runs) - len(values),
        "p50_seconds": __import__("statistics").median(values) if values else None,
        "p90_seconds": values[math.ceil(0.9 * len(values)) - 1] if values else None,
    }


def summarize(timelines):
    """Aggregate actual queue-inclusive paths, never summed parallel steps."""
    samples = {"producer_consumer_path": [], "cold_control_path": [], "whole_pilot": []}
    segments = {}
    commits = {item["commit"] for item in timelines}
    if len(commits) > 1:
        raise ValueError("Measure each exact code revision as a separate cohort")
    if len({item.get("decision", "unavailable") for item in timelines}) > 1:
        raise ValueError("Measure reuse, cold and fallback as separate cohorts")
    if len({item["run_id"] for item in timelines}) != len(timelines):
        raise ValueError("Duplicate workflow run in measurement cohort")
    for item in timelines:
        latest = {}
        for job in item["jobs"]:
            old = latest.get(job["name"])
            if old is None or (job["attempt"], job["id"]) > (old["attempt"], old["id"]):
                latest[job["name"]] = job
        for name, members in (("producer_consumer_path", (PRODUCER, "Pilot consumer")), ("cold_control_path", ("Cold control",)), ("whole_pilot", (PRODUCER, "Pilot consumer", "Cold control"))):
            rows = [latest[member] for member in members if member in latest]
            success = len(rows) == len(members) and all(job["conclusion"] == "success" for job in rows)
            samples[name].append({"conclusion": "success" if success else "failure", "seconds": max((job["trigger_to_completion_seconds"] for job in rows), default=0)})
        for name, job in latest.items():
            for field in ("queue_seconds", "execution_seconds"):
                segments.setdefault(f"{name}/{field}", []).append({"conclusion": job["conclusion"], "seconds": job[field]})
    return {
        "schema": 1,
        "commit": next(iter(commits), None),
        "paths": {name: measurements(rows) for name, rows in samples.items()},
        "segments": {name: measurements(rows) for name, rows in segments.items()},
        "claim": "Contract/overhead experiment; whole-Validation improvement requires separate integrated evidence.",
    }


def timeline(run_id, api=gh):
    observed = api(f"repos/{REPOSITORY}/actions/runs/{positive(run_id)}")
    jobs = pages(f"repos/{REPOSITORY}/actions/runs/{positive(run_id)}/jobs?filter=all", "jobs", api)

    def seconds(first, last):
        return (datetime.fromisoformat(last.replace("Z", "+00:00")) - datetime.fromisoformat(first.replace("Z", "+00:00"))).total_seconds()

    rows = []
    for job in jobs:
        if job.get("name") not in (PRODUCER, "Pilot consumer", "Cold control") or not job.get("completed_at"):
            continue
        rows.append(
            {
                "id": job["id"],
                "name": job["name"],
                "attempt": job["run_attempt"],
                "conclusion": job["conclusion"],
                "created_at": job["created_at"],
                "started_at": job["started_at"],
                "completed_at": job["completed_at"],
                "queue_seconds": seconds(job["created_at"], job["started_at"] or job["completed_at"]),
                "execution_seconds": seconds(job["started_at"], job["completed_at"]) if job["started_at"] else 0,
                "trigger_to_completion_seconds": seconds(observed["created_at"], job["completed_at"]),
                "steps": job.get("steps", []),
            }
        )
    return {
        "schema": 1,
        "run_id": positive(run_id),
        "commit": observed["head_sha"],
        "created_at": observed["created_at"],
        "decision": os.environ.get("PILOT_DECISION") or "unavailable",
        "jobs": rows,
        "claim": "Contract/overhead pilot only; no whole-Validation improvement measured.",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("inputs", "seal", "acquire", "timeline", "summarize"))
    parser.add_argument("output", type=Path)
    parser.add_argument("--cold", action="store_true")
    args = parser.parse_args()
    if args.command in ("inputs", "seal", "acquire"):
        if os.environ.get("GITHUB_REPOSITORY") != REPOSITORY or os.environ.get("GITHUB_REF") != "refs/heads/" + BRANCH:
            raise ValueError("Pilot is restricted to the exact trusted repository and branch")
    if args.command == "inputs":
        write_json(args.output, inputs())
    elif args.command == "seal":
        proof = seal(json.loads(Path("target/pilot-inputs.json").read_text()), args.output, os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_RUN_ATTEMPT"])
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.writelines(f"{key}={value}\n" for key, value in proof.items())
    elif args.command == "acquire":
        expected = json.loads(Path("target/pilot-inputs.json").read_text())
        proof = json.loads(os.environ["PRODUCER_PROOF"])
        if expected["commit"] != os.environ["GITHUB_SHA"] or positive(proof["run_id"]) != positive(os.environ["GITHUB_RUN_ID"]):
            raise ValueError("Producer outputs do not belong to this exact checkout and workflow run")
        decision = acquire(expected, proof, args.output, cold=args.cold)
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write(f"decision={decision}\n")
        write_json(Path("target/pilot-decision.json"), {"decision": decision})
    elif args.command == "timeline":
        write_json(args.output, timeline(os.environ["GITHUB_RUN_ID"]))
    else:
        write_json(args.output, summarize(json.loads(Path(os.environ["PILOT_SAMPLES"]).read_text())))


if __name__ == "__main__":
    main()
