#!/usr/bin/env python3
"""A rehearsal is routed away from the release credentials, and nothing else is.

The first half runs tools/ci/release-plan.py. The second half reads
.github/workflows/release.yml and evaluates its own expressions for each kind
of run, with every secret set to a recognisable value: the signing jobs of a
rehearsal must select the `release-rehearsal` environment and hand no secret
to any step, and every other run must select `release` and hand over all of
them.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
PLAN = Path(__file__).with_name("release-plan.py")
WORKFLOW = ROOT / ".github/workflows/release.yml"


# --- The subset of GitHub Actions expressions the workflow uses -------------

TOKEN = re.compile(r"\s*(?:(?P<string>'(?:[^']|'')*')|(?P<number>\d+(?:\.\d+)?)|(?P<name>[A-Za-z_][A-Za-z0-9_-]*)"
                   r"|(?P<operator>&&|\|\||==|!=|[!().,]))")


def tokens(text):
    position, found = 0, []
    text = text.rstrip()
    while position < len(text):
        match = TOKEN.match(text, position)
        if match is None:
            raise ValueError(f"cannot read expression at {text[position:]!r}")
        found.append((match.lastgroup, match.group(match.lastgroup)))
        position = match.end()
    return found


def truthy(value):
    return not (value is None or value is False or value == "" or (type(value) in (int, float) and value == 0))


def number(value):
    if value is None or value == "":
        return 0.0
    if isinstance(value, bool):
        return 1.0 if value else 0.0
    try:
        return float(value)
    except ValueError:
        return float("nan")


def equal(left, right):
    """GitHub's loose equality: strings ignore case, other mixes compare as numbers."""
    if isinstance(left, str) and isinstance(right, str):
        return left.lower() == right.lower()
    if type(left) is type(right):
        return left == right
    return number(left) == number(right)


def render(value):
    """What a step sees when the value lands in `env:` or `with:`."""
    if value is None:
        return ""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float) and value.is_integer():
        return str(int(value))
    return str(value)


class Evaluation:
    def __init__(self, text, contexts):
        self.tokens, self.position, self.contexts = tokens(text), 0, contexts

    def peek(self):
        return self.tokens[self.position] if self.position < len(self.tokens) else (None, None)

    def take(self, value=None):
        kind, text = self.peek()
        if kind is None or (value is not None and text != value):
            raise ValueError(f"expected {value or 'more'} at token {self.position}")
        self.position += 1
        return kind, text

    def result(self):
        value = self.either()
        if self.position != len(self.tokens):
            raise ValueError("unexpected trailing tokens")
        return value

    def either(self):
        value = self.both()
        while self.peek()[1] == "||":
            self.take()
            other = self.both()
            value = value if truthy(value) else other
        return value

    def both(self):
        value = self.comparison()
        while self.peek()[1] == "&&":
            self.take()
            other = self.comparison()
            value = other if truthy(value) else value
        return value

    def comparison(self):
        value = self.negation()
        while self.peek()[1] in ("==", "!="):
            operator = self.take()[1]
            other = self.negation()
            value = equal(value, other) if operator == "==" else not equal(value, other)
        return value

    def negation(self):
        if self.peek()[1] == "!":
            self.take()
            return not truthy(self.negation())
        return self.primary()

    def primary(self):
        kind, text = self.take()
        if text == "(":
            value = self.either()
            self.take(")")
            return value
        if kind == "string":
            return text[1:-1].replace("''", "'")
        if kind == "number":
            return float(text)
        if kind != "name":
            raise ValueError(f"unexpected {text!r}")
        if self.peek()[1] == "(":
            return self.call(text)
        if text in ("true", "false", "null"):
            return {"true": True, "false": False, "null": None}[text]
        value = self.contexts.get(text)
        while self.peek()[1] == ".":
            self.take()
            value = value.get(self.take()[1]) if isinstance(value, dict) else None
        return value

    def call(self, name):
        self.take("(")
        arguments = []
        while self.peek()[1] != ")":
            arguments.append(self.either())
            if self.peek()[1] == ",":
                self.take()
        self.take(")")
        if name == "format":
            return re.sub(r"\{(\d+)\}", lambda match: render(arguments[1 + int(match[1])]), arguments[0])
        if name == "startsWith":
            return render(arguments[0]).lower().startswith(render(arguments[1]).lower())
        if name == "always":
            return True
        raise ValueError(f"function {name} is not modelled by this test")


def evaluate(text, contexts):
    return Evaluation(text, contexts).result()


# --- The workflow, as text ---------------------------------------------------

EXPRESSION = re.compile(r"\$\{\{(.*?)\}\}", re.S)
SECRET_NAMES = sorted(set(re.findall(r"\bsecrets\.([A-Z0-9_]+)", WORKFLOW.read_text())))
SIGNING_JOBS = ("sign", "windows-sign")


def jobs():
    """{job name: its text}. The file indents jobs by two spaces."""
    text = WORKFLOW.read_text()
    body = text[text.index("\njobs:\n") + len("\njobs:\n"):]
    starts = list(re.finditer(r"^  ([A-Za-z0-9_-]+):[ \t]*$", body, re.M))
    return {match[1]: body[match.start():(starts[index + 1].start() if index + 1 < len(starts) else len(body))]
            for index, match in enumerate(starts)}


def without_comments(text):
    return "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("#"))


def run(event, ref, rehearsal=None, secrets=True, platforms="android-desktop"):
    """The contexts of one run. `inputs` is empty unless the run is manual."""
    inputs = {"rehearsal": rehearsal, "platforms": platforms} if rehearsal is not None else {}
    return {
        "github": {"event_name": event, "ref": ref, "ref_name": ref.rsplit("/", 1)[-1], "run_id": "1001",
                   "token": "synthetic-token", "repository": "synthetic-owner/synthetic-repo"},
        "inputs": inputs,
        "secrets": {name: f"SYNTHETIC-{name}" for name in SECRET_NAMES} if secrets else {},
    }


TAG_PUSH = dict(event="push", ref="refs/tags/v9.8.7")
MANUAL = dict(event="workflow_dispatch", ref="refs/heads/master", rehearsal=False)
REHEARSAL = dict(event="workflow_dispatch", ref="refs/heads/synthetic-branch", rehearsal=True)
# Runs GitHub cannot start today. A rehearsal must still need both conditions.
REHEARSAL_INPUT_WITHOUT_DISPATCH = dict(event="push", ref="refs/tags/v9.8.7", rehearsal=True)
OTHER_EVENT = dict(event="schedule", ref="refs/heads/master")
REAL_RUNS = {"tag push": TAG_PUSH, "manual run": MANUAL, "rehearsal input on a push": REHEARSAL_INPUT_WITHOUT_DISPATCH,
             "another event": OTHER_EVENT}


class Workflow(unittest.TestCase):
    def setUp(self):
        self.jobs = jobs()

    def environment(self, job, **scenario):
        declared = re.findall(r"^    environment:[ \t]*\n      name:[ \t]*\$\{\{(.*)\}\}[ \t]*$", self.jobs[job], re.M)
        self.assertEqual(len(declared), 1, f"{job} must name its environment with one expression")
        return render(evaluate(declared[0], run(**scenario)))

    def secret_expressions(self, job):
        found = [text for text in EXPRESSION.findall(without_comments(self.jobs[job])) if "secrets" in text]
        self.assertTrue(found, job)
        return found

    def test_the_file_is_read_as_expected(self):
        self.assertEqual(set(self.jobs), {"plan", "tests", "windows-tests", "windows-package", "windows-sign", "package",
                                          "sbom", "sign", "publish"})
        self.assertGreaterEqual(len(SECRET_NAMES), 20)
        for name in ("ANDROID_KEYSTORE_BASE64", "ANDROID_STORE_PASSWORD", "ANDROID_KEY_PASSWORD", "ANDROID_KEY_ALIAS",
                     "MACOS_CERTIFICATE_BASE64", "APPLE_APP_PASSWORD", "AZURE_CLIENT_SECRET", "IOS_PROFILE_BASE64"):
            self.assertIn(name, SECRET_NAMES)

    def test_only_the_two_signing_jobs_use_an_environment_or_secrets(self):
        for name, text in self.jobs.items():
            code = without_comments(text)
            with self.subTest(job=name):
                self.assertEqual("environment:" in code, name in SIGNING_JOBS)
                self.assertEqual("secrets" in code.replace("AZURE_* secrets", ""), name in SIGNING_JOBS)
        head = WORKFLOW.read_text().split("\njobs:\n")[0]
        self.assertNotIn("secrets.", head)

    def test_secrets_are_only_read_by_name_inside_expressions(self):
        """No `secrets: inherit`, `toJSON(secrets)` or `secrets[...]`, which this test could not follow."""
        for job in SIGNING_JOBS:
            code = without_comments(self.jobs[job]).replace("AZURE_* secrets", "")
            outside = EXPRESSION.sub("", code)
            self.assertNotIn("secrets", outside, job)
            for text in self.secret_expressions(job):
                self.assertEqual(len(re.findall(r"\bsecrets\b", text)), len(re.findall(r"\bsecrets\.[A-Z0-9_]+\b", text)), text)

    def test_a_rehearsal_signs_in_the_rehearsal_environment(self):
        for job in SIGNING_JOBS:
            self.assertEqual(self.environment(job, **REHEARSAL), "release-rehearsal", job)
            # Whatever branch or tag it is started from.
            self.assertEqual(self.environment(job, **{**REHEARSAL, "ref": "refs/heads/master"}), "release-rehearsal", job)

    def test_every_other_run_signs_in_the_protected_release_environment(self):
        for job in SIGNING_JOBS:
            for name, scenario in REAL_RUNS.items():
                with self.subTest(job=job, run=name):
                    self.assertEqual(self.environment(job, **scenario), "release")
            # A manual run on a branch is refused by the plan; were it not, the
            # environment's own protection rules still apply to it.
            self.assertEqual(self.environment(job, **{**MANUAL, "ref": "refs/heads/synthetic-branch"}), "release", job)

    def test_a_rehearsal_hands_no_secret_to_any_step(self):
        contexts = run(**REHEARSAL)
        for job in SIGNING_JOBS:
            for text in self.secret_expressions(job):
                with self.subTest(job=job, expression=text.strip()):
                    self.assertIn(render(evaluate(text, contexts)), ("", "false"))

    def test_every_other_run_receives_every_secret(self):
        for job in SIGNING_JOBS:
            for text in self.secret_expressions(job):
                name = re.search(r"\bsecrets\.([A-Z0-9_]+)", text)[1]
                for run_name, scenario in REAL_RUNS.items():
                    with self.subTest(job=job, expression=text.strip(), run=run_name):
                        self.assertIn(render(evaluate(text, run(**scenario))), (f"SYNTHETIC-{name}", "true"))
                        # Unset secrets stay empty: the signing scripts decide what that means.
                        self.assertIn(render(evaluate(text, run(**scenario, secrets=False))), ("", "false"))

    def test_presence_flags_are_true_only_with_the_secret_outside_a_rehearsal(self):
        flags = [text for job in SIGNING_JOBS for text in self.secret_expressions(job) if "!=" in text]
        # Six Azure settings and the macOS certificate decide "signed" or "unsigned preview".
        self.assertEqual(len(flags), 7)
        for text in flags:
            self.assertIs(evaluate(text, run(**TAG_PUSH)), True, text)
            self.assertIs(evaluate(text, run(**TAG_PUSH, secrets=False)), False, text)
            self.assertIs(evaluate(text, run(**REHEARSAL)), False, text)

    def android_step(self):
        steps = re.split(r"^      - ", self.jobs["sign"], flags=re.M)
        found = [step for step in steps if "tools/ci/sign-android.sh" in step]
        self.assertEqual(len(found), 1)
        return found[0]

    def test_the_android_step_is_told_whether_it_is_a_rehearsal_and_given_the_keystore_only_otherwise(self):
        step = self.android_step()
        self.assertIn("if: matrix.platform == 'android'", step)
        self.assertIn("run: bash tools/ci/sign-android.sh\n", step)
        values = dict(re.findall(r"^          ([A-Z_]+): \$\{\{(.*)\}\}$", step, re.M))
        self.assertEqual(set(values), {"REHEARSAL", "KEYSTORE", "STORE_PASSWORD", "KEY_PASSWORD", "KEY_ALIAS"})

        def seen(**scenario):
            return {name: render(evaluate(text, run(**scenario))) for name, text in values.items()}

        self.assertEqual(seen(**REHEARSAL), {"REHEARSAL": "true", "KEYSTORE": "", "STORE_PASSWORD": "", "KEY_PASSWORD": "",
                                             "KEY_ALIAS": ""})
        for name, scenario in REAL_RUNS.items():
            with self.subTest(run=name):
                self.assertEqual(seen(**scenario), {
                    "REHEARSAL": "false", "KEYSTORE": "SYNTHETIC-ANDROID_KEYSTORE_BASE64",
                    "STORE_PASSWORD": "SYNTHETIC-ANDROID_STORE_PASSWORD", "KEY_PASSWORD": "SYNTHETIC-ANDROID_KEY_PASSWORD",
                    "KEY_ALIAS": "SYNTHETIC-ANDROID_KEY_ALIAS"})
                # Without its secrets a release is not a rehearsal: sign-android.sh then fails.
                self.assertEqual(seen(**scenario, secrets=False)["REHEARSAL"], "false")

    def test_no_job_signs_android_packages_any_other_way(self):
        code = without_comments(WORKFLOW.read_text())
        self.assertEqual(code.count("tools/ci/sign-android.sh"), 1)
        self.assertNotIn("package-android.sh", code)
        for forbidden in ("IPTV_DEBUG_SIGNED_RELEASE", "debug.keystore", "keytool"):
            self.assertNotIn(forbidden, code)

    def test_signing_material_is_removed_whatever_happened(self):
        steps = re.split(r"^      - ", self.jobs["sign"], flags=re.M)
        removal = [step for step in steps if step.startswith("name: Remove signing material")]
        self.assertEqual(len(removal), 1)
        self.assertIn("if: always()", removal[0])
        for name in ("release.jks", "rehearsal-android.p12"):
            self.assertIn(f'"$RUNNER_TEMP/{name}"', removal[0])
        self.assertGreater(self.jobs["sign"].index(removal[0]), self.jobs["sign"].index(self.android_step()))

    def test_the_plan_is_given_the_same_rehearsal_decision(self):
        step = [part for part in re.split(r"^      - ", self.jobs["plan"], flags=re.M) if "release-plan.py" in part]
        self.assertEqual(len(step), 1)
        values = dict(re.findall(r"^          ([A-Z_]+): \$\{\{(.*)\}\}$", step[0], re.M))
        self.assertEqual(set(values), {"EVENT_NAME", "REF", "PLATFORMS", "REHEARSAL"})
        self.assertEqual(render(evaluate(values["REHEARSAL"], run(**REHEARSAL))), "true")
        for name, scenario in REAL_RUNS.items():
            self.assertEqual(render(evaluate(values["REHEARSAL"], run(**scenario))), "false", name)
        self.assertEqual(render(evaluate(values["PLATFORMS"], run(**TAG_PUSH))), "android-desktop")
        self.assertEqual(render(evaluate(values["PLATFORMS"], run(**{**MANUAL, "platforms": "all"}))), "all")
        # Every packaging and signing job waits for the plan's refusals.
        for job in ("package", "sign", "windows-package"):
            needs = re.search(r"^    needs: \[(.*)\]$", self.jobs[job], re.M)[1].split(", ")
            self.assertIn("plan", needs, job)
        self.assertIn("windows-package", re.search(r"^    needs: \[(.*)\]$", self.jobs["windows-sign"], re.M)[1])

    def test_only_a_tag_push_is_made_public_and_a_rehearsal_owns_a_throwaway_tag(self):
        publish = self.jobs["publish"]
        condition = re.search(r"^    if: (.*)$", publish, re.M)[1]
        tag = re.search(r"^      TAG: \$\{\{(.*)\}\}$", publish, re.M)[1]
        flag = re.search(r'if \[ "\$\{\{(.*)\}\}" = "true" \]; then\n\s+rehearsal_flag="--rehearsal"', publish)[1]
        self.assertIs(truthy(evaluate(condition, run(**TAG_PUSH))), True)
        self.assertIs(truthy(evaluate(condition, run(**REHEARSAL))), True)
        self.assertIs(truthy(evaluate(condition, run(**MANUAL))), False)
        self.assertEqual(evaluate(tag, run(**TAG_PUSH)), "v9.8.7")
        self.assertEqual(evaluate(tag, run(**REHEARSAL)), "rehearsal-1001")
        # --rehearsal is what keeps the release a draft (tools/ci/test-publish-release.py).
        self.assertEqual(render(evaluate(flag, run(**REHEARSAL))), "true")
        self.assertEqual(render(evaluate(flag, run(**TAG_PUSH))), "false")
        for step in ("rehearsal.py create", "rehearsal.py cleanup"):
            self.assertIn(f'python3 tools/ci/{step} "$TAG" "$GITHUB_SHA"', publish)
        # The publication rights stay in the one job that has no signing secret.
        self.assertNotIn("environment:", publish)
        self.assertEqual(without_comments(WORKFLOW.read_text()).count("contents: write"), 1)


class Evaluator(unittest.TestCase):
    """The model above agrees with GitHub where the workflow depends on it."""

    def test_operators_follow_github(self):
        contexts = {"github": {"event_name": "workflow_dispatch"}, "inputs": {"rehearsal": True}, "secrets": {"A": "value"}}
        cases = {
            "true && 'yes' || 'no'": "yes", "false && 'yes' || 'no'": "no", "'' || 'fallback'": "fallback",
            "null && 'x'": None, "!null": True, "!'text'": False, "'a' == 'A'": True, "1 == '1'": True,
            "null == ''": True, "secrets.A != ''": True, "secrets.MISSING != ''": False, "secrets.MISSING": None,
            "inputs.rehearsal && secrets.A": "value", "!(inputs.rehearsal) && secrets.A || ''": "",
            "!inputs.rehearsal && secrets.A != ''": False, "(github.event_name == 'push') || inputs.rehearsal": True,
            "format('rehearsal-{0}', 7)": "rehearsal-7", "startsWith('refs/tags/v1', 'refs/tags/v')": True,
            "'it''s'": "it's",
        }
        for text, expected in cases.items():
            with self.subTest(text):
                self.assertEqual(evaluate(text, contexts), expected)
                self.assertIs(type(evaluate(text, contexts)), type(expected))

    def test_what_it_does_not_model_is_an_error_not_a_guess(self):
        for text in ("toJSON(secrets)", "secrets['A']", "secrets.*", "contains(secrets.A, 'x')", "secrets.A &", "(secrets.A"):
            with self.subTest(text):
                with self.assertRaises(ValueError):
                    evaluate(text, {"secrets": {"A": "value"}})


class Plan(unittest.TestCase):
    def plan(self, event, ref, platforms="android-desktop", rehearsal="false", **environment):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "output"
            output.write_text("")
            result = subprocess.run(
                [sys.executable, str(PLAN)], capture_output=True, text=True,
                env={"PATH": os.environ["PATH"], "GITHUB_OUTPUT": str(output), "EVENT_NAME": event, "REF": ref,
                     "PLATFORMS": platforms, "REHEARSAL": rehearsal, **environment})
            written = dict(line.split("=", 1) for line in output.read_text().splitlines())
        return result, {name: [entry["platform"] for entry in json.loads(value)["include"]] for name, value in written.items()}

    def assert_refused(self, result, outputs, message):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(message, result.stderr)
        self.assertEqual(outputs, {}, "a refused run must not hand a matrix to the build jobs")

    def test_a_tag_push_packages_android_and_desktop(self):
        result, outputs = self.plan("push", "refs/tags/v9.8.7")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(outputs, {"matrix": ["android", "linux", "macos"], "signing_matrix": ["android", "macos"]})
        self.assertIn("with the release credentials", result.stdout)

    def test_a_rehearsal_packages_the_complete_default_matrix_from_any_branch(self):
        for ref in ("refs/heads/synthetic-branch", "refs/heads/master"):
            with self.subTest(ref):
                result, outputs = self.plan("workflow_dispatch", ref, rehearsal="true")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(outputs, {"matrix": ["android", "linux", "macos"], "signing_matrix": ["android", "macos"]})
                self.assertIn("no release credentials", result.stdout)

    def test_a_rehearsal_with_ios_is_refused_before_anything_is_built(self):
        result, outputs = self.plan("workflow_dispatch", "refs/heads/synthetic-branch", "all", "true")
        self.assert_refused(result, outputs, "A rehearsal cannot include iOS")
        self.assertIn("provisioning profile", result.stderr)
        self.assertIn("platforms=android-desktop", result.stderr)

    def test_a_partial_rehearsal_is_refused(self):
        for platforms in ("android", "desktop"):
            with self.subTest(platforms):
                result, outputs = self.plan("workflow_dispatch", "refs/heads/master", platforms, "true")
                self.assert_refused(result, outputs, "requires the complete Android and desktop matrix")

    def test_a_manual_run_with_the_release_credentials_only_runs_on_master(self):
        for ref in ("refs/heads/synthetic-branch", "refs/tags/v9.8.7", "refs/heads/master-2", ""):
            with self.subTest(ref):
                result, outputs = self.plan("workflow_dispatch", ref)
                self.assert_refused(result, outputs, "only runs on master")

    def test_a_manual_run_on_master_keeps_every_selection_and_ios_signing(self):
        expected = {
            "android-desktop": (["android", "linux", "macos"], ["android", "macos"]),
            "android": (["android"], ["android"]),
            "desktop": (["linux", "macos"], ["macos"]),
            "all": (["android", "linux", "macos", "ios"], ["android", "macos", "ios"]),
        }
        for platforms, (matrix, signing) in expected.items():
            with self.subTest(platforms):
                result, outputs = self.plan("workflow_dispatch", "refs/heads/master", platforms)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(outputs, {"matrix": matrix, "signing_matrix": signing})

    def test_a_push_is_never_a_rehearsal_or_a_branch(self):
        result, outputs = self.plan("push", "refs/tags/v9.8.7", rehearsal="true")
        self.assert_refused(result, outputs, "always a real Android and desktop release")
        result, outputs = self.plan("push", "refs/heads/master")
        self.assert_refused(result, outputs, "must be a v* tag")
        result, outputs = self.plan("push", "refs/tags/rehearsal-1001")
        self.assert_refused(result, outputs, "must be a v* tag")

    def test_anything_unclear_is_refused(self):
        cases = [
            (("pull_request", "refs/pull/7/merge"), {}, "does not run on"),
            (("", "refs/heads/master"), {}, "does not run on"),
            (("workflow_dispatch", "refs/heads/master", "android-desktop", "True"), {}, "REHEARSAL must be true or false"),
            (("workflow_dispatch", "refs/heads/master", "android-desktop", ""), {}, "REHEARSAL must be true or false"),
            (("workflow_dispatch", "refs/heads/master", "everything"), {}, "unknown platform selection"),
            (("workflow_dispatch", "refs/heads/master", ""), {}, "unknown platform selection"),
        ]
        for arguments, environment, message in cases:
            with self.subTest(arguments=arguments):
                result, outputs = self.plan(*arguments, **environment)
                self.assert_refused(result, outputs, message)


if __name__ == "__main__":
    unittest.main()
