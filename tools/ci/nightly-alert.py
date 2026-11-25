#!/usr/bin/env python3
"""Reconcile scheduled Validation failures into one issue; retry verified runner acquisition loss once.

Only trusted default-branch code runs in the observer. No artifacts or log contents
are executed. The workflow serializes issue writes; issue state journals retry
claims before the API call so an ambiguous response cannot cause a second retry.
"""
import json
import os
from pathlib import Path
import re
import subprocess

REPOSITORY = 'teng-lin/patina-iptv'
WORKFLOW = '.github/workflows/validation.yml'
MARKER = '<!-- patina-nightly-validation:v1 -->'
STATE = re.compile(r'<!-- patina-nightly-state:(.+?) -->')
TITLE = 'CI: scheduled Validation failures'
RUNNER_UNAVAILABLE = 'The job was not acquired by Runner of type hosted even after multiple attempts'
PROXIES = {'validation', 'android', 'test (ubuntu-24.04)', 'test (macos-15)'}


def api(path, method='GET', data=None, paginate=False):
    command = ['gh', 'api', '--method', method]
    if paginate:
        command += ['--paginate', '--slurp']
    command += [f'repos/{REPOSITORY}/{path}']
    if data is not None:
        command += ['--input', '-']
    result = subprocess.run(command, input=None if data is None else json.dumps(data),
                            text=True, capture_output=True, check=True, timeout=60)
    return json.loads(result.stdout) if result.stdout.strip() else None


def trusted(run):
    return (run.get('event') == 'schedule' and run.get('head_branch') == 'master'
            and run.get('path', '').partition('@')[0] == WORKFLOW
            and run.get('head_repository', {}).get('full_name') == REPOSITORY)


def key(run):
    return f'{int(run["id"])}:{int(run["run_attempt"])}'


def order(record):
    return record['created_at'], record['id'], record['attempt']


def classify(jobs, annotations):
    """Missing evidence, timeouts, product failures and deliberate cancels never retry."""
    primary = [job for job in jobs if job['name'] not in PROXIES]
    unsuccessful = [job for job in primary if job['conclusion'] not in ('success', 'skipped')]
    if not unsuccessful:
        return False
    for job in unsuccessful:
        # This observed GitHub platform annotation is positive evidence. A missing
        # log, a generic cancellation or an elapsed-time guess is insufficient.
        failures = [a['message'] for a in annotations.get(job['id'], [])
                    if a.get('annotation_level') == 'failure']
        if (job['conclusion'] != 'cancelled' or job.get('steps') != []
                or failures != [RUNNER_UNAVAILABLE]):
            return False
    # Exempt only the expected gate assertion, never checkout failures, gate
    # timeouts or missing step evidence on an otherwise recognized proxy name.
    for job in jobs:
        if job['name'] not in PROXIES or job['conclusion'] in ('success', 'skipped'):
            continue
        expected = ('Require every selected lane to pass' if job['name'] == 'validation'
                    else 'Run test \"$RESULT\" = success')
        failed_steps = [step['name'] for step in job.get('steps', [])
                        if step.get('conclusion') not in ('success', 'skipped')]
        if job['conclusion'] != 'failure' or failed_steps != [expected]:
            return False
    return True


def read_state(issue):
    if issue is None:
        return {}
    match = STATE.search(issue['body'] or '')
    if not match:
        raise ValueError('Tracking issue has no state journal; refusing to discard retry history')
    state = json.loads(match[1])
    if not isinstance(state, dict):
        raise ValueError('Invalid nightly state journal')
    return state


def body(state):
    records = sorted(state.values(), key=order, reverse=True)
    latest = records[0]
    lines = [MARKER, 'Managed by the nightly observer. Refs #337.', '',
             f'Latest scheduled run: [{latest["id"]}](https://github.com/{REPOSITORY}/actions/runs/{latest["id"]}) '
             f'(attempt {latest["attempt"]}): **{latest["conclusion"]}**.', '',
             'Schedule delay is measured from 03:17 UTC to run creation; it is separate from job failures.', '',
             '| Run / attempt | Created (UTC) | Schedule delay | Result | Retry |',
             '|---|---|---|---|---|']
    for record in records[:20]:
        lines.append(f'| [{record["id"]}](https://github.com/{REPOSITORY}/actions/runs/{record["id"]}) / '
                     f'{record["attempt"]} | {record["created_at"]} | {record["delay_minutes"]} min | '
                     f'{record["conclusion"]} | {record["retry"]} |')
    lines += ['', 'Non-successful jobs in the latest attempt:']
    lines += [f'- [{job["name"]}](https://github.com/{REPOSITORY}/actions/runs/{latest["id"]}/job/{job["id"]}): '
              f'{job["conclusion"]}' for job in latest['jobs']]
    lines += ['', 'Retry requires a cancelled job with no steps and the explicit GitHub runner-acquisition '
              'failure annotation. Product failures, timeouts, missing evidence and manual cancellations stay red. '
              'A retry uses re-run-failed-jobs, including dependent gates; no selected lane is removed.', '',
              '<!-- patina-nightly-state:' + json.dumps(state, separators=(',', ':')) + ' -->']
    return '\n'.join(lines) + '\n'


def save(issue, state, client):
    latest = max(state.values(), key=order)
    data = {'title': TITLE, 'body': body(state),
            'state': 'closed' if latest['conclusion'] == 'success' else 'open'}
    if issue is None:
        # Do not create a failure tracker merely to report healthy history.
        if latest['conclusion'] == 'success':
            return None
        return client('issues', 'POST', {k: v for k, v in data.items() if k != 'state'})
    return client(f'issues/{int(issue["number"])}', 'PATCH', data)


def observe(client=api, environment=None):
    from datetime import datetime

    environment = os.environ if environment is None else environment
    if (environment.get('GITHUB_REPOSITORY') != REPOSITORY
            or environment.get('GITHUB_REF') != 'refs/heads/master'
            or environment.get('GITHUB_EVENT_NAME') != 'workflow_run'):
        raise ValueError('Nightly observer requires a trusted master workflow_run event')
    event = json.loads(Path(environment['GITHUB_EVENT_PATH']).read_text())['workflow_run']
    if not trusted(event) or event.get('status') != 'completed':
        raise ValueError('Only completed scheduled master Validation runs may be observed')

    issues = [issue for page in client('issues?state=all&creator=github-actions%5Bbot%5D&per_page=100', paginate=True)
              for issue in page if MARKER in (issue.get('body') or '') and 'pull_request' not in issue]
    if len(issues) > 1:
        raise ValueError('Duplicate nightly trackers require manual reconciliation')
    issue = issues[0] if issues else None
    state = read_state(issue)
    # Reconcile recent schedules as well as the event: default concurrency can
    # replace a pending observer. This also makes out-of-order delivery harmless.
    runs = client('actions/workflows/validation.yml/runs?event=schedule&branch=master&per_page=10')['workflow_runs']
    runs = {int(run['id']): run for run in runs if trusted(run)}
    current = client(f'actions/runs/{int(event["id"])}')
    if not trusted(current):
        raise ValueError('Run identity changed')
    runs[current['id']] = current
    newest = max(runs.values(), key=lambda run: (run['created_at'], run['id']))
    changed = False
    retry_run = None
    for run in sorted(runs.values(), key=lambda run: (run['created_at'], run['id'])):
        if run['status'] != 'completed' or key(run) in state:
            continue
        pages = client(f'actions/runs/{int(run["id"])}/attempts/{int(run["run_attempt"])}/jobs?per_page=100',
                       paginate=True)
        jobs = [job for page in pages for job in page['jobs']]
        if not jobs:
            raise ValueError('Missing jobs; refusing to infer a nightly result')
        annotations = {}
        for job in jobs:
            if job['conclusion'] == 'cancelled' and job['name'] not in PROXIES:
                check_id = int(job['check_run_url'].rsplit('/', 1)[1])
                annotation_pages = client(f'check-runs/{check_id}/annotations?per_page=100', paginate=True)
                annotations[job['id']] = [a for page in annotation_pages for a in page]
        created = datetime.fromisoformat(run['created_at'].replace('Z', '+00:00'))
        scheduled = created.replace(hour=3, minute=17, second=0)
        # Before today's cron slot a run belongs to the preceding day's schedule.
        if scheduled > created:
            from datetime import timedelta
            scheduled -= timedelta(days=1)
        record = {'id': run['id'], 'attempt': run['run_attempt'], 'created_at': run['created_at'],
                  'conclusion': run['conclusion'], 'delay_minutes': int((created - scheduled).total_seconds() // 60),
                  'retry': 'not eligible', 'jobs': [
                      {k: job[k] for k in ('id', 'name', 'conclusion')}
                      for job in jobs if job['conclusion'] not in ('success', 'skipped')]}
        eligible = (run['id'] == newest['id'] and run['run_attempt'] == 1
                    and run['conclusion'] == 'failure' and classify(jobs, annotations))
        if eligible:
            record['retry'] = 'claimed once (API outcome may be unknown)'
            retry_run = run
        state[key(run)] = record
        changed = True
    if changed:
        # Reconcile the whole window before publishing its latest result. Saving
        # historical failures one by one could briefly reopen a healthy tracker
        # or leave it stale if a later API request fails.
        state = dict(sorted(state.items(), key=lambda pair: order(pair[1]))[-30:])
        issue = save(issue, state, client)
    if retry_run is not None:
        # The complete journal, including this claim, is durable before the POST.
        client(f'actions/runs/{int(retry_run["id"])}/rerun-failed-jobs', 'POST', {})
        state[key(retry_run)]['retry'] = 'requested once'
        save(issue, state, client)
    return state


if __name__ == '__main__':
    observe()
