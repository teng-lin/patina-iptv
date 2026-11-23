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
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('validation_plan', ROOT / 'tools/ci/validation-plan.py')
validation = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validation)
VERSION_SPEC = importlib.util.spec_from_file_location('release_version', ROOT / 'tools/ci/version.py')
versioning = importlib.util.module_from_spec(VERSION_SPEC)
VERSION_SPEC.loader.exec_module(versioning)
PLAN_SPEC = importlib.util.spec_from_file_location('release_plan', ROOT / 'tools/ci/release-plan.py')
release_plan = importlib.util.module_from_spec(PLAN_SPEC)
PLAN_SPEC.loader.exec_module(release_plan)
REQUIRED = {'signing-input-android', 'signing-input-macos', 'signing-input-windows',
            'packages-linux', 'packages-sbom', 'packages-linux-media-sbom',
            'packages-macos-media-sbom', 'packages-windows-media-sbom', 'packages-linux-native-sources'}
OPTIONAL = {'packages-macos-native-sources'}
PROMOTION_JOBS = {
    'plan', 'validation', 'candidate / sbom', 'candidate / windows-package',
    'candidate / package (android, ubuntu-24.04)', 'candidate / package (linux, ubuntu-24.04)',
    'candidate / package (macos, macos-15)',
}
LEGACY_ANDROID_JOB = 'candidate / package (android, ubuntu-24.04)'
SPLIT_ANDROID_JOBS = {'candidate / android-native (arm64-v8a)', 'candidate / android-native (armeabi-v7a)',
                      'candidate / android-package'}
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
    if missing := REQUIRED - groups:
        raise ValueError('missing candidate artifact groups: ' + ', '.join(sorted(missing)))
    if unexpected := groups - REQUIRED - OPTIONAL:
        raise ValueError('unexpected candidate artifact groups: ' + ', '.join(sorted(unexpected)))
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


def tag_commit(tag, root=ROOT):
    ref = f'refs/tags/{tag}'
    output = subprocess.check_output(['git', 'ls-remote', 'origin', ref, ref + '^{}'],
                                     cwd=root, text=True)
    refs = {name: sha for sha, name in (line.split() for line in output.splitlines())}
    commit = refs.get(ref + '^{}', refs.get(ref))
    if not re.fullmatch(r'[0-9a-f]{40}', commit or ''):
        raise ValueError(f'release tag {tag} does not exist on origin')
    return commit


def promotion_inputs(repository, run_id, commit):
    base = f'repos/{repository}/actions/runs/{run_id}'
    run = api(base)
    if str(run.get('id')) != run_id or not trusted(run, repository, commit):
        raise ValueError('promotion requires a successful master Validation push at the tagged commit')
    attempt = run['run_attempt']
    result = api(f'{base}/attempts/{attempt}/jobs?per_page=100')
    candidates = PROMOTION_JOBS | SPLIT_ANDROID_JOBS
    checks = {job['name']: job['conclusion'] for job in result['jobs'] if job['name'] in candidates}
    required = PROMOTION_JOBS
    if SPLIT_ANDROID_JOBS & checks.keys():
        required = (PROMOTION_JOBS - {LEGACY_ANDROID_JOB}) | SPLIT_ANDROID_JOBS
    if checks != dict.fromkeys(required, 'success'):
        raise ValueError('master validation and every unsigned package build must have passed')
    result = api(f'{base}/artifacts?per_page=100')
    if result['total_count'] != len(result['artifacts']):
        raise ValueError('too many artifacts to verify the candidate inventory')
    artifacts = {}
    for artifact in result['artifacts']:
        name = artifact['name']
        if name not in REQUIRED | OPTIONAL:
            continue
        source = artifact['workflow_run']
        if (name in artifacts or artifact['expired'] or source['head_sha'] != commit
                or str(source['id']) != run_id or not re.fullmatch(r'sha256:[0-9a-f]{64}', artifact['digest'] or '')):
            raise ValueError(f'invalid or ambiguous candidate artifact {name}')
        artifacts[name] = {'id': artifact['id'], 'digest': artifact['digest']}
    if missing := REQUIRED - artifacts.keys():
        raise ValueError('the master run is missing unsigned artifacts: ' + ', '.join(sorted(missing)))
    return attempt, checks, artifacts


def unpack_zip(archive, group):
    with zipfile.ZipFile(archive) as bundle:
        for entry in bundle.infolist():
            # These workflow artifacts are flat. Do not follow links or let a
            # file escape its group before it is staged for a signing job.
            name = entry.filename
            if (entry.is_dir() or not re.fullmatch(r'[A-Za-z0-9_.-]+', name)
                    or name in {'.', '..'} or stat.S_IFMT(entry.external_attr >> 16) not in {0, stat.S_IFREG}):
                raise ValueError(f'unexpected artifact member {name!r}')
            with bundle.open(entry) as source, (group / name).open('xb') as destination:
                shutil.copyfileobj(source, destination)


def promote(directory, environment=os.environ, root=ROOT):
    tag, run_id = environment['PROMOTE_TAG'], environment['VALIDATION_RUN']
    release_plan.check_promotion(environment['GITHUB_EVENT_NAME'], environment['GITHUB_REF'],
                                 'android-desktop', 'false', tag, run_id)
    repository = environment['GH_REPO']
    commit = tag_commit(tag, root)
    publisher = environment['GITHUB_SHA']
    subprocess.run(['git', 'merge-base', '--is-ancestor', commit, publisher], cwd=root, check=True)
    attempt, checks, artifacts = promotion_inputs(repository, run_id, commit)
    with tempfile.TemporaryDirectory(prefix='patina-promotion-') as temporary:
        stage = Path(temporary)
        source_tree = stage / 'source'
        subprocess.run(['git', 'worktree', 'add', '--detach', str(source_tree), commit],
                       cwd=root, stdout=sys.stderr, check=True)
        try:
            release_plan.check_changelog(source_tree, f'refs/tags/{tag}')
            inputs = identity(source_tree)
            build = build_identity(source_tree)
            plan = validation.select([], 'release', source_tree, candidate_build=True)
        finally:
            subprocess.run(['git', 'worktree', 'remove', str(source_tree)],
                           cwd=root, stdout=sys.stderr, check=True)
        directory.mkdir(parents=True)
        for name, artifact in sorted(artifacts.items()):
            archive = stage / 'artifact.zip'
            with archive.open('wb') as stream:
                subprocess.run(['gh', 'api', f'repos/{repository}/actions/artifacts/{artifact["id"]}/zip'],
                               stdout=stream, check=True)
            if 'sha256:' + digest(archive) != artifact['digest']:
                raise ValueError(f'GitHub artifact checksum mismatch for {name}')
            group = directory / name
            group.mkdir()
            unpack_zip(archive, group)
        if tag_commit(tag, root) != commit:
            raise ValueError(f'release tag {tag} moved while staging artifacts')
        manifest = {'schema': 1, 'repository': repository, 'sha': commit, 'tag': tag,
                    'run_id': int(run_id), 'attempt': attempt, 'inputs': inputs, 'build': build,
                    'plan': plan, 'checks': checks, 'artifacts': artifacts, 'files': inventory(directory),
                    'publisher': {'sha': publisher, 'ref': environment['GITHUB_REF']}}
        (directory / 'manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
    return commit, build['version']


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
    commands = parser.add_subparsers(dest='action', required=True)
    for action in ('seal', 'select'):
        commands.add_parser(action).add_argument('directory', type=Path)
    check = commands.add_parser('check-tag')
    check.add_argument('tag')
    check.add_argument('commit')
    args = parser.parse_args()
    if args.action == 'seal':
        seal(args.directory)
    elif args.action == 'check-tag':
        if tag_commit(args.tag) != args.commit:
            raise ValueError(f'release tag {args.tag} moved')
    elif os.environ.get('PROMOTE_TAG'):
        commit, version = promote(args.directory)
        print(f'run={os.environ["VALIDATION_RUN"]}\ncommit={commit}\nversion={version}')
    else:
        print('run=' + select(args.directory))


if __name__ == '__main__':
    main()
