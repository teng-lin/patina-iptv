#!/usr/bin/env python3
"""Transfer the exact four NSH runtime files needed for each Android ABI."""
import argparse
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile


SPEC = importlib.util.spec_from_file_location('releases', Path(__file__).with_name('nsh-cli-release.py'))
releases = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(releases)
ABIS = {'arm64-v8a', 'armeabi-v7a', 'x86_64'}
LIMIT = 256 * 1024 * 1024


def entries(abi):
    if abi not in ABIS:
        raise ValueError(f'Unsupported NSH Android ABI: {abi}')
    return ('classes.dex', 'classes.jar', f'lib/{abi}/libnsh.so', f'symbols/{abi}/libnsh.so')


def pack(root, abi, archive):
    archive.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, 'w:gz') as output:
        for name in entries(abi):
            path = root / name
            if not path.is_file() or path.is_symlink() or not 0 < path.stat().st_size <= LIMIT:
                raise ValueError(f'Invalid NSH runtime file: {name}')
            output.add(path, arcname=name, recursive=False)


def unpack(root, abi, archive):
    names = entries(abi)
    root.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, 'r:gz') as source, tempfile.TemporaryDirectory(dir=root) as temporary:
        staged = Path(temporary)
        members = source.getmembers()
        if sorted(member.name for member in members) != sorted(names):
            raise ValueError('NSH runtime archive does not match the requested ABI')
        for member in members:
            if not member.isfile() or not 0 < member.size <= LIMIT:
                raise ValueError('Invalid NSH runtime archive member')
            destination = staged / member.name
            destination.parent.mkdir(parents=True, exist_ok=True)
            with source.extractfile(member) as incoming, destination.open('xb') as outgoing:
                shutil.copyfileobj(incoming, outgoing)
        for name in names:
            destination = root / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            (staged / name).replace(destination)


def fetch(root, abis, key):
    missing = []
    with tempfile.TemporaryDirectory() as temporary:
        for abi in abis:
            archive = Path(temporary) / releases.RUNTIME
            try:
                if releases.restore('Linux-X64', key, archive, f'nsh-android-{abi}'):
                    unpack(root, abi, archive)
                    continue
            except (OSError, ValueError, tarfile.TarError, subprocess.SubprocessError) as error:
                print(f'::warning::Could not restore NSH Android {abi} ({type(error).__name__})')
            missing.append(abi)
    with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
        print('missing=' + ','.join(missing), file=output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    prepare = commands.add_parser('pack')
    prepare.add_argument('root', type=Path)
    prepare.add_argument('abis')
    prepare.add_argument('stage', type=Path)
    retrieve = commands.add_parser('fetch')
    retrieve.add_argument('root', type=Path)
    retrieve.add_argument('abis')
    retrieve.add_argument('key')
    extract = commands.add_parser('unpack')
    extract.add_argument('root', type=Path)
    extract.add_argument('abi', choices=ABIS)
    extract.add_argument('archive', type=Path)
    options = parser.parse_args()
    if options.command == 'pack':
        for abi in options.abis.split(','):
            pack(options.root, abi, options.stage / abi / releases.RUNTIME)
    elif options.command == 'fetch':
        fetch(options.root, options.abis.split(','), options.key)
    else:
        unpack(options.root, options.abi, options.archive)


if __name__ == '__main__':
    main()
