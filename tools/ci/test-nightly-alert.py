#!/usr/bin/env python3
"""Synthetic GitHub API contract tests; no network or provider fixtures."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('nightly', Path(__file__).with_name('nightly-alert.py'))
nightly = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(nightly)
ROOT = Path(__file__).resolve().parents[2]


def run(identifier=100, attempt=1, conclusion='failure', status='completed'):
    return {'id': identifier, 'run_attempt': attempt, 'event': 'schedule', 'head_branch': 'master',
            'path': nightly.WORKFLOW, 'head_repository': {'full_name': nightly.REPOSITORY},
            'created_at': f'2026-11-{identifier - 80:02}T11:17:00Z',
            'conclusion': conclusion, 'status': status}


def jobs():
    return [{'id': 1, 'name': 'host / host-tests (ubuntu-24.04)', 'conclusion': 'success', 'steps': []},
            {'id': 2, 'name': 'packages / desktop-packages (macos, macos-15)', 'conclusion': 'cancelled',
             'steps': [], 'check_run_url': 'https://api.github.com/repos/example/project/check-runs/2'},
            {'id': 3, 'name': 'validation', 'conclusion': 'failure',
             'steps': [{'name': 'Require every selected lane to pass', 'conclusion': 'failure'}]}]


class MockAPI:
    def __init__(self):
        self.runs = [run()]
        self.jobs = jobs()
        self.annotations = [{'annotation_level': 'failure', 'message': nightly.RUNNER_UNAVAILABLE}]
        self.issue = None
        self.calls = []
        self.fail = None

    def __call__(self, path, method='GET', data=None, paginate=False):
        self.calls.append((path, method, copy.deepcopy(data), paginate))
        if self.fail and self.fail in path:
            raise subprocess.CalledProcessError(1, 'synthetic GitHub API denial')
        if path.startswith('issues?'):
            return [[self.issue] if self.issue else []]
        if path == 'issues' and method == 'POST':
            self.issue = dict(data, number=5, state='open')
            return copy.deepcopy(self.issue)
        if path == 'issues/5' and method == 'PATCH':
            self.issue.update(copy.deepcopy(data))
            return copy.deepcopy(self.issue)
        if path.startswith('actions/workflows/'):
            return {'workflow_runs': copy.deepcopy(self.runs)}
        if path.endswith('/jobs?per_page=100'):
            return [{'jobs': copy.deepcopy(self.jobs)}]
        if path.startswith('check-runs/'):
            return [copy.deepcopy(self.annotations)]
        if path.endswith('/rerun-failed-jobs'):
            return None
        if path.startswith('actions/runs/'):
            identifier = int(path.rsplit('/', 1)[1])
            return copy.deepcopy(next(item for item in self.runs if item['id'] == identifier))
        raise AssertionError(path)

    def retries(self):
        return [call for call in self.calls if call[0].endswith('/rerun-failed-jobs')]


class ObserverTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.event_path = Path(self.temp.name) / 'event.json'
        self.env = {'GITHUB_REPOSITORY': nightly.REPOSITORY, 'GITHUB_REF': 'refs/heads/master',
                    'GITHUB_EVENT_NAME': 'workflow_run', 'GITHUB_EVENT_PATH': str(self.event_path)}
        self.client = MockAPI()

    def observe(self, event=None):
        self.event_path.write_text(json.dumps({'workflow_run': event or self.client.runs[0]}))
        return nightly.observe(self.client, self.env)

    def test_one_tracker_one_retry_for_duplicate_delivery_and_observer_rerun(self):
        self.observe()
        self.observe()
        self.assertEqual(len(self.client.retries()), 1)
        self.assertEqual(sum(call[1] == 'POST' and call[0] == 'issues' for call in self.client.calls), 1)
        retry = self.client.retries()[0]
        self.assertEqual(retry[:3], ('actions/runs/100/rerun-failed-jobs', 'POST', {}))
        self.assertTrue(any('/attempts/1/jobs?' in call[0] and call[3] for call in self.client.calls))
        self.assertIn('480 min', self.client.issue['body'])
        self.assertIn('validation', self.client.issue['body'])

    def test_second_attempt_remains_red_without_another_retry(self):
        self.observe()
        self.client.runs[0]['run_attempt'] = 2
        self.observe()
        self.assertEqual(len(self.client.retries()), 1)
        self.assertEqual(self.client.issue['state'], 'open')
        self.assertIn('attempt 2): **failure**', self.client.issue['body'])

    def test_successful_retry_closes_and_next_failed_schedule_reopens_same_issue(self):
        self.observe()
        self.client.runs[0].update(run_attempt=2, conclusion='success')
        self.client.jobs = [dict(item, conclusion='success') for item in jobs()]
        self.observe()
        self.assertEqual(self.client.issue['state'], 'closed')
        self.client.runs.insert(0, run(101))
        self.client.jobs = jobs()
        self.observe()
        self.assertEqual(self.client.issue['number'], 5)
        self.assertEqual(self.client.issue['state'], 'open')
        self.assertEqual(len(self.client.retries()), 2)

    def test_initial_success_does_not_open_issue(self):
        self.client.runs[0]['conclusion'] = 'success'
        self.client.jobs = [dict(item, conclusion='success') for item in jobs()]
        self.observe()
        self.assertIsNone(self.client.issue)

    def test_product_failure_timeout_manual_cancel_missing_evidence_do_not_retry(self):
        cases = [
            ('failure', [], self.client.annotations),
            ('timed_out', [], self.client.annotations),
            ('cancelled', [], []),
            ('cancelled', [], [{'annotation_level': 'failure', 'message': 'The operation was canceled.'}]),
            ('cancelled', [{'name': 'Build', 'conclusion': 'failure'}], self.client.annotations),
            ('cancelled', [], self.client.annotations + [
                {'annotation_level': 'failure', 'message': 'The job has exceeded the maximum execution time'}]),
        ]
        for conclusion, steps, annotations in cases:
            with self.subTest(conclusion=conclusion, steps=steps, annotations=annotations):
                self.client = MockAPI()
                self.client.jobs[1].update(conclusion=conclusion, steps=steps)
                self.client.annotations = annotations
                self.observe()
                self.assertFalse(self.client.retries())
                self.assertEqual(self.client.issue['state'], 'open')

    def test_mixed_product_failure_and_runner_loss_never_retry(self):
        self.client.jobs[0]['conclusion'] = 'failure'
        self.observe()
        self.assertFalse(self.client.retries())


    def test_missing_steps_or_independent_gate_failure_never_retry(self):
        for job_index, change in ((1, {'steps': None}), (2, {'steps': []}),
                                  (2, {'conclusion': 'timed_out'}),
                                  (2, {'steps': [{'name': 'Checkout', 'conclusion': 'failure'}]})):
            with self.subTest(job_index=job_index, change=change):
                self.client = MockAPI()
                self.client.jobs[job_index].update(change)
                self.observe()
                self.assertFalse(self.client.retries())

    def test_whole_run_deliberately_cancelled_never_retry(self):
        self.client.runs[0]['conclusion'] = 'cancelled'
        self.observe()
        self.assertFalse(self.client.retries())

    def test_retry_api_failure_leaves_durable_claim_and_is_not_repeated(self):
        self.client.fail = '/rerun-failed-jobs'
        with self.assertRaises(subprocess.CalledProcessError):
            self.observe()
        self.assertIn('claimed once', self.client.issue['body'])
        self.client.fail = None
        self.observe()
        self.assertEqual(len(self.client.retries()), 1)

    def test_issue_write_or_annotation_failure_never_retries(self):
        for failure in ('check-runs/', 'issues'):
            with self.subTest(failure=failure):
                self.client = MockAPI()
                self.client.fail = failure
                with self.assertRaises(subprocess.CalledProcessError):
                    self.observe()
                self.assertFalse(self.client.retries())

    def test_no_jobs_fails_closed(self):
        self.client.jobs = []
        with self.assertRaisesRegex(ValueError, 'Missing jobs'):
            self.observe()
        self.assertFalse(self.client.retries())

    def test_out_of_order_events_and_coalesced_pending_observers_preserve_newest_result(self):
        self.client.runs = [run(101, conclusion='success'), run()]
        self.client.jobs = [dict(item, conclusion='success') for item in jobs()]
        self.observe(event=run())
        self.assertFalse(self.client.retries())
        self.assertEqual(self.client.issue['state'], 'closed')
        self.assertIn('Latest scheduled run: [101]', self.client.issue['body'])
        self.observe(event=run())
        self.assertIn('Latest scheduled run: [101]', self.client.issue['body'])

    def test_trust_boundaries(self):
        for change in ({'event': 'pull_request'}, {'head_branch': 'feature'},
                       {'head_repository': {'full_name': 'example/fork'}},
                       {'path': '.github/workflows/other.yml'}, {'status': 'in_progress'}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.observe(event=run() | change)
        self.env['GITHUB_REF'] = 'refs/heads/feature'
        with self.assertRaises(ValueError):
            self.observe()
        self.assertFalse(self.client.calls)

    def test_corrupt_journal_fails_closed(self):
        self.client.issue = {'number': 5, 'body': nightly.MARKER, 'state': 'open'}
        with self.assertRaisesRegex(ValueError, 'journal'):
            self.observe()
        self.assertFalse(self.client.retries())

    def test_api_uses_json_input_and_propagates_errors(self):
        with patch.object(nightly.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, '{}')) as command:
            nightly.api('issues', 'POST', {'body': 'synthetic $() `text`\nnext'})
        self.assertEqual(json.loads(command.call_args.kwargs['input'])['body'], 'synthetic $() `text`\nnext')
        self.assertTrue(command.call_args.kwargs['check'])
        self.assertEqual(command.call_args.kwargs['timeout'], 60)

    def test_workflow_serializes_trusted_observers_without_touching_validation(self):
        workflow = (ROOT / '.github/workflows/nightly-alert.yml').read_text()
        self.assertIn('group: nightly-validation-tracker\n  cancel-in-progress: false', workflow)
        self.assertIn('ref: master', workflow)
        self.assertNotIn('ref: ${{ github.event.workflow_run.head_sha', workflow)
        self.assertNotIn('actions/download-artifact', workflow)
        self.assertIn("github.event.workflow_run.event == 'schedule'", workflow)
        for permission in ('actions: write', 'issues: write', 'checks: read'):
            self.assertIn(permission, workflow)


if __name__ == '__main__':
    unittest.main()
