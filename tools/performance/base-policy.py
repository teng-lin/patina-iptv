#!/usr/bin/env python3
"""Compare performance policy against event ancestry, never a replaced tag object."""
import os
from pathlib import Path
import re
import subprocess
import tempfile


def git(*arguments):
    return subprocess.check_output(['git', *arguments], text=True).strip()


def baseline(environment=os.environ):
    if git('rev-parse', '--is-shallow-repository') == 'true':
        subprocess.run(['git', 'fetch', '--no-tags', '--unshallow', 'origin'], check=True)
    event, ref = environment.get('PERF_EVENT', ''), environment.get('PERF_REF', '')
    # A tag push's before field is the previous tag target, which may be
    # unreachable (or unrelated). Its source parent is stable and reviewable.
    if event == 'pull_request':
        base = environment.get('PERF_BASE', '')
        if not base or set(base) == {'0'}:
            raise ValueError('pull request requires a performance base commit')
    elif event == 'merge_group':
        base = environment.get('PERF_BASE', '')
        if not base or set(base) == {'0'}:
            raise ValueError('merge group requires a performance base commit')
    elif event == 'push' and ref.startswith('refs/heads/'):
        base = environment.get('PERF_BASE', '')
    else:
        base = ''
    if not base or set(base) == {'0'}:
        parents = git('rev-list', '--parents', '-n', '1', 'HEAD').split()
        return parents[1] if len(parents) > 1 else None
    if not re.fullmatch('[0-9a-f]{40}', base):
        raise ValueError('performance base must be a full commit SHA')
    exists = subprocess.run(['git', 'cat-file', '-e', base + '^{commit}'], capture_output=True)
    if exists.returncode:
        subprocess.run(['git', 'fetch', '--no-tags', 'origin', base], check=True)
    git('cat-file', '-e', base + '^{commit}')
    return base


def main():
    output = Path('target/performance/summary.json')
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text('{"schema":1,"valid":false,"errors":["base policy or validation did not complete"]}\n')
    base = baseline()
    if base is None:
        print('Initial performance policy: source commit has no parent.')
    elif subprocess.run(['git', 'cat-file', '-e', base + ':tools/performance/baseline.json'], capture_output=True).returncode:
        print('Initial performance policy: the base revision has no baseline.')
    else:
        with tempfile.TemporaryDirectory(prefix='patina-performance-policy-') as temporary:
            directory = Path(temporary)
            for name in ('baseline.json', 'fixtures.json', 'journeys.py'):
                (directory / name).write_text(git('show', base + ':tools/performance/' + name) + '\n')
            subprocess.run(['python3', str(directory / 'journeys.py'), 'policy', str(directory / 'baseline.json'),
                            '--candidate', 'tools/performance/baseline.json', '--records', 'tools/performance/changes.json',
                            '--base-fixtures', str(directory / 'fixtures.json'), '--candidate-fixtures', 'tools/performance/fixtures.json',
                            '--report', str(output)], check=True)
    output.write_text('{"schema":1,"valid":false,"errors":["base policy passed; journeys did not complete"]}\n')


if __name__ == '__main__':
    main()
