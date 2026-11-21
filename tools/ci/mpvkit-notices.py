#!/usr/bin/env python3
"""Download checksum-pinned native license texts; reject incomplete inventory."""
import argparse
import hashlib
import json
import io
import tarfile
from pathlib import Path
import re
import urllib.request


def generate(manifest_path, output, fetch_script):
    manifest = json.loads(manifest_path.read_text())
    artifacts = re.findall(r'^([A-Za-z0-9_]+) (https://github.com/\S+) ([0-9a-f]{64})$', fetch_script.read_text(), re.M)
    expected = {name: (url, checksum) for name, url, checksum in artifacts}
    actual = {entry['name']: (entry['url'], entry['sha256']) for entry in manifest['artifacts']}
    if not expected or actual != expected:
        raise ValueError('MPVKit artifact inventory is stale; regenerate the license manifest')
    components = {entry['component']: entry for entry in manifest['components']}
    for artifact in manifest['artifacts']:
        if not artifact['components'] or not set(artifact['components']) <= components.keys():
            raise ValueError(f"Missing license coverage for {artifact['name']}")
    output.mkdir(parents=True, exist_ok=True)
    downloads = {}
    for name, component in components.items():
        if not component['files']:
            raise ValueError(f'Missing license texts for {name}')
        for item in component['files']:
            if not item['url'].startswith('https://'):
                raise ValueError('License source must use HTTPS')
            if item['url'] not in downloads:
                with urllib.request.urlopen(item['url'], timeout=60) as response:
                    data = response.read(32 * 1024 * 1024 + 1)
                if len(data) > 32 * 1024 * 1024:
                    raise ValueError('License download exceeds limit')
                downloads[item['url']] = data
            data = downloads[item['url']]
            if 'archive_member' in item:
                if hashlib.sha256(data).hexdigest() != item['archive_sha256']:
                    raise ValueError('Crate archive checksum mismatch')
                with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
                    member = archive.getmember(item['archive_member'])
                    if not member.isfile() or member.size > 4 * 1024 * 1024:
                        raise ValueError('Invalid license archive member')
                    data = archive.extractfile(member).read()
            if len(data) > 4 * 1024 * 1024 or hashlib.sha256(data).hexdigest() != item['sha256']:
                raise ValueError(f"License checksum mismatch: {name}/{item['path']}")
            relative = Path(name) / item['path']
            if relative.is_absolute() or '..' in relative.parts:
                raise ValueError('Unsafe license path')
            destination = output / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
    # The base inventory still verifies all stock inputs. The shipped format
    # framework is replaced by Patina's checksum-pinned, guarded build.
    if any(artifact['name'] == 'Libavformat' for artifact in manifest['artifacts']):
        pin_path = Path(__file__).resolve().parents[1] / 'mpv-runtime-ios/runtime-ios.json'
        pin = json.loads(pin_path.read_text())
        artifact = next(item for item in manifest['artifacts'] if item['name'] == 'Libavformat')
        artifact['modified_from'] = {'url': artifact['url'], 'sha256': artifact['sha256']}
        artifact.update(url=pin['url'], sha256=pin['sha256'], source_url=pin['source_url'], source_sha256=pin['source_sha256'])
        artifact['modifications'] = 'Per-stream relay capability guard; exact patches and build configuration accompany the corresponding source.'
        (output / 'PATINA-LIBAVFORMAT-SOURCE.txt').write_text(
            f"Guarded Libavformat {pin['ffmpeg']}\nCorresponding source, patches, licenses and build configuration:\n"
            f"{pin['source_url']}\nSHA-256: {pin['source_sha256']}\n"
            "Other frameworks retain the MPVKit versions in components.json.\n")
    (output / 'components.json').write_text(json.dumps(manifest, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('target/notices/mpvkit'))
    args = parser.parse_args()
    here = Path(__file__).resolve().parent
    generate(here / 'mpvkit-licenses.json', args.output, here.parent / 'fetch-mpvkit.sh')
