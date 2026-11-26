#!/usr/bin/env python3
"""Corresponding source for the system libraries bundled in desktop packages.

libmpv, with FFmpeg and the libraries it uses linked in, is Patina's own
pinned build in both desktop packages (tools/mpv-runtime-unix): its exact
sources are retained with its dependency release and named in the package's
notices/mpv-runtime/SOURCE.txt. This script covers the other bundled
libraries: the Linux AppImage's GTK stack and the host libraries libmpv
uses (Ubuntu builds), and any Homebrew library the macOS app still links
(normally none). Each release publishes the source they were built from:

  Patina-<version>-macos-native-sources.tar   (only when Homebrew libraries are bundled)
  Patina-<version>-linux-native-sources.tar

Packaging writes `notices/native/sources.json` (what to fetch) and
`SOURCE.txt` (where a user finds it) into the package. This script reads that
manifest out of the finished package, downloads every source, checks it
against its recorded checksum or commit, and writes the archive and a
`.json` manifest beside it. Anything missing or mismatched fails the job: a
release must not ship a library without its source.

macOS: the source comes from the formula file Homebrew keeps inside each
installed keg (`.brew/<name>.rb`). That is the recipe of the installed
version, which the tap's current formula need not be. Its `url`/`sha256`
pairs name the upstream archive, patches and resources.

Linux: `apt-get source --download-only <source>=<version>` for the exact
version `dpkg-query` reports (needs deb-src entries: enable-deb-src.sh). A
version that has left the archive index is fetched from Launchpad, which
keeps every published version. Unchanged files still in the configured
archive can be fetched there. The exact version's .dsc checks each file.

Usage:
  native-sources.py macos APP OUTPUT.tar
  native-sources.py linux APPDIR OUTPUT.tar
"""
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile

REPOSITORY = "https://github.com/teng-lin/patina-iptv"
# GitHub refuses release files of 2 GiB or more.
ARCHIVE_LIMIT = 2 * 1024 ** 3 - 1
NOTICES = {"macos": "Contents/Resources/notices/native", "linux": "usr/bin/notices/native"}
UBUNTU_DOWNLOAD_JOBS = 3


def archive_name(platform, version):
    return f"Patina-{version}-{platform}-native-sources.tar"


NOTE = {
    "macos": """\
libmpv.2.dylib in Contents/Frameworks is Patina's pinned libmpv runtime, with
FFmpeg and the libraries it uses linked in. Its notices, and the address and
checksum of its exact source archive, are in ../mpv-runtime (SOURCE.txt).

The other libraries in Contents/Frameworks are Homebrew builds. Only their
load paths and code signatures were changed to make the app self-contained.

components.json lists each library with its Homebrew formula and version.
sources.json lists, for every formula, the upstream archive, patches and
resources it was built from, with SHA-256 checksums or Git commits. The
formula itself, which is the build recipe, is in the directory named after
it.
""",
    "linux": """\
libmpv.so.2 in usr/lib is Patina's pinned libmpv runtime, with FFmpeg and the
libraries it uses linked in. Its notices, and the address and checksum of its
exact source archive, are in ../mpv-runtime (SOURCE.txt).

The other libraries in usr/lib (GTK, and the host libraries GTK and libmpv
use) are Ubuntu builds, copied from the system the AppImage was built on.

components.json lists each library with the Ubuntu package that installed it
and the source package and version that package was built from. sources.json
lists those source packages.
""",
}


def source_note(platform, version):
    """SOURCE.txt: shipped inside the package, beside sources.json."""
    return (
        f"Patina {version}: source code of the bundled native libraries\n\n"
        + NOTE[platform] +
        "\nThe complete corresponding source of a released build is published with\n"
        "that release:\n\n"
        f"  {archive_name(platform, version)}\n"
        f"  {REPOSITORY}/releases/tag/v{version}\n\n"
        "The archive holds exactly the sources named in sources.json, each checked\n"
        "against its checksum, and a manifest with every file's address and\n"
        "SHA-256. The archive's own SHA-256 is in the release's SHA256SUMS.\n\n"
        "A build that was not released (a development or CI build) has no such\n"
        "archive; sources.json still names what to fetch.\n"
    )


def digest(path, algorithm="sha256"):
    with open(path, "rb") as stream:
        return hashlib.file_digest(stream, algorithm).hexdigest()


# ---------------------------------------------------------------- Homebrew

URL_LINE = re.compile(r'^(\s*)url\s+"([^"]+)"(.*)$')
# Blocks whose sources are not part of an Apple silicon bottle.
SKIPPED_BLOCKS = ("head ", "test ", "bottle ", "livecheck ", "on_linux ", "on_intel ", "def ")


def enclosing(lines, index):
    """The headers of the blocks around a line, outermost first, as (line, text).

    Formulae are formatted by RuboCop, so indentation follows nesting: going
    up, each line indented less than everything seen so far opens a block.
    """
    indent = len(lines[index]) - len(lines[index].lstrip())
    headers = []
    for position in range(index - 1, -1, -1):
        line = lines[position]
        if not line.strip():
            continue
        current = len(line) - len(line.lstrip())
        if current < indent:
            headers.append((position, line.strip()))
            indent = current
            if indent == 0:
                break
    return headers[::-1]


def checksum_pairs(lines, headers, variable):
    """The values of `%w[name checksum ...].each_slice(2) do |name, checksum|`.

    GNU formulae list their upstream patches this way: one block for all of
    them, with the patch number spliced into the address.
    """
    for position, header in headers:
        loop = re.fullmatch(r"(\w+|\])\.each_slice\(2\) do \|(\w+), (\w+)\|", header)
        if not loop or loop[3] != variable:
            continue
        if loop[1] == "]":
            last = position
        else:
            opening = re.compile(r"\s*" + re.escape(loop[1]) + r" = %w\[\s*$")
            first = next((number for number, line in enumerate(lines) if opening.match(line)), None)
            if first is None:
                break
            last = next(number for number in range(first, len(lines)) if lines[number].strip() == "]")
        words = []
        for line in reversed(lines[:last]):
            words[:0] = line.replace("%w[", " ").split("=")[-1].split()
            if "%w[" in line:
                break
        pairs = list(zip(words[::2], words[1::2]))
        if pairs and len(words) % 2 == 0 and all(re.fullmatch(r"[0-9a-f]{64}", checksum) for _, checksum in pairs):
            return loop[2], pairs
    return None


def test_only(lines, resource):
    """Sample data for `test do`: marked `:test`, or staged nowhere else."""
    quoted = '"' + re.escape(resource) + '"'
    declaration = re.compile(r"\s*resource " + quoted + r"(,.*)? do$")
    if any(match and ":test" in (match[1] or "") for match in map(declaration.match, lines)):
        return True
    use = re.compile(r"\bresource\(?\s*" + quoted)
    uses = [index for index, line in enumerate(lines) if use.search(line) and not declaration.match(line)]
    return bool(uses) and all(
        any(header.startswith("test ") for _, header in enclosing(lines, index)) for index in uses)


def formula_sources(text, name):
    """Every source a formula's stable build downloads: archive, patches, resources."""
    lines = text.split("\n__END__\n")[0].splitlines()
    sources = []
    for index, line in enumerate(lines):
        match = URL_LINE.match(line)
        if not match:
            continue
        headers = enclosing(lines, index)
        if not headers or not headers[0][1].startswith("class "):
            raise ValueError(f"{name}: source outside the formula class")
        blocks = [header + " " for _, header in headers[1:]]
        if any(block.startswith(SKIPPED_BLOCKS) for block in blocks):
            continue
        role = "source"
        for block in blocks:
            resource = re.match(r'resource "([^"]+)"', block)
            if resource:
                role = "resource:" + resource[1]
            elif block.startswith("patch") and role == "source":
                role = "patch"
        if role.startswith("resource:") and test_only(lines, role.split(":", 1)[1]):
            continue
        indent = len(match[1])
        statement = line
        last = index
        while statement.rstrip().endswith(",") and last + 1 < len(lines):
            last += 1
            statement += " " + lines[last].strip()
        source = {"role": role, "url": match[2]}
        expanded = [source]
        revision = re.search(r'\brevision:\s*"([^"]+)"', statement)
        if revision or re.search(r"\b(tag|branch):|using:\s*:git\b", statement) or match[2].endswith(".git"):
            if not revision or not re.fullmatch(r"[0-9a-f]{40}", revision[1]):
                raise ValueError(f"{name}: Git source {match[2]} is not pinned to a commit")
            source.update(git=True, revision=revision[1])
            tag = re.search(r'\btag:\s*"([^"]+)"', statement)
            if tag:
                source["tag"] = tag[1]
        else:
            mirrors = []
            variable = None
            for following in lines[last + 1:]:
                if not following.strip():
                    continue
                current = len(following) - len(following.lstrip())
                if current < indent or (current == indent and URL_LINE.match(following)):
                    break
                if current > indent:
                    continue
                mirror = re.fullmatch(r'mirror "([^"]+)"', following.strip())
                checksum = re.fullmatch(r'sha256 (?:"([0-9a-f]{64})"|(\w+))', following.strip())
                if mirror:
                    mirrors.append(mirror[1])
                if checksum:
                    source["sha256"], variable = checksum[1], checksum[2]
                    break
            if mirrors:
                source["mirrors"] = mirrors
            if variable:
                pairs = checksum_pairs(lines, headers, variable)
                if not pairs:
                    raise ValueError(f"{name}: cannot resolve the checksum of {match[2]}")
                splice = "#{" + pairs[0] + "}"
                expanded = [
                    {key: ([item.replace(splice, value) for item in field] if key == "mirrors"
                           else checksum if key == "sha256" else field.replace(splice, value))
                     for key, field in source.items()}
                    for value, checksum in pairs[1]]
            elif not source.get("sha256"):
                raise ValueError(f"{name}: source {match[2]} has no SHA-256 checksum")
        for item in expanded:
            if any("#{" in address for address in [item["url"], *item.get("mirrors", [])]):
                raise ValueError(f"{name}: cannot resolve the address {item['url']}")
        sources += expanded
    if not any(source["role"] == "source" for source in sources):
        raise ValueError(f"{name}: formula names no stable source")
    return sources


def homebrew_component(prefix):
    """One installed keg: its name, version, formula text and sources."""
    prefix = Path(prefix)
    name, version = prefix.parent.name, prefix.name
    formulae = sorted((prefix / ".brew").glob("*.rb"))
    if len(formulae) != 1:
        raise ValueError(f"{name} {version}: expected one installed formula in {prefix}/.brew")
    text = formulae[0].read_text()
    return {"name": name, "version": version, "recipe": f"{name}-{version}/formula.rb",
            "sources": formula_sources(text, name)}, text


def manifest(platform, version, components):
    return {"schema": 1, "platform": platform, "release": version, "archive": archive_name(platform, version),
            "components": sorted(components, key=lambda component: (component["name"], component["version"]))}


# ------------------------------------------------------------------ Ubuntu

def source_package(query_output, package):
    """`dpkg-query -W -f='${source:Package} ${source:Version}'` for one binary package."""
    fields = query_output.split()
    if len(fields) != 2:
        raise ValueError(f"no source package recorded for {package}")
    return {"source": fields[0], "source_version": fields[1]}


def ubuntu_components(inventory):
    """Bundled libraries grouped by the source package version that built them."""
    grouped = {}
    for row in inventory:
        key = (row["source"], row["source_version"])
        grouped.setdefault(key, set()).add(row["package"])
    return [{"name": name, "version": version, "binaries": sorted(packages)}
            for (name, version), packages in sorted(grouped.items())]


def print_uris(output):
    """`apt-get source --print-uris`: 'URL' name size ALGORITHM:hex, one file per line."""
    files = []
    for line in output.splitlines():
        match = re.fullmatch(r"'([^']+)' (\S+) (\d+) (SHA256|SHA512):([0-9a-f]+)", line.strip())
        if match:
            files.append({"url": match[1], "name": match[2], "size": int(match[3]),
                          "algorithm": match[4].lower(), "checksum": match[5]})
    if not any(row["name"].endswith(".dsc") for row in files):
        raise ValueError("apt named no source description (.dsc)")
    return files


def dsc_files(text):
    """The files a Debian source description lists under Checksums-Sha256."""
    files = []
    inside = False
    for line in text.splitlines():
        if line.startswith("Checksums-Sha256:"):
            inside = True
        elif inside and line.startswith(" "):
            checksum, size, name = line.split()
            if not re.fullmatch(r"[0-9a-f]{64}", checksum) or "/" in name or name.startswith("."):
                raise ValueError("malformed source description")
            files.append({"name": name, "size": int(size), "algorithm": "sha256", "checksum": checksum})
        elif inside:
            break
    if not files:
        raise ValueError("source description lists no files")
    return files


def launchpad_url(name, version, filename):
    """Launchpad keeps every published source version, also after the archive drops it."""
    return f"https://launchpad.net/ubuntu/+archive/primary/+sourcefiles/{name}/{version}/{filename}"


def dsc_name(name, version):
    return f"{name}_{version.split(':', 1)[-1]}.dsc"


# -------------------------------------------------------------- collecting

def run(arguments, cwd=None):
    try:
        return subprocess.run(arguments, cwd=cwd, capture_output=True, text=True, timeout=180)
    except subprocess.TimeoutExpired:
        raise SystemExit(f"timed out collecting source: {' '.join(arguments)}") from None


def download(url, destination):
    """True when the file arrived. Integrity is the caller's checksum, not the transport."""
    arguments = [
        "curl", "--fail", "--location", "--silent", "--show-error", "--retry", "3",
        # Also refused and reset connections; the checksum guards what arrives.
        "--retry-all-errors", "--connect-timeout", "20", "--max-time", "120", "--retry-max-time", "120",
        # Leave time for a retry when a connected server stops sending bytes.
        "--speed-limit", "1", "--speed-time", "20",
        "--proto", "=https,http", "--proto-redir", "=https,http", "--output", str(destination), url,
    ]
    try:
        result = subprocess.run(arguments, capture_output=True, text=True, timeout=180)
    except subprocess.TimeoutExpired:
        print(f"  timed out downloading {url}", file=sys.stderr)
    else:
        if result.returncode == 0:
            return True
        print(f"  could not download {url}: {result.stderr.strip()}", file=sys.stderr)
    Path(destination).unlink(missing_ok=True)
    return False


GIT_ATTEMPTS = 3
GIT_TIMEOUT = 1800


def clone(url, directory):
    """Clones without checking out, trying again after a dropped connection."""
    for attempt in range(1, GIT_ATTEMPTS + 1):
        shutil.rmtree(directory, ignore_errors=True)
        try:
            result = subprocess.run(["git", "clone", "--quiet", "--no-checkout", url, directory],
                                    capture_output=True, text=True, timeout=GIT_TIMEOUT)
        except subprocess.TimeoutExpired:
            print(f"  git clone timed out for {url} (attempt {attempt})", file=sys.stderr)
            continue
        if result.returncode == 0:
            return True
        print(f"  git clone failed for {url} (attempt {attempt}): {result.stderr.strip()}", file=sys.stderr)
    return False


def git_archive(url, revision, destination):
    """A tar of exactly one commit; the commit hash is the integrity check."""
    with tempfile.TemporaryDirectory() as parent:
        directory = str(Path(parent) / "repository")
        if not clone(url, directory):
            return False
        arguments = ["git", "-C", directory, "archive", "--format=tar", f"--prefix={revision}/",
                     "--output", str(destination), revision + "^{commit}"]
        result = run(arguments)
        if result.returncode != 0:
            print(f"  git archive failed for {url}: {result.stderr.strip()}", file=sys.stderr)
            return False
        resolved = run(["git", "-C", directory, "rev-parse", revision + "^{commit}"]).stdout.strip()
        return resolved == revision


def unique(directory, name):
    candidate, number = name, 1
    while (directory / candidate).exists():
        number += 1
        candidate = f"{number}-{name}"
    return candidate


def fetch_homebrew(component, stage, download=download, git_archive=git_archive):
    directory = stage / f"{component['name']}-{component['version']}"
    directory.mkdir(parents=True, exist_ok=True)
    for source in component["sources"]:
        label = f"{component['name']} {component['version']} ({source['role']})"
        if source.get("git"):
            name = unique(directory, f"{component['name']}-{source['revision']}.tar")
            if not git_archive(source["url"], source["revision"], directory / name):
                raise SystemExit(f"cannot fetch source of {label}: {source['url']} at {source['revision']}")
            source["sha256"] = digest(directory / name)
        else:
            name = unique(directory, source["url"].split("?")[0].rstrip("/").rsplit("/", 1)[-1] or "source")
            for url in [source["url"], *source.get("mirrors", [])]:
                if download(url, directory / name):
                    break
            else:
                raise SystemExit(f"cannot fetch source of {label}: {source['url']}")
            actual = digest(directory / name)
            if actual != source["sha256"]:
                raise SystemExit(f"checksum mismatch for source of {label}: {source['url']}\n"
                                 f"  expected {source['sha256']}\n  received {actual}")
        source["file"] = f"{directory.name}/{name}"


def check_file(path, row, label):
    if not path.is_file():
        raise SystemExit(f"source file missing for {label}: {row['name']}")
    actual = digest(path, row["algorithm"])
    if path.stat().st_size != row["size"] or actual != row["checksum"]:
        raise SystemExit(f"checksum mismatch for source of {label}: {row['name']}\n"
                         f"  expected {row['checksum']}\n  received {actual}")


def cached_source(cache, row, destination):
    if cache is None:
        return False
    entry = cache / f"{row['algorithm']}-{row['checksum']}"
    if entry.is_symlink():
        return False
    try:
        check_file(entry, row, destination.name)
        shutil.copyfile(entry, destination)
    except (SystemExit, FileNotFoundError):
        return False
    return True


def fetch_ubuntu(component, stage, run=run, download=download, cache=None):
    name, version = component["name"], component["version"]
    label = f"{name} {version}"
    directory = stage / f"{name}_{version.split(':', 1)[-1]}"
    directory.mkdir(parents=True, exist_ok=True)
    listed = run(["apt-get", "source", "--print-uris", f"{name}={version}"])
    if listed.returncode == 0:
        files = print_uris(listed.stdout)
        restored = [cached_source(cache, row, directory / row["name"]) for row in files]
        if not all(restored):
            # apt source reuses files here that match the signed index.
            fetched = run(["apt-get", "source", "--download-only", f"{name}={version}"], cwd=directory)
            if fetched.returncode != 0:
                raise SystemExit(f"cannot fetch source of {label}: {fetched.stderr.strip()}")
        component["origin"] = "apt"
    elif "deb-src" in listed.stderr:
        raise SystemExit("apt has no deb-src entries: run tools/ci/enable-deb-src.sh first")
    else:
        # The installed version is no longer the newest in its pocket.
        print(f"  {label} is not in the archive index ({listed.stderr.strip()}); using Launchpad", file=sys.stderr)
        description = dsc_name(name, version)
        if not download(launchpad_url(name, version, description), directory / description):
            raise SystemExit(
                f"cannot fetch source of {label}: neither the configured deb-src indexes nor Launchpad have it")
        files = [{"name": description, "size": (directory / description).stat().st_size, "algorithm": "sha256",
                  "checksum": digest(directory / description)}, *dsc_files((directory / description).read_text())]
        for row in files:
            row["url"] = launchpad_url(name, version, row["name"])
        # A newer distro revision often reuses the same upstream tarball and
        # detached signature. APT names their current archive URLs directly.
        missing = [row for row in files[1:] if not cached_source(cache, row, directory / row["name"])]
        archive_urls = {}
        if missing:
            current = run(["apt-get", "source", "--print-uris", name])
            if current.returncode == 0:
                archive_urls = {row["name"]: row["url"] for row in print_uris(current.stdout)}
        for row in missing:
            destination = directory / row["name"]
            archive_url = archive_urls.get(row["name"])
            if archive_url and download(archive_url, destination):
                continue
            if not download(row["url"], destination):
                raise SystemExit(f"cannot fetch source of {label}: {row['name']}")
        component["origin"] = "launchpad"
    component["sources"] = []
    for row in files:
        check_file(directory / row["name"], row, label)
        component["sources"].append({"url": row["url"], "sha256": digest(directory / row["name"]),
                                     "file": f"{directory.name}/{row['name']}"})
    extra = {path.name for path in directory.iterdir()} - {row["name"] for row in files}
    if extra:
        raise SystemExit(f"unexpected files fetched for {label}: {sorted(extra)}")
    if cache is None:
        return ()
    cache.mkdir(parents=True, exist_ok=True)
    entries = []
    for row in files:
        entry = cache / f"{row['algorithm']}-{row['checksum']}"
        entries.append(entry.name)
        if (not entry.is_symlink() and entry.is_file() and entry.stat().st_size == row["size"]
                and digest(entry, row["algorithm"]) == row["checksum"]):
            continue
        # Two source packages can share a file. Publish verified bytes atomically.
        with tempfile.TemporaryDirectory(dir=cache) as temporary:
            pending = Path(temporary) / entry.name
            shutil.copyfile(directory / row["name"], pending)
            pending.replace(entry)
    return entries


def linux_cache_key(package):
    description = json.loads((Path(package) / NOTICES["linux"] / "sources.json").read_text())
    versions = sorted((row["name"], row["version"]) for row in description["components"])
    return hashlib.sha256(json.dumps(versions).encode()).hexdigest()


def write_archive(stage, output):
    """A plain tar (its members are already compressed) with stable metadata."""
    with tarfile.open(output, "w") as archive:
        for path in sorted(stage.rglob("*")):
            if not path.is_file() or path.is_symlink():
                continue
            entry = tarfile.TarInfo(path.relative_to(stage).as_posix())
            entry.size = path.stat().st_size
            entry.mode = 0o644
            with path.open("rb") as stream:
                archive.addfile(entry, stream)
    if Path(output).stat().st_size > ARCHIVE_LIMIT:
        raise SystemExit(f"{output} exceeds the size GitHub accepts for a release file")


def collect(platform, package, output, fetch=None, scratch=None, source_cache=None):
    package, output = Path(package), Path(output)
    notices = package / NOTICES[platform]
    if platform == "macos" and not (notices / "sources.json").exists():
        # Only the pinned runtime is bundled; its sources live with its release.
        print(f"{package}: no Homebrew library is bundled; no source archive to collect")
        return None
    description = json.loads((notices / "sources.json").read_text())
    if description.get("schema") != 1 or description.get("platform") != platform or not description.get("components"):
        raise SystemExit(f"{notices}/sources.json does not describe bundled {platform} libraries")
    if output.name != description["archive"]:
        raise SystemExit(f"the package names its source archive {description['archive']}, not {output.name}")
    if fetch is None:
        def fetch(component, stage):
            if platform == "macos":
                return fetch_homebrew(component, stage)
            return fetch_ubuntu(component, stage, cache=source_cache)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=scratch or output.parent) as directory:
        stage = Path(directory)

        def collect_component(component):
            print(f"{component['name']} {component['version']}", flush=True)
            entries = fetch(component, stage)
            if not component.get("sources"):
                raise SystemExit(f"no source fetched for {component['name']} {component['version']}")
            if "recipe" in component:
                recipe = stage / component["recipe"]
                shutil.copyfile(notices / component["recipe"], recipe)
            return entries or ()

        components = description["components"]
        used_entries = set()
        if platform == "linux":
            downloads = ThreadPoolExecutor(max_workers=UBUNTU_DOWNLOAD_JOBS)
            try:
                for entries in downloads.map(collect_component, components):
                    used_entries.update(entries)
            finally:
                downloads.shutdown(wait=True, cancel_futures=True)
        else:
            for component in components:
                collect_component(component)
        text = json.dumps(description, indent=2) + "\n"
        (stage / "sources.json").write_text(text)
        (stage / "SOURCE.txt").write_bytes((notices / "SOURCE.txt").read_bytes())
        write_archive(stage, output)
    output.with_suffix(".json").write_text(text)
    if platform == "linux" and source_cache is not None:
        # A fallback snapshot can carry old digests. Save only this package's sources.
        for entry in source_cache.iterdir():
            if (entry.name not in used_entries and re.fullmatch(r"sha256-[0-9a-f]{64}|sha512-[0-9a-f]{128}", entry.name)
                    and (entry.is_file() or entry.is_symlink())):
                entry.unlink(missing_ok=True)
    files = sum(len(component["sources"]) for component in description["components"])
    print(f"{output.name}: {len(description['components'])} components, {files} files, "
          f"{output.stat().st_size / 2**20:.0f} MB")
    return description


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "linux-cache-key":
        print(linux_cache_key(sys.argv[2]))
    elif len(sys.argv) == 4 and sys.argv[1] in NOTICES:
        cache = os.environ.get("PATINA_NATIVE_SOURCE_CACHE")
        collect(sys.argv[1], sys.argv[2], sys.argv[3], source_cache=Path(cache) if cache else None)
    else:
        raise SystemExit(__doc__)
