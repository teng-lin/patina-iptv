"""Verified native source preparation and corresponding-source bundles (stdlib only)."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import urllib.request
import zipfile


def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def relative_path(value):
    if not isinstance(value, str) or not value or "\\" in value:
        raise ValueError(f"Invalid relative path: {value!r}")
    path = PurePosixPath(value)
    if path.is_absolute() or ".." in path.parts or any(":" in p for p in path.parts):
        raise ValueError(f"Unsafe archive path: {value!r}")
    return path


def read_lock(path):
    lock = json.loads(Path(path).read_text(encoding="utf-8"))
    if lock.get("schema") != 1 or not isinstance(lock.get("sources"), list):
        raise ValueError("Expected source lock schema 1 with sources array")
    components = [(item, "source") for item in lock["sources"]]
    if lock.get("toolchain") is not None:
        components.append((lock["toolchain"], "toolchain"))
    names = set()
    for item, _ in components:
        if not isinstance(item, dict):
            raise ValueError("Invalid component")
        name = item.get("name", "")
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", name) or name.casefold() in names:
            raise ValueError(f"Invalid or duplicate component name: {name!r}")
        names.add(name.casefold())
        if not isinstance(item.get("version"), str) or not item["version"]:
            raise ValueError(f"Missing version: {name}")
        if not isinstance(item.get("url"), str) or not item["url"].startswith("https://"):
            raise ValueError(f"Expected HTTPS source URL: {name}")
        if not re.fullmatch(r"[0-9a-f]{64}", item.get("sha256", "")):
            raise ValueError(f"Expected SHA-256: {name}")
        if not isinstance(item.get("license_files"), list) or not item["license_files"]:
            raise ValueError(f"Missing explicit license files: {name}")
        for license_path in item["license_files"]:
            relative_path(license_path)
    return lock, components


def download(item, cache):
    cache.mkdir(parents=True, exist_ok=True)
    destination = cache / item["sha256"]
    if destination.is_file() and digest(destination) == item["sha256"]:
        return destination
    with tempfile.NamedTemporaryFile(dir=cache, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            request = urllib.request.Request(item["url"], headers={"User-Agent": "Patina-runtime-builder/1"})
            with urllib.request.urlopen(request, timeout=120) as response:
                if not response.geturl().startswith("https://"):
                    raise ValueError("Source redirected away from HTTPS")
                shutil.copyfileobj(response, stream)
        except BaseException:
            stream.close()
            temporary.unlink(missing_ok=True)
            raise
    try:
        if digest(temporary) != item["sha256"]:
            raise ValueError(f"Source checksum mismatch: {item['name']}")
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)
    return destination


def extract(archive, destination):
    """Extract without following archive links; permit only internal symlinks."""
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=True)
    if any(destination.iterdir()):
        raise ValueError("Extraction destination must be empty")
    is_zip = zipfile.is_zipfile(archive)
    opener = zipfile.ZipFile if is_zip else tarfile.open
    with opener(archive) as source:
        entries = []
        for entry in source.infolist() if is_zip else source.getmembers():
            path = relative_path(entry.filename if is_zip else entry.name)
            if not path.parts:
                continue
            mode = entry.external_attr >> 16 if is_zip else entry.mode
            directory = entry.is_dir() if is_zip else entry.isdir()
            symlink = stat.S_ISLNK(mode) if is_zip else entry.issym()
            hardlink = False if is_zip else entry.islnk()
            if is_zip and stat.S_IFMT(mode) not in (0, stat.S_IFREG, stat.S_IFDIR, stat.S_IFLNK):
                raise ValueError(f"Unsupported archive entry: {path}")
            if not is_zip and not (directory or symlink or hardlink or entry.isfile()):
                raise ValueError(f"Unsupported archive entry: {path}")
            entries.append((entry, path, mode, directory, symlink, hardlink))
        roots = {p.parts[0] for _, p, *_ in entries}
        strip_root = len(roots) == 1 and all(len(p.parts) > 1 or directory for _, p, _, directory, _, _ in entries)
        seen = set()
        links = []
        for entry, path, mode, directory, symlink, hardlink in entries:
            path = PurePosixPath(*path.parts[1:]) if strip_root else path
            if not path.parts:
                continue
            key = os.path.normcase(str(path))
            if key in seen:
                raise ValueError(f"Duplicate archive path: {path}")
            seen.add(key)
            target = destination.joinpath(*path.parts)
            if symlink or hardlink:
                link = source.read(entry).decode("utf-8") if is_zip else entry.linkname
                if "\\" in link or PurePosixPath(link).is_absolute() or any(":" in p for p in PurePosixPath(link).parts):
                    raise ValueError(f"Unsafe archive link: {path}")
                if hardlink:
                    linkpath = relative_path(link)
                    if strip_root:
                        if not linkpath.parts or linkpath.parts[0] not in roots:
                            raise ValueError(f"Unsafe archive hardlink: {path}")
                        linkpath = PurePosixPath(*linkpath.parts[1:])
                    resolved = destination.joinpath(*linkpath.parts)
                else:
                    resolved = target.parent / link
                if not resolved.resolve().is_relative_to(destination.resolve()):
                    raise ValueError(f"Escaping archive link: {path}")
                links.append((target, link, resolved, hardlink))
                continue
            if directory:
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with (source.open(entry) if is_zip else source.extractfile(entry)) as incoming, target.open("xb") as outgoing:
                    shutil.copyfileobj(incoming, outgoing)
                target.chmod(0o755 if mode & 0o111 else 0o644)
        # No file writes occur after links are installed. Validate again to reject
        # chains that become escaping links after another link is created.
        for target, link, resolved, hardlink in links:
            if not target.parent.resolve().is_relative_to(destination.resolve()):
                raise ValueError(f"Escaping archive link parent: {target}")
            target.parent.mkdir(parents=True, exist_ok=True)
            if hardlink:
                if not resolved.is_file() or resolved.is_symlink():
                    raise ValueError(f"Invalid hardlink target: {target}")
                os.link(resolved, target)
            else:
                target.symlink_to(link)
        for target, _, _, _ in links:
            if not target.resolve().is_relative_to(destination.resolve()):
                raise ValueError(f"Escaping archive link chain: {target}")


def restore_cache(lock_path, work, source_bundle):
    """Restore verified inputs from a retained bundle without consulting URLs."""
    lock, components = read_lock(lock_path)
    with tempfile.TemporaryDirectory(dir=work) as temporary:
        extracted = Path(temporary) / "bundle"
        extract(source_bundle, extracted)
        if read_lock(extracted / "sources.lock.json")[0] != lock:
            raise ValueError("Retained source bundle lock differs from requested lock")
        archives = []
        for item, _ in components:
            archive = extracted / "archives" / f"{item['name']}-{item['sha256']}.archive"
            if not archive.resolve().is_relative_to(extracted.resolve()) or not archive.is_file() or digest(archive) != item["sha256"]:
                raise ValueError(f"Missing or modified retained source archive: {item['name']}")
            archives.append((archive, item["sha256"]))
        # Validate the entire bundle before populating the persistent cache.
        cache = work / "cache"
        cache.mkdir(parents=True, exist_ok=True)
        for archive, checksum in archives:
            with tempfile.NamedTemporaryFile(dir=cache, delete=False) as stream:
                staged = Path(stream.name)
                try:
                    with archive.open("rb") as incoming:
                        shutil.copyfileobj(incoming, stream)
                except BaseException:
                    stream.close()
                    staged.unlink(missing_ok=True)
                    raise
            try:
                staged.replace(cache / checksum)
            finally:
                staged.unlink(missing_ok=True)


def apply_patches(component, directory):
    """Apply the retained local changes to a fresh, checksum-verified source."""
    applied = []
    for patch in sorted((Path(__file__).resolve().parent / "patches").glob(f"{component}-*.patch")):
        subprocess.run(
            ["patch", "-t", "-N", "-F", "0", "-p", "1", "-i", str(patch)],
            cwd=directory,
            check=True,
        )
        applied.append({"path": "patches/" + patch.name, "sha256": digest(patch)})
    return applied


def patch_inventory(component):
    return [
        {"path": "patches/" + patch.name, "sha256": digest(patch)}
        for patch in sorted((Path(__file__).resolve().parent / "patches").glob(f"{component}-*.patch"))
    ]


def prepare(lock_path, work, source_bundle=None):
    lock, components = read_lock(lock_path)
    work = Path(work).resolve()
    work.mkdir(parents=True, exist_ok=True)
    if source_bundle is not None:
        restore_cache(lock_path, work, source_bundle)
    inventory = []
    for item, role in components:
        if source_bundle is None:
            archive = download(item, work / "cache")
        else:
            archive = work / "cache" / item["sha256"]
            if not archive.is_file() or digest(archive) != item["sha256"]:
                raise ValueError(f"Missing or modified offline source archive: {item['name']}")
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            extracted = Path(temporary) / "source"
            extract(archive, extracted)
            patches = apply_patches(item["name"], extracted)
            licenses = []
            for name in item["license_files"]:
                license_path = extracted.joinpath(*relative_path(name).parts)
                if not license_path.resolve().is_relative_to(extracted.resolve()) or not license_path.is_file():
                    raise ValueError(f"Missing or unsafe license file: {item['name']}/{name}")
                licenses.append({"path": name, "sha256": digest(license_path)})
            destination = work / "sources" / item["name"]
            destination.parent.mkdir(exist_ok=True)
            if destination.is_symlink():
                raise ValueError(f"Unexpected source symlink: {destination}")
            if destination.exists():
                shutil.rmtree(destination)
            shutil.move(str(extracted), destination)
        inventory.append({**item, "role": role, "licenses": licenses, "patches": patches})
    (work / "sources.lock.json").write_text(json.dumps(lock, indent=2) + "\n", encoding="utf-8")
    (work / "components.json").write_text(json.dumps({"schema": 1, "components": inventory}, indent=2) + "\n", encoding="utf-8")


def bundle(lock_path, work, output):
    lock, components = read_lock(lock_path)
    work, output = Path(work), Path(output)
    if json.loads((work / "sources.lock.json").read_text(encoding="utf-8")) != lock:
        raise ValueError("Prepared source lock differs from requested lock")
    inventory = json.loads((work / "components.json").read_text(encoding="utf-8"))
    expected = {item["name"]: item for item, _ in components}
    if len(inventory["components"]) != len(expected) or {item["name"] for item in inventory["components"]} != set(expected):
        raise ValueError("Component inventory differs from lock")
    files = [(work / "sources.lock.json", "sources.lock.json"), (work / "components.json", "components.json")]
    for item, role in components:
        archive = work / "cache" / item["sha256"]
        if not archive.is_file() or digest(archive) != item["sha256"]:
            raise ValueError(f"Missing or modified source archive: {item['name']}")
        files.append((archive, f"archives/{item['name']}-{item['sha256']}.archive"))
        record = next(row for row in inventory["components"] if row["name"] == item["name"])
        if record.get("role") != role or any(record.get(key) != value for key, value in item.items()):
            raise ValueError(f"Component inventory changed: {item['name']}")
        if record.get("patches", []) != patch_inventory(item["name"]):
            raise ValueError(f"Native patches changed after source preparation: {item['name']}")
        if [entry["path"] for entry in record["licenses"]] != item["license_files"]:
            raise ValueError(f"License inventory differs from lock: {item['name']}")
        for license_info in record["licenses"]:
            path = work / "sources" / item["name"] / license_info["path"]
            if not path.resolve().is_relative_to((work / "sources" / item["name"]).resolve()):
                raise ValueError(f"Unsafe license path: {path}")
            if digest(path) != license_info["sha256"]:
                raise ValueError(f"License changed: {path}")
            files.append((path, f"licenses/{item['name']}/{license_info['path']}"))
    scripts = Path(__file__).resolve().parent
    for path in sorted(scripts.rglob("*")):
        if path.is_file() and "__pycache__" not in path.parts and path.name != "runtime.json" and (path.suffix in {".py", ".sh", ".ps1", ".json", ".cmake", ".patch", ".md", ".txt", ".c", ".h"} or path.name in {"Dockerfile", "Makefile"}):
            files.append((path, "build-scripts/" + path.relative_to(scripts).as_posix()))
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0) as compressed, tarfile.open(fileobj=compressed, mode="w") as archive:
        for path, name in sorted(files, key=lambda pair: pair[1]):
            info = archive.gettarinfo(str(path), arcname=name)
            # License files can legitimately be internal source symlinks.
            info.type = tarfile.REGTYPE
            info.linkname = ""
            info.size = path.stat().st_size
            info.uid = info.gid = info.mtime = 0
            info.uname = info.gname = ""
            info.mode = 0o755 if path.suffix == ".sh" else 0o644
            with path.open("rb") as stream:
                archive.addfile(info, stream)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("prepare", "bundle"):
        command = commands.add_parser(name)
        command.add_argument("--lock", type=Path, required=True)
        command.add_argument("--work", type=Path, required=True)
        if name == "bundle":
            command.add_argument("--output", type=Path, required=True)
        else:
            command.add_argument("--source-bundle", type=Path, help="Restore inputs from a retained source bundle without network access")
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.lock, args.work, args.source_bundle)
    else:
        bundle(args.lock, args.work, args.output)


if __name__ == "__main__":
    main()
