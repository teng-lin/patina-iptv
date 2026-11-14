#!/usr/bin/env python3
"""A rehearsal only makes and removes what is provably its own, and never
takes a failed call for "nothing there"."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("rehearsal.py")
REPOSITORY = "synthetic-owner/synthetic-repo"
TAG = "rehearsal-1001"
REF = f"refs/tags/{TAG}"
SHA = "1f" * 20
OTHER_SHA = "2e" * 20
TAG_OBJECT_SHA = "3d" * 20

# A stand-in for `gh api --include`: one synthetic repository whose tags,
# commits and releases live in a file, answered in the shape the real CLI
# prints (status line, headers, blank line, JSON body; exit 1 and
# "gh: message (HTTP status)" on an HTTP error). Every call is logged. `rules`
# inject the awkward cases: a rule matches a method and a part of the path,
# may change the state (`set`), and may replace the answer with an HTTP status
# or with a gh that ends without any answer (`exit`). `when: after` lets the
# request take effect first: the change was made and the answer lost.
STUB = r'''
import json, os, sys
from pathlib import Path

REPOSITORY = "repos/synthetic-owner/synthetic-repo/"
REASONS = {200: "OK", 201: "Created", 204: "No Content", 401: "Unauthorized", 403: "Forbidden",
           404: "Not Found", 422: "Unprocessable Entity", 502: "Bad Gateway"}
home = Path(os.environ["STUB_HOME"])
state_file = home / "state.json"
state = json.loads(state_file.read_text())
arguments = sys.argv[1:]
with (home / "calls.log").open("a") as log:
    log.write(json.dumps(arguments) + "\n")


def answer(status, body=None, text=None):
    out = sys.stdout.buffer
    out.write(f"HTTP/2.0 {status} {REASONS[status]}\n".encode())
    out.write(b"Content-Type: application/json; charset=utf-8\r\nX-Github-Request-Id: SYNTHETIC:0001\r\n\r\n")
    if body is not None or text is not None:
        out.write((text or json.dumps(body)).encode())
    out.flush()
    state_file.write_text(json.dumps(state))
    if status >= 400:
        sys.stderr.write(f"gh: {body['message']} (HTTP {status})\n")
        sys.exit(1)
    sys.exit(0)


def error(status, message):
    answer(status, {"message": message, "documentation_url": "https://docs.github.com/rest", "status": str(status)})


def apply(rule):
    state.update(rule.get("set", {}))
    if "exit" in rule:
        state_file.write_text(json.dumps(state))
        sys.stderr.write(rule["stderr"] + "\n")
        sys.exit(rule["exit"])
    if "status" in rule and rule["status"] < 300:
        answer(rule["status"], rule.get("body"), rule.get("text"))
    if "status" in rule:
        error(rule["status"], rule["message"])


def page(items, query):
    size, number = int(query.get("per_page", 30)), int(query.get("page", 1))
    return items[(number - 1) * size:number * size]


def ref_body(name):
    return {"ref": name, "node_id": "SYNTHETIC", "url": "https://api.github.invalid/" + name,
            "object": {**state["refs"][name], "url": "https://api.github.invalid/object"}}


def handle(method, path, query, fields):
    if not path.startswith(REPOSITORY):
        error(404, "Not Found")
    path = path[len(REPOSITORY):]
    if method == "GET" and path.startswith("git/matching-refs/"):
        prefix = "refs/" + path[len("git/matching-refs/"):]
        return 200, page([ref_body(name) for name in sorted(state["refs"]) if name.startswith(prefix)], query)
    if method == "GET" and path.startswith("git/commits/"):
        sha = path[len("git/commits/"):]
        if sha not in state["commits"]:
            error(404, "Not Found")
        return 200, {"sha": sha, "message": "synthetic commit"}
    if method == "POST" and path == "git/refs":
        if fields["ref"] in state["refs"]:
            error(422, "Reference already exists")
        if fields["sha"] not in state["commits"]:
            error(422, "Object does not exist")
        state["refs"][fields["ref"]] = {"type": "commit", "sha": fields["sha"]}
        return 201, ref_body(fields["ref"])
    if method == "DELETE" and path.startswith("git/refs/"):
        name = "refs/" + path[len("git/refs/"):]
        if name not in state["refs"]:
            error(422, "Reference does not exist")
        del state["refs"][name]
        return 204, None
    if method == "GET" and path == "releases":
        return 200, page(state["releases"], query)
    if path.startswith("releases/") and method in ("GET", "DELETE"):
        found = [release for release in state["releases"] if str(release["id"]) == path[len("releases/"):]]
        if not found:
            error(404, "Not Found")
        if method == "GET":
            return 200, found[0]
        state["releases"].remove(found[0])
        return 204, None
    sys.exit("unexpected gh call")


if arguments[:3] != ["api", "--include", "--method"] or len(arguments) < 5 or len(arguments) % 2 == 0:
    sys.exit("unexpected gh call")
method, (path, _, query) = arguments[3], arguments[4].partition("?")
if any(flag != "--raw-field" for flag in arguments[5::2]):
    sys.exit("unexpected gh call")
fields = dict(field.split("=", 1) for field in arguments[6::2])
query = dict(pair.split("=", 1) for pair in query.split("&") if pair)
rule = next((r for r in state["rules"] if r.get("method", method) == method and r["path"] in path), None)
if rule is not None:
    if "times" in rule:
        rule["times"] -= 1
        if rule["times"] == 0:
            state["rules"].remove(rule)
    if rule.get("when", "before") == "before":
        apply(rule)
result = handle(method, path, query, fields)
if rule is not None and rule.get("when") == "after":
    apply(rule)
answer(*result)
'''

BAD_CREDENTIALS = {"status": 401, "message": "Bad credentials"}
OUTAGE = {"status": 502, "message": "Server Error"}
READ_ONLY_TOKEN = {"status": 403, "message": "Resource not accessible by integration"}
NO_TOKEN = {"exit": 4, "stderr": "To get started with GitHub CLI, please run:  gh auth login\n"
            "Alternatively, populate the GH_TOKEN environment variable with a GitHub API authentication token."}
NO_NETWORK = {"exit": 1, "stderr": 'Get "https://api.github.invalid/": dial tcp: lookup api.github.invalid: no such host'}


def lightweight(sha=SHA):
    return {"type": "commit", "sha": sha}


def release(identifier, tag=TAG, draft=True):
    return {"id": identifier, "tag_name": tag, "draft": draft, "name": f"synthetic {tag}", "target_commitish": "master"}


class Rehearsal(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name)
        bin_directory = self.home / "bin"
        bin_directory.mkdir()
        (bin_directory / "gh").write_text(f"#!{sys.executable}\n{STUB}")
        (bin_directory / "gh").chmod(0o755)
        # The stub is the only program on the path and no token is passed on:
        # a test cannot reach the real CLI or GitHub.
        self.environment = {"PATH": str(bin_directory), "STUB_HOME": str(self.home), "GH_REPO": REPOSITORY}
        self.world()

    def world(self, refs=None, releases=(), commits=(SHA, OTHER_SHA), rules=()):
        state = {"refs": refs or {}, "releases": list(releases), "commits": list(commits), "rules": list(rules)}
        (self.home / "state.json").write_text(json.dumps(state))
        (self.home / "calls.log").write_text("")

    def run_script(self, *arguments, **environment):
        return subprocess.run([sys.executable, str(SCRIPT), *arguments], capture_output=True, text=True,
                              env={**self.environment, **environment})

    def create(self, **environment):
        return self.run_script("create", TAG, SHA, **environment)

    def cleanup(self, **environment):
        return self.run_script("cleanup", TAG, SHA, **environment)

    def state(self):
        state = json.loads((self.home / "state.json").read_text())
        return {"refs": state["refs"], "releases": state["releases"]}

    def raw_calls(self):
        return [json.loads(line) for line in (self.home / "calls.log").read_text().splitlines()]

    def calls(self):
        """"METHOD path", without the repository and the paging."""
        return [f"{call[3]} {call[4].partition('?')[0].removeprefix(f'repos/{REPOSITORY}/')}" for call in self.raw_calls()]

    def writes(self):
        return [call for call in self.calls() if not call.startswith("GET ")]

    def assert_refused(self, result, message, untouched):
        """Failed, said why, wrote nothing and left the repository as it was."""
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(message, result.stderr)
        self.assertEqual(self.writes(), [])
        self.assertEqual(self.state(), untouched)


class Creating(Rehearsal):
    def test_absent_tag_is_created_lightweight_at_the_expected_commit(self):
        result = self.create()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls(), [f"GET git/matching-refs/tags/{TAG}", f"GET git/commits/{SHA}",
                                        "POST git/refs", f"GET git/matching-refs/tags/{TAG}"])
        self.assertEqual(self.raw_calls()[2][5:], ["--raw-field", f"ref={REF}", "--raw-field", f"sha={SHA}"])
        self.assertEqual(self.state()["refs"], {REF: lightweight()})
        self.assertIn(f"created tag {TAG} at {SHA}", result.stdout)

    def test_rerun_accepts_the_tag_it_already_made(self):
        self.world(refs={REF: lightweight()})
        result = self.create()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("already points at", result.stdout)
        self.assertEqual(self.calls(), [f"GET git/matching-refs/tags/{TAG}"])

    def test_rerun_after_a_lost_answer_does_not_create_twice(self):
        self.world(rules=[{"method": "POST", "path": "/git/refs", "when": "after", "times": 1, **OUTAGE}])
        first = self.create()
        self.assertNotEqual(first.returncode, 0)
        self.assertIn("HTTP 502", first.stderr)
        self.assertNotIn("created", first.stdout)
        self.assertEqual(self.state()["refs"], {REF: lightweight()})
        second = self.create()
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(self.writes(), ["POST git/refs"])

    def test_tag_at_another_commit_is_a_conflict(self):
        world = {"refs": {REF: lightweight(OTHER_SHA)}, "releases": []}
        self.world(**world)
        self.assert_refused(self.create(), f"points at {OTHER_SHA}, not at {SHA}", world)

    def test_annotated_tag_is_a_conflict(self):
        world = {"refs": {REF: {"type": "tag", "sha": TAG_OBJECT_SHA}}, "releases": []}
        self.world(**world)
        self.assert_refused(self.create(), "not a lightweight tag", world)

    def test_longer_tag_with_the_same_start_is_not_taken_for_it(self):
        neighbour = {f"{REF}0": lightweight(OTHER_SHA)}
        self.world(refs=dict(neighbour))
        result = self.create()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.state()["refs"], {**neighbour, REF: lightweight()})

    def test_unknown_commit_is_refused_before_any_write(self):
        self.world(commits=[OTHER_SHA])
        self.assert_refused(self.create(), f"{SHA} is not a commit in {REPOSITORY}", {"refs": {}, "releases": []})

    def test_tag_that_appears_during_the_creation_is_not_claimed(self):
        theirs = {REF: lightweight(OTHER_SHA)}
        self.world(rules=[{"method": "POST", "path": "/git/refs", "set": {"refs": theirs}}])
        result = self.create()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("HTTP 422: Reference already exists", result.stderr)
        self.assertNotIn("created", result.stdout)
        self.assertEqual(self.state()["refs"], theirs)

    def test_token_that_may_not_write_fails_the_creation(self):
        self.world(rules=[{"method": "POST", "path": "/git/refs", **READ_ONLY_TOKEN}])
        result = self.create()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("HTTP 403", result.stderr)
        self.assertIn("Resource not accessible by integration", result.stderr)
        self.assertEqual(self.state()["refs"], {})

    def test_failed_lookup_is_never_taken_for_an_absent_tag(self):
        cases = {"rejected token": (BAD_CREDENTIALS, "HTTP 401"), "no token": (NO_TOKEN, "gh auth login"),
                 "no network": (NO_NETWORK, "no such host"), "outage": (OUTAGE, "HTTP 502"),
                 "empty answer": ({"status": 200}, "expected a list"),
                 "not a list": ({"status": 200, "body": {"message": "synthetic"}}, "expected a list"),
                 "not JSON": ({"status": 200, "text": "<html>synthetic gateway page</html>"}, "expected a list")}
        for name, (failure, message) in cases.items():
            with self.subTest(name):
                self.world(rules=[{"method": "GET", "path": "/git/matching-refs/", **failure}])
                self.assert_refused(self.create(), message, {"refs": {}, "releases": []})
                self.assertEqual(len(self.calls()), 1)

    def test_repository_the_token_cannot_see_is_not_an_absent_tag(self):
        result = self.create(GH_REPO="synthetic-owner/synthetic-private")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("HTTP 404, not found or not visible to this token", result.stderr)
        self.assertEqual(len(self.raw_calls()), 1)


class CleaningUp(Rehearsal):
    # Other people's tags and releases, which no cleanup of TAG may touch.
    NEIGHBOURS = {
        "refs": {"refs/tags/v9.8.7": lightweight(SHA), "refs/tags/rehearsal-1002": lightweight(SHA),
                 f"{REF}0": lightweight(SHA), f"refs/heads/{TAG}": lightweight(SHA)},
        "releases": [release(400, "v9.8.7", draft=False), release(502, "rehearsal-1002"), release(503, "v9.9.0")],
    }

    def rehearsed(self, tag=None, releases=(release(501),), rules=()):
        """The neighbours, and what a rehearsal leaves: its tag and its draft."""
        refs = {**self.NEIGHBOURS["refs"], REF: tag or lightweight()}
        self.world(refs=refs, releases=[*releases, *self.NEIGHBOURS["releases"]], rules=rules)
        return self.state()

    def test_draft_and_tag_are_deleted_and_nothing_else(self):
        self.rehearsed()
        result = self.cleanup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls(), [
            f"GET git/matching-refs/tags/{TAG}", "GET releases",
            "GET releases/501", "DELETE releases/501", "GET releases",
            f"GET git/matching-refs/tags/{TAG}", f"DELETE git/refs/tags/{TAG}",
            f"GET git/matching-refs/tags/{TAG}", "GET releases"])
        self.assertEqual(self.state(), self.NEIGHBOURS)
        self.assertIn(f"verified: no tag or release named {TAG} is left", result.stdout)

    def test_tag_of_a_rehearsal_that_never_made_its_draft_is_deleted(self):
        self.rehearsed(releases=[])
        result = self.cleanup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.writes(), [f"DELETE git/refs/tags/{TAG}"])
        self.assertEqual(self.state(), self.NEIGHBOURS)
        self.assertEqual(self.calls()[-1], "GET releases")

    def test_every_draft_of_the_tag_is_deleted(self):
        self.rehearsed(releases=[release(501), release(504)])
        result = self.cleanup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.writes(), ["DELETE releases/501", "DELETE releases/504", f"DELETE git/refs/tags/{TAG}"])
        self.assertEqual(self.state(), self.NEIGHBOURS)

    def test_rerun_after_a_complete_cleanup_succeeds_without_writing(self):
        self.world(**self.NEIGHBOURS)
        result = self.cleanup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("nothing to clean up", result.stdout)
        self.assertEqual(self.calls(), [f"GET git/matching-refs/tags/{TAG}", "GET releases"])
        self.assertEqual(self.state(), self.NEIGHBOURS)

    def test_rerun_finishes_a_cleanup_whose_tag_deletion_failed(self):
        self.rehearsed(rules=[{"method": "DELETE", "path": "/git/refs/", "times": 1, **OUTAGE}])
        first = self.cleanup()
        self.assertNotEqual(first.returncode, 0)
        self.assertIn("HTTP 502", first.stderr)
        self.assertNotIn("verified", first.stdout)
        self.assertEqual(self.state()["refs"][REF], lightweight())
        second = self.cleanup()
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(self.state(), self.NEIGHBOURS)

    def test_failed_draft_deletion_keeps_the_tag_that_proves_ownership(self):
        for name, failure in {"outage": OUTAGE, "token may not write": READ_ONLY_TOKEN}.items():
            with self.subTest(name):
                before = self.rehearsed(rules=[{"method": "DELETE", "path": "/releases/", "times": 1, **failure}])
                result = self.cleanup()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(f"DELETE repos/{REPOSITORY}/releases/501: HTTP {failure['status']}", result.stderr)
                self.assertEqual(self.writes(), ["DELETE releases/501"])
                self.assertEqual(self.state(), before)
                rerun = self.cleanup()
                self.assertEqual(rerun.returncode, 0, rerun.stderr)
                self.assertEqual(self.state(), self.NEIGHBOURS)

    def test_tag_deletion_lost_in_an_outage_is_noticed_by_the_rerun(self):
        self.rehearsed(rules=[{"method": "DELETE", "path": "/git/refs/", "when": "after", "times": 1, **OUTAGE}])
        self.assertNotEqual(self.cleanup().returncode, 0)
        rerun = self.cleanup()
        self.assertEqual(rerun.returncode, 0, rerun.stderr)
        self.assertIn("nothing to clean up", rerun.stdout)
        self.assertEqual(self.state(), self.NEIGHBOURS)

    def test_tag_at_another_commit_is_foreign_and_nothing_is_deleted(self):
        before = self.rehearsed(tag=lightweight(OTHER_SHA))
        self.assert_refused(self.cleanup(), f"points at {OTHER_SHA}, not at {SHA}", before)

    def test_annotated_tag_is_foreign_and_nothing_is_deleted(self):
        before = self.rehearsed(tag={"type": "tag", "sha": TAG_OBJECT_SHA})
        self.assert_refused(self.cleanup(), "not a lightweight tag", before)

    def test_public_release_and_its_tag_are_never_deleted(self):
        before = self.rehearsed(releases=[release(501, draft=False)])
        self.assert_refused(self.cleanup(), "release 501 of rehearsal-1001 is public", before)

    def test_public_release_beyond_the_first_page_still_protects_everything(self):
        newer = [release(1000 + number, f"v8.{number}.0", draft=False) for number in range(130)]
        before = self.rehearsed(releases=[release(501), *newer, release(505, draft=False)])
        self.assert_refused(self.cleanup(), "release 505 of rehearsal-1001 is public", before)
        self.assertEqual([call[4].partition("?")[2] for call in self.raw_calls() if "/releases?" in call[4]],
                         ["per_page=100&page=1", "per_page=100&page=2"])

    def test_draft_beyond_the_first_page_is_found_and_deleted(self):
        newer = [release(1000 + number, f"v8.{number}.0", draft=False) for number in range(130)]
        self.rehearsed(releases=[*newer, release(501)])
        result = self.cleanup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.writes(), ["DELETE releases/501", f"DELETE git/refs/tags/{TAG}"])
        self.assertEqual(self.state()["releases"], [*newer, *self.NEIGHBOURS["releases"]])

    def test_release_published_during_the_cleanup_is_not_deleted(self):
        before = self.rehearsed()
        published = [release(501, draft=False), *self.NEIGHBOURS["releases"]]
        self.world(refs=before["refs"], releases=before["releases"],
                   rules=[{"method": "GET", "path": "/releases/501", "set": {"releases": published}}])
        self.assert_refused(self.cleanup(), "release 501 is no longer a draft", {**before, "releases": published})

    def test_tag_moved_during_the_cleanup_is_not_deleted(self):
        before = self.rehearsed()
        moved = {**before["refs"], REF: lightweight(OTHER_SHA)}
        self.world(refs=before["refs"], releases=before["releases"],
                   rules=[{"method": "DELETE", "path": "/releases/501", "when": "after", "set": {"refs": moved}}])
        result = self.cleanup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("it changed during the cleanup, tag not deleted", result.stderr)
        self.assertEqual(self.writes(), ["DELETE releases/501"])
        self.assertEqual(self.state()["refs"], moved)

    def test_accepted_deletion_that_did_not_happen_is_an_error(self):
        # While the draft is still there, the tag is not even tried.
        cases = (("/releases/", "release 501 of rehearsal-1001 still exists", ["DELETE releases/501"]),
                 ("/git/refs/", "tag rehearsal-1001 still exists", ["DELETE releases/501", f"DELETE git/refs/tags/{TAG}"]))
        for path, message, writes in cases:
            with self.subTest(path):
                self.rehearsed(rules=[{"method": "DELETE", "path": path, "status": 204}])
                result = self.cleanup()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stderr)
                self.assertNotIn("verified", result.stdout)
                self.assertEqual(self.writes(), writes)
                self.assertEqual(self.state()["refs"][REF], lightweight())

    def test_draft_whose_tag_is_gone_is_left_for_a_person(self):
        self.world(refs=self.NEIGHBOURS["refs"], releases=[release(501), *self.NEIGHBOURS["releases"]])
        before = self.state()
        self.assert_refused(self.cleanup(), "cannot be shown to be this rehearsal's", before)

    def test_failed_lookup_is_never_taken_for_nothing_to_clean_up(self):
        failures = {"rejected token": (BAD_CREDENTIALS, "HTTP 401"), "no token": (NO_TOKEN, "gh auth login"),
                    "no network": (NO_NETWORK, "no such host"), "outage": (OUTAGE, "HTTP 502"),
                    "forbidden": (READ_ONLY_TOKEN, "HTTP 403"), "empty answer": ({"status": 200}, "expected a list")}
        for lookup in ("/git/matching-refs/", "/releases"):
            for name, (failure, message) in failures.items():
                with self.subTest(lookup=lookup, failure=name):
                    before = self.rehearsed(rules=[{"method": "GET", "path": lookup, **failure}])
                    result = self.cleanup()
                    self.assert_refused(result, message, before)
                    self.assertNotIn("nothing to clean up", result.stdout)

    def test_repository_the_token_cannot_see_is_not_nothing_to_clean_up(self):
        before = self.rehearsed()
        result = self.cleanup(GH_REPO="synthetic-owner/synthetic-private")
        self.assert_refused(result, "HTTP 404, not found or not visible to this token", before)
        self.assertNotIn("nothing to clean up", result.stdout)

    def test_malformed_descriptions_stop_the_cleanup(self):
        before = self.rehearsed()
        broken = [{"id": "501", "tag_name": TAG, "draft": True}, {"id": 501, "tag_name": TAG, "draft": "false"},
                  {"id": 501, "tag_name": TAG}, "synthetic"]
        for item in broken:
            with self.subTest(item=item):
                self.world(refs=before["refs"], releases=[item])
                result = self.cleanup()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("unexpected description of a release", result.stderr)
                self.assertEqual(self.writes(), [])


class Arguments(Rehearsal):
    def test_only_rehearsal_tags_full_commits_and_a_named_repository_are_accepted(self):
        refused = [
            ("v9.8.7", SHA, {}), ("rehearsal-", SHA, {}), ("rehearsal", SHA, {}), ("Rehearsal-1001", SHA, {}),
            (f"refs/tags/{TAG}", SHA, {}), (f"{TAG}/../../heads/master", SHA, {}), (f"{TAG}?page=2", SHA, {}),
            ("rehearsal-*", SHA, {}), (f"{TAG}..1", SHA, {}), (f"{TAG}\n", SHA, {}), (f"x-{TAG}", SHA, {}),
            (TAG, SHA[:7], {}), (TAG, SHA.upper(), {}), (TAG, SHA + "1", {}), (TAG, "master", {}), (TAG, "", {}),
            (TAG, SHA, {"GH_REPO": ""}), (TAG, SHA, {"GH_REPO": "synthetic-repo"}),
            (TAG, SHA, {"GH_REPO": "synthetic-owner/synthetic-repo/extra"}),
        ]
        self.world(refs={REF: lightweight()}, releases=[release(501)])
        before = self.state()
        for command in ("create", "cleanup"):
            for tag, sha, environment in refused:
                with self.subTest(command=command, tag=tag, sha=sha, **environment):
                    result = self.run_script(command, tag, sha, **environment)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertTrue(result.stderr.strip())
                    self.assertEqual(self.raw_calls(), [])
        self.assertEqual(self.state(), before)

    def test_tags_a_workflow_run_would_make_are_accepted(self):
        for tag in ("rehearsal-1001", "rehearsal-18273645501-2", "rehearsal-v9.8.7_rc1"):
            with self.subTest(tag):
                self.world()
                self.assertEqual(self.run_script("create", tag, SHA).returncode, 0)
                self.assertEqual(self.state()["refs"], {f"refs/tags/{tag}": lightweight()})

    def test_anything_but_the_two_commands_prints_the_usage(self):
        for arguments in ([], ["create", TAG], ["cleanup", TAG, SHA, "extra"], ["delete", TAG, SHA]):
            with self.subTest(arguments=arguments):
                result = self.run_script(*arguments)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("rehearsal.py cleanup TAG EXPECTED_SHA", result.stderr)
                self.assertEqual(self.raw_calls(), [])

    def test_missing_cli_is_an_error(self):
        empty = self.home / "empty"
        empty.mkdir()
        self.world(refs={REF: lightweight()})
        for command in ("create", "cleanup"):
            with self.subTest(command):
                result = self.run_script(command, TAG, SHA, PATH=str(empty))
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("could not run gh", result.stderr)
                self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
