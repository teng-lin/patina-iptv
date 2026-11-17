#!/usr/bin/env python3
"""Seal trusted master candidates and select exact-SHA unsigned release inputs.

GitHub's successful default-branch workflow identity is the trust boundary. A
manifest from a PR, another workflow, another SHA, or another attempt cannot
replace full candidate validation. Artifact expiry falls back to fresh checks;
invalid evidence fails closed instead of silently rebuilding over it.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('validation_plan', ROOT / 'tools/ci/validation-plan.py')
validation = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validation)
VERSION_SPEC = importlib.util.spec_from_file_location('release_version', ROOT / 'tools/ci/version.py')
versioning = importlib.util.module_from_spec(VERSION_SPEC)
VERSION_SPEC.loader.exec_module(versioning)
REQUIRED = {'signing-input-android', 'signing-input-macos', 'signing-input-windows',
            'packages-linux', 'packages-sbom', 'packages-linux-media-sbom',
            'packages-macos-media-sbom', 'packages-windows-media-sbom', 'packages-linux-native-sources'}
OPTIONAL = {'packages-macos-native-sources'}
INPUTS = ('Cargo.lock', 'Cargo.toml', 'nsh.rev', 'rust-toolchain.toml', 'tools/ci/package.Cargo.lock',
          '.github/workflows/unsigned-candidate.yml', '.github/workflows/validation.yml',
          'tools/ci/check-plan.py', 'tools/ci/signing-policy.json', 'tools/ci/version.py', '.cargo/config.toml',
          'tools/mpv-runtime/runtime.json', 'tools/mpv-runtime-unix/runtime-linux.json',
          'tools/mpv-runtime-unix/runtime-macos.json')


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def identity(root=ROOT):
    return {name: digest(root / name) for name in INPUTS}


def build_identity(root=ROOT):
    version = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['package']['version']
    return {'profile': 'release', 'android_abis': ['arm64-v8a', 'armeabi-v7a'],
            'credentials': False, 'recipe': '.github/workflows/unsigned-candidate.yml',
            'flags': {'cargo': '--release --locked', 'IPTV_UNSIGNED_RELEASE': '1'},
            'version': version, 'android_version_code': versioning.android_version_code(version),
            'nsh': (root / 'nsh.rev').read_text().strip(),
            'toolchain': (root / 'rust-toolchain.toml').read_text()}


def inventory(directory):
    groups = {path.name for path in directory.iterdir() if path.name != 'manifest.json'}
    if not REQUIRED <= groups or groups - REQUIRED - OPTIONAL:
        raise ValueError('candidate artifact groups are missing or unexpected')
    files = {}
    for group in sorted(groups):
        folder = directory / group
        if not folder.is_dir() or folder.is_symlink():
            raise ValueError('candidate artifact group must be a directory')
        members = list(folder.rglob('*'))
        if not any(path.is_file() for path in members):
            raise ValueError('empty candidate artifact group')
        for path in members:
            if path.is_symlink() or not (path.is_file() or path.is_dir()):
                raise ValueError('candidate contains a symlink or special file')
            if path.is_file():
                files[path.relative_to(directory).as_posix()] = digest(path)
    return files


def trusted(run, repository, sha):
    return (run.get('event') == 'push' and run.get('head_branch') == 'master'
            and run.get('head_sha') == sha and run.get('status') == 'completed'
            and run.get('conclusion') == 'success' and run.get('path') == '.github/workflows/validation.yml'
            and run.get('head_repository', {}).get('full_name') == repository)


def seal(directory, environment=os.environ, root=ROOT):
    if environment.get('GITHUB_EVENT_NAME') != 'push' or environment.get('GITHUB_REF') != 'refs/heads/master':
        raise ValueError('only a master push can seal a candidate')
    plan = json.loads(environment['PLAN'])
    needs = json.loads(environment['NEEDS'])
    if plan != validation.select([], 'release', root, candidate_build=True) or any(needs.get(job, {}).get('result') != 'success'
                                                          for job in ('plan', 'validation', 'candidate')):
        raise ValueError('full candidate validation and unsigned packages must pass before sealing')
    manifest = {'schema': 1, 'repository': environment['GITHUB_REPOSITORY'], 'sha': environment['GITHUB_SHA'],
                'run_id': int(environment['GITHUB_RUN_ID']), 'attempt': int(environment['GITHUB_RUN_ATTEMPT']),
                'inputs': identity(root), 'plan': plan, 'checks': {job: needs[job]['result'] for job in needs},
                'build': build_identity(root),
                'files': inventory(directory)}
    (directory / 'manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
    return manifest


def verify(directory, run, repository, sha, root=ROOT):
    if not trusted(run, repository, sha):
        raise ValueError('candidate did not come from successful exact-SHA master validation')
    manifest = json.loads((directory / 'manifest.json').read_text())
    if (manifest.get('schema') != 1 or manifest.get('repository') != repository or manifest.get('sha') != sha
            or manifest.get('run_id') != run['id'] or manifest.get('attempt') != run['run_attempt']
            or manifest.get('inputs') != identity(root) or manifest.get('build') != build_identity(root) or manifest.get('plan') != validation.select([], 'release', root, candidate_build=True)
            or manifest.get('checks') != {'plan': 'success', 'validation': 'success', 'candidate': 'success'}):
        raise ValueError('candidate manifest identity or checks do not match')
    if inventory(directory) != manifest.get('files'):
        raise ValueError('candidate bytes do not match the immutable manifest')
    return manifest


def api(path):
    return json.loads(subprocess.check_output(['gh', 'api', path], text=True))


def select(directory, environment=os.environ):
    if environment.get('CANDIDATE_REUSE') != 'true':
        return ''
    repository, sha = environment['GH_REPO'], environment['GITHUB_SHA']
    # Only this workflow, exact SHA and successful trusted push runs; never PR artifacts.
    runs = api(f'repos/{repository}/actions/workflows/validation.yml/runs?head_sha={sha}&event=push&status=success&per_page=100')['workflow_runs']
    for run in runs:
        if not trusted(run, repository, sha):
            continue
        artifacts = api(f'repos/{repository}/actions/runs/{run["id"]}/artifacts?per_page=100')['artifacts']
        candidates = [item for item in artifacts if item['name'] == 'release-candidate' and not item['expired']]
        if not candidates:
            continue
        if len(candidates) != 1:
            raise ValueError('ambiguous release candidate artifact')
        subprocess.run(['gh', 'run', 'download', str(run['id']), '--repo', repository,
                        '--name', 'release-candidate', '--dir', str(directory)], check=True)
        verify(directory, run, repository, sha)
        print(f'Verified trusted candidate run {run["id"]}, SHA {sha}', file=sys.stderr)
        return str(run['id'])
    print('No retained exact-SHA trusted candidate; full validation and unsigned build are required.', file=sys.stderr)
    return ''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('seal', 'select'))
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    if args.action == 'seal':
        seal(args.directory)
    else:
        print('run=' + select(args.directory))


if __name__ == '__main__':
    main()
