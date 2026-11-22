#!/usr/bin/env python3
"""A source dependency update must also reach the committed runner lock."""
from pathlib import Path
import tomllib
import json
import subprocess
def resolve_reference(packages, dependency):
    """The packages a lock's `dependencies` entry names: "name", "name version"
    or "name version (source)"."""
    fields = dependency.split(" ", 2)
    return [p for p in packages if p["name"] == fields[0]
            and (len(fields) < 2 or p["version"] == fields[1])
            and (len(fields) < 3 or p.get("source") == fields[2].strip("()"))]


def validate_references(packages):
    for package in packages:
        dependencies = package.get("dependencies", [])
        if dependencies != sorted(dependencies):
            raise SystemExit(f"Regenerate runner lock; unsorted dependencies in {package['name']}")
        for dependency in dependencies:
            if len(resolve_reference(packages, dependency)) != 1:
                raise SystemExit(f"Regenerate runner lock; ambiguous or missing dependency {dependency!r} in {package['name']}")

def validate_source_identities(packaged, source):
    """Every required source package must exist, including newly introduced names."""
    identities = {(p["name"], p["version"], p.get("source"), p.get("checksum")) for p in packaged}
    missing = [f'{p["name"]} {p["version"]}' for p in source
               if (p["name"], p["version"], p.get("source"), p.get("checksum")) not in identities]
    if missing:
        raise SystemExit("Regenerate tools/ci/package.Cargo.lock; missing or mismatched source dependencies: " + ", ".join(missing))

def validate_member_dependencies(packaged, members):
    """Each packaged workspace member's dependency list must be what the
    runner resolves for it. A runner builds the members as path
    dependencies, whose dev-dependencies it does not resolve, so a
    dependency that was already a member's dev-dependency (in the source
    lock) becomes a new edge there alone.

    `members` maps (name, version) to the identities, (name, version,
    source), of the member's normal and build dependencies on any target.
    `packaged` must already pass `validate_references`."""
    stale = []
    for package in packaged:
        if "source" in package:
            continue
        expected = members.get((package["name"], package["version"]))
        if expected is None:
            continue
        locked = set()
        for dependency in package.get("dependencies", []):
            [match] = resolve_reference(packaged, dependency)
            locked.add((match["name"], match["version"], match.get("source")))
        if locked != expected:
            changes = [f"+{name} {version}" for name, version, _ in sorted(expected - locked, key=str)]
            changes += [f"-{name} {version}" for name, version, _ in sorted(locked - expected, key=str)]
            stale.append(f'{package["name"]} ({", ".join(changes)})')
    if stale:
        raise SystemExit("Regenerate tools/ci/package.Cargo.lock; stale dependency lists: " + "; ".join(stale))


# Workspace binaries built in CI that are not packaged into NSH runners, and
# transitive feature-unified deps only pulled in by those unpackaged members
# (`iptv-tui` -> `nsh-platform-tui` -> `nsh-base` enables `time/local-offset`).
# (`iptv-web` is in the runner lock: the shell's browser build depends on it.)
UNPACKAGED_MEMBERS = {"iptv-tui"}
UNPACKAGED_FEATURE_DEPS = {"num_threads"}


def main():
    root = Path(__file__).resolve().parents[2]
    packaged = tomllib.loads((root / "tools/ci/package.Cargo.lock").read_text())["package"]
    validate_references(packaged)
    command = ["cargo", "metadata", "--locked", "--format-version", "1"]
    manifests = json.loads(subprocess.check_output([*command, "--no-deps"], cwd=root))
    # The workspace builds the standalone bridge by default. The NSH runner
    # requests only iptv-web's adapters with default-features = false.
    defaults = [f"{p['name']}/default" for p in manifests["packages"]
                if p["name"] != "iptv-web" and "default" in p["features"]]
    metadata = json.loads(subprocess.check_output([
        *command, "--no-default-features", "--features", ",".join(defaults),
    ], cwd=root))
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    packages_by_id = {p["id"]: p for p in metadata["packages"]}
    pending = [
        member for member in metadata["workspace_members"]
        if packages_by_id[member]["name"] not in UNPACKAGED_MEMBERS
    ]
    reachable = set()
    while pending:
        node = pending.pop()
        if node in reachable:
            continue
        reachable.add(node)
        for dependency in nodes[node]["deps"]:
            if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"]):
                pending.append(dependency["pkg"])
    needed = {
        (p["name"], p["version"], p["source"])
        for p in metadata["packages"]
        if p["id"] in reachable and p["name"] not in UNPACKAGED_FEATURE_DEPS
    }
    source = [p for p in tomllib.loads((root / "Cargo.lock").read_text())["package"]
              if (p["name"], p["version"], p.get("source")) in needed]
    validate_source_identities(packaged, source)
    members = {}
    for member in metadata["workspace_members"]:
        package = packages_by_id[member]
        members[(package["name"], package["version"])] = {
            (packages_by_id[d["pkg"]]["name"], packages_by_id[d["pkg"]]["version"],
             packages_by_id[d["pkg"]]["source"])
            for d in nodes[member]["deps"]
            if any(kind["kind"] != "dev" for kind in d["dep_kinds"])
        }
    validate_member_dependencies(packaged, members)
    print("shared source and runner lock identities and member dependencies agree")

if __name__ == "__main__":
    main()
