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
RETIRED = ('v0-rust-ios-compile-v1-', 'v0-rust-stress-v2-', 'v0-rust-fuzz-v2-', 'v0-rust-android-check-v2-')
RUST_FAMILY = re.compile(
    r'^v0-rust-(host-v2|coverage-v2|android-smoke-x86_64-v5|windows-native-v1|windows-installer-v1|'
    r'desktop-package-v2-linux|desktop-package-macos-v3)-.*-(Linux-x64|Darwin-arm64|Windows_NT-x64)(?:-|$)'
)
APT_FAMILY = re.compile(r'^apt-v4-(ubuntu[0-9.]+-[a-z0-9]+)-[a-f0-9]{16}-(.+)$')


def timestamp(value):
    return datetime.fromisoformat(value.replace('Z', '+00:00'))


def family(key):
    if match := RUST_FAMILY.match(key):
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


def trusted_window(event, environment):
    run = event['workflow_run']
    if (environment.get('GITHUB_REPOSITORY') != REPOSITORY or environment.get('GITHUB_REF') != MASTER
            or environment.get('GITHUB_EVENT_NAME') != 'workflow_run' or run['event'] != 'push'
            or run['head_branch'] != 'master' or run['path'] != '.github/workflows/validation.yml'
            or run['head_repository']['full_name'] != REPOSITORY or run['conclusion'] != 'success'):
        raise ValueError('Cache pruning requires a successful trusted master Validation run')
    return timestamp(run['run_started_at']), timestamp(run['updated_at'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--apply', action='store_true', help='delete the identified cache entries')
    args = parser.parse_args()
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
    start, finish = trusted_window(event, os.environ)
    current = subprocess.check_output(
        ['gh', 'api', f'repos/{REPOSITORY}/git/ref/heads/master', '--jq', '.object.sha'], text=True, timeout=30,
    ).strip()
    if event['workflow_run']['head_sha'] != current:
        print('Skipping pruning for an older master commit', flush=True)
        return
    endpoint = f'repos/{REPOSITORY}/actions/caches'
    pages = json.loads(subprocess.check_output(['gh', 'api', '--method', 'GET', '--paginate', '--slurp',
                                                endpoint, '-f', f'ref={MASTER}', '-F', 'per_page=100'], timeout=60))
    entries = [entry for page in pages for entry in page['actions_caches']]
    for entry in superseded(entries, start, finish):
        print(f'{"Deleting" if args.apply else "Would delete"} {entry["id"]}: {entry["key"]}', flush=True)
        if args.apply:
            try:
                subprocess.run(['gh', 'api', '--method', 'DELETE', f'{endpoint}/{int(entry["id"])}'], check=True, timeout=30)
            except (OSError, subprocess.SubprocessError) as error:
                print(f'::warning::Cache deletion did not complete ({type(error).__name__})')


if __name__ == '__main__':
    main()
