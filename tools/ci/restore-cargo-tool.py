#!/usr/bin/env python3
"""Install one executable from an upstream release with a committed digest."""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import sys
import tarfile
import tempfile
from urllib.request import urlopen
import zipfile


PINS = Path(__file__).with_name('cargo-tool-assets.json')
LIMIT = 64 * 1024 * 1024


def extract(archive, destination):
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as bundle:
            entries = [entry for entry in bundle.infolist() if PurePosixPath(entry.filename).name == destination.name]
            if len(entries) != 1 or stat.S_IFMT(entries[0].external_attr >> 16) not in (0, stat.S_IFREG):
                raise ValueError('Expected exactly one regular Cargo executable')
            with bundle.open(entries[0]) as incoming, destination.open('xb') as outgoing:
                shutil.copyfileobj(incoming, outgoing)
    else:
        with tarfile.open(archive) as bundle:
            entries = [entry for entry in bundle if PurePosixPath(entry.name).name == destination.name]
            if len(entries) != 1 or not entries[0].isfile():
                raise ValueError('Expected exactly one regular Cargo executable')
            with bundle.extractfile(entries[0]) as incoming, destination.open('xb') as outgoing:
                shutil.copyfileobj(incoming, outgoing)


def restore(key, destination):
    pin = json.loads(PINS.read_text()).get(key)
    if not pin:
        return False
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent) as stage:
        root = Path(stage)
        archive = root / 'upstream.archive'
        with urlopen(pin['url'], timeout=45) as response:
            data = response.read(LIMIT + 1)
        if len(data) > LIMIT or hashlib.sha256(data).hexdigest() != pin['sha256']:
            raise ValueError('Upstream Cargo tool archive checksum mismatch')
        archive.write_bytes(data)
        executable = root / destination.name
        extract(archive, executable)
        executable.chmod(0o755)
        executable.replace(destination)
    return True


if __name__ == '__main__':
    key, path = sys.argv[1:]
    restored = False
    try:
        restored = restore(key, Path(path))
    except (OSError, ValueError, tarfile.TarError, zipfile.BadZipFile) as error:
        print(f'::warning::Could not restore upstream Cargo tool ({type(error).__name__}); trying the cache')
    with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
        print(f'restored={str(restored).lower()}', file=output)
