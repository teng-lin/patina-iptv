#!/usr/bin/env python3
"""Retire older build caches after a successful master Validation run."""
import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import re
import subprocess


REPOSITORY = 'teng-lin/patina-iptv'
MASTER = 'refs/heads/master'
VALIDATION_PATH = '.github/workflows/validation.yml'
RETIRED = ('v0-rust-ios-compile-v1-', 'v0-rust-stress-v2-', 'v0-rust-fuzz-v2-', 'v0-rust-android-check-v2-',
           'v0-rust-desktop-package-v2-linux-')
RUST_HOST = r'(Linux-x64|Darwin-arm64|Windows_NT-x64)(?:-|$)'
RUST_FAMILY = re.compile(r'^v0-rust-(host-v2|coverage-v2)-.*-' + RUST_HOST)
RUST_COMPILER_FAMILY = re.compile(
    r'^v0-rust-(android-smoke-x86_64-v5|windows-native-v1|windows-installer-v1|'
    r'desktop-package-macos-v3)-([a-f0-9]{16}|[a-f0-9]{64})-.*-' + RUST_HOST
)
APT_FAMILY = re.compile(r'^apt-v4-(ubuntu[0-9.]+-[a-z0-9]+)-[a-f0-9]{16}-(.+)$')


def timestamp(value):
    return datetime.fromisoformat(value.replace('Z', '+00:00'))


def family(key):
    if match := RUST_COMPILER_FAMILY.match(key) or RUST_FAMILY.match(key):
        return ('rust', *match.groups())
    if match := APT_FAMILY.match(key):
        return ('apt', *match.groups())
    return None


def superseded(entries, start, finish):
    groups = {}
    retired = []
    for entry in entries:
        if entry['ref'] != MASTER:
            continue
        key = entry['key']
        if key.startswith(RETIRED) and timestamp(entry['last_accessed_at']) < start:
            retired.append(entry)
        elif group := family(key):
            groups.setdefault(group, []).append(entry)
    apt_replaced = False
    for group, caches in groups.items():
        newest = max(caches, key=lambda entry: (timestamp(entry['created_at']), entry['id']))
        replacement = timestamp(newest['created_at'])
        # A previous run may have retained a cache active at the time. Revisit
        # it on subsequent exact hits as well as when a new generation is saved.
        if replacement > finish:
            continue
        if group[:2] == ('apt', 'ubuntu24.04-amd64'):
            apt_replaced = True
        for entry in caches:
            if (timestamp(entry['created_at']) < replacement
                    and timestamp(entry['last_accessed_at']) < max(start, replacement)):
                retired.append(entry)
    if apt_replaced:
        retired.extend(entry for entry in entries if entry['ref'] == MASTER
                       and entry['key'].startswith('apt-v3-ubuntu24.04-amd64-')
                       and timestamp(entry['last_accessed_at']) < start)
    return retired


def validation_window(run):
    if (run['event'] != 'push' or run['head_branch'] != 'master'
            or run['path'].partition('@')[0] != VALIDATION_PATH
            or run['head_repository']['full_name'] != REPOSITORY or run['conclusion'] != 'success'):
        raise ValueError('Cache pruning requires a successful trusted master Validation run')
    return timestamp(run['run_started_at']), timestamp(run['updated_at'])


def pruning_window(environment):
    if environment.get('GITHUB_REPOSITORY') != REPOSITORY or environment.get('GITHUB_REF') != MASTER:
        raise ValueError('Cache pruning requires the master branch in the trusted repository')
    event_name = environment.get('GITHUB_EVENT_NAME')
    if event_name == 'workflow_run':
        run = json.loads(Path(environment['GITHUB_EVENT_PATH']).read_text())['workflow_run']
        window = validation_window(run)
        current = subprocess.check_output(
            ['gh', 'api', f'repos/{REPOSITORY}/git/ref/heads/master', '--jq', '.object.sha'], text=True, timeout=30,
        ).strip()
        if run['head_sha'] != current:
            print('Skipping pruning for an older master commit', flush=True)
            return None
        return window
    if event_name not in ('schedule', 'workflow_dispatch'):
        raise ValueError('Unsupported cache pruning event')

    # Rapid merges can make every completion event stale. Use a successful
    # window anyway, and leave replacements saved after that window alone.
    endpoint = f'repos/{REPOSITORY}/actions/workflows/{Path(VALIDATION_PATH).name}/runs'
    result = json.loads(subprocess.check_output(
        ['gh', 'api', '--method', 'GET', endpoint, '-f', 'branch=master', '-f', 'event=push',
         '-f', 'status=success', '-F', 'per_page=1'],
        timeout=30,
    ))
    runs = result['workflow_runs']
    if not runs:
        print('Skipping pruning until a master Validation run succeeds', flush=True)
        return None
    return validation_window(runs[0])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--apply', action='store_true', help='delete the identified cache entries')
    args = parser.parse_args()
    window = pruning_window(os.environ)
    if window is None:
        return
    endpoint = f'repos/{REPOSITORY}/actions/caches'
    pages = json.loads(subprocess.check_output(['gh', 'api', '--method', 'GET', '--paginate', '--slurp',
                                                endpoint, '-f', f'ref={MASTER}', '-F', 'per_page=100'], timeout=60))
    entries = [entry for page in pages for entry in page['actions_caches']]
    for entry in superseded(entries, *window):
        print(f'{"Deleting" if args.apply else "Would delete"} {entry["id"]}: {entry["key"]}', flush=True)
        if args.apply:
            try:
                subprocess.run(['gh', 'api', '--method', 'DELETE', f'{endpoint}/{int(entry["id"])}'], check=True, timeout=30)
            except (OSError, subprocess.SubprocessError) as error:
                print(f'::warning::Cache deletion did not complete ({type(error).__name__})')


if __name__ == '__main__':
    main()
