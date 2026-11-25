#!/usr/bin/env python3
"""The tier workflows keep required checks, exact-SHA releases and native coverage."""
import importlib.util
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import textwrap
import unittest
from unittest.mock import Mock

ROOT = Path(__file__).resolve().parents[2]


def module(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), ROOT / "tools/ci" / (name + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


class Workflows(unittest.TestCase):
    def test_only_the_orchestrator_subscribes_validation_lanes_to_pr_push_and_schedule(self):
        for name in ("ci", "windows", "android", "coverage", "package-check", "windows-package-check",
                     "android-smoke", "import-stress", "fuzz", "ios", "credentials"):
            text = (ROOT / f".github/workflows/{name}.yml").read_text()
            triggers = text.split("\non:\n", 1)[1].split("\npermissions:", 1)[0]
            self.assertIn("  workflow_call:", triggers, name)
            for event in ("pull_request", "push", "schedule", "merge_group"):
                self.assertNotRegex(triggers, rf"(?m)^  {event}:", name)
        text = (ROOT / ".github/workflows/validation.yml").read_text()
        for event in ("pull_request", "push", "schedule", "merge_group"):
            self.assertRegex(text, rf"(?m)^  {event}:")
        self.assertIn("branches: [master]", text)

    def test_every_ci_helper_test_is_in_the_local_check_wrapper(self):
        wrapper = (ROOT / "tools/check.sh").read_text()
        for path in (ROOT / "tools/ci").glob("test-*.py"):
            self.assertIn(path.relative_to(ROOT).as_posix(), wrapper)

    def test_helpers_lint_workflows_with_a_checksum_pinned_actionlint(self):
        text = (ROOT / ".github/workflows/validation.yml").read_text()
        self.assertIn("actionlint: ${{ steps.select.outputs.actionlint }}", text)
        helpers = text.split("  helpers:\n", 1)[1].split("  host:\n", 1)[0]
        lint = helpers.split("      - name: Lint workflow definitions\n", 1)[1].split("      - name:", 1)[0]
        self.assertIn("if: needs.plan.outputs.actionlint == 'true'", lint)
        self.assertIn("timeout-minutes: 3", lint)
        self.assertIn("v1.7.12/actionlint_1.7.12_linux_amd64.tar.gz", lint)
        self.assertIn("8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8", lint)
        self.assertLess(lint.index("sha256sum --check"), lint.index("tar -xzf"))
        self.assertIn('"$RUNNER_TEMP/actionlint/actionlint" -shellcheck= -pyflakes=', lint)
        # Workflow lint precedes the expensive compiler/NSH helper setup.
        self.assertLess(helpers.index("Lint workflow definitions"), helpers.index("Read the compiler and NSH pins"))

    def test_standalone_lanes_do_not_emit_legacy_required_names(self):
        host = (ROOT / ".github/workflows/ci.yml").read_text()
        android = (ROOT / ".github/workflows/android.yml").read_text()
        self.assertIn("name: host-tests (${{ matrix.os }}${{ matrix.part && format(', {0}', matrix.part) || '' }})", host)
        self.assertIn("name: android-packaging", android)

    def test_standalone_lanes_pass_prerequisite_options_to_setup(self):
        for name, option in (("credentials", "policy-tool: 'false'"), ("import-stress", "prerequisites: store")):
            with self.subTest(name=name):
                text = (ROOT / f".github/workflows/{name}.yml").read_text()
                checkout, setup = text.split("      - uses: ./.github/actions/setup\n", 1)
                self.assertNotIn(option, checkout)
                self.assertIn(option, setup.split("      - name:", 1)[0])

    def test_native_validation_has_no_signing_environment(self):
        for name in ("ios", "credentials"):
            text = (ROOT / f".github/workflows/{name}.yml").read_text()
            self.assertNotIn("secrets.", text)
            self.assertNotIn("environment:", text)
            self.assertIn("timeout-minutes:", text)

    def test_non_pr_windows_runs_have_independent_concurrency_groups(self):
        text = (ROOT / ".github/workflows/windows.yml").read_text()
        self.assertIn("format('windows-run-{0}', github.run_id)", text)
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'pull_request' }}", text)

    def test_aggregate_waits_for_every_lane_and_current_protection_mirrors_it(self):
        plan = module("validation-plan")
        text = (ROOT / ".github/workflows/validation.yml").read_text()
        aggregate = text.split("  validation:\n", 1)[1].split("  protected-contexts:", 1)[0]
        needs = re.search(r"needs: \[(.*?)\]", aggregate)[1].split(", ")
        self.assertEqual(set(needs), {"plan", "helpers", "candidate", "tools", *plan.LANES})
        self.assertIn("if: always()", aggregate)
        aliases = text.split("  protected-contexts:\n", 1)[1]
        self.assertIn("needs: validation", aliases)
        self.assertIn("always() && vars.PATINA_REQUIRE_AGGREGATE_ONLY != 'true'", aliases)
        self.assertIn("github.event_name == 'pull_request'", aliases)
        self.assertIn("github.event_name == 'merge_group'", aliases)
        for context in ("test (ubuntu-24.04)", "test (macos-15)", "android"):
            self.assertIn(context, aliases)
        self.assertIn('test "$RESULT" = success', aliases)

    def test_collocated_credentials_report_success_from_linux_only(self):
        validation = (ROOT / ".github/workflows/validation.yml").read_text()
        host = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertIn("credentials: ${{ needs.plan.outputs.credentials_on_host == 'true' }}", validation)
        self.assertIn("needs.plan.outputs.credentials_on_host != 'true'", validation)
        step = host.split("        id: credentials\n", 1)[1].split("      - name:", 1)[0]
        self.assertIn("if: matrix.part == 'core' && inputs.credentials", step)
        self.assertLess(step.index("tools/ci/isolated-credentials.py"), step.index("verified=true"))
        self.assertIn("credentials-verified: ${{ steps.credentials.outputs.verified }}", host)

    def test_collocated_ios_reports_success_from_macos_only(self):
        validation = (ROOT / ".github/workflows/validation.yml").read_text()
        host = (ROOT / ".github/workflows/ci.yml").read_text()
        packages = (ROOT / ".github/workflows/package-check.yml").read_text()
        for job, workflow, platforms in (("host", host, "matrix.os == 'macos-15'"),
                                         ("packages", packages, "matrix.platform == 'macos'")):
            self.assertIn(f"ios: ${{{{ needs.plan.outputs.ios_on_host == 'true' && needs.plan.outputs.macos_host == '{job}' }}}}", validation)
            step = workflow.split("        id: ios\n", 1)[1].split("      - ", 1)[0]
            self.assertIn(f"if: {platforms} && inputs.ios", step)
            self.assertLess(step.index("bash tools/ci/check-ios.sh"), step.index("verified=true"))
            self.assertIn("ios-verified: ${{ steps.ios.outputs.verified }}", workflow)
        self.assertIn("needs.plan.outputs.ios_on_host != 'true'", validation)

    def test_mac_packaging_replaces_the_separate_host_and_debug_app_build(self):
        validation = (ROOT / ".github/workflows/validation.yml").read_text()
        host = (ROOT / ".github/workflows/ci.yml").read_text()
        packages = (ROOT / ".github/workflows/package-check.yml").read_text()
        self.assertIn("skip-macos: ${{ needs.plan.outputs.macos_host != 'host' }}", validation)
        self.assertIn("inputs.skip-macos && '{\"include\":[{\"os\":\"ubuntu-24.04\",\"part\":\"core\"},{\"os\":\"ubuntu-24.04\",\"part\":\"app\"}]}'", host)
        self.assertIn("{\"os\":\"macos-15\",\"part\":\"\"}", host)
        self.assertIn("policy-tool: ${{ matrix.part == 'core' }}", host)
        self.assertIn("macos-host-tier: ${{ needs.plan.outputs.macos_host == 'packages' &&", validation)
        step = packages.split("        id: macos-host\n", 1)[1].split("      - ", 1)[0]
        self.assertIn("if: matrix.platform == 'macos' && inputs.macos-host-tier != ''", step)
        self.assertLess(packages.index("bash tools/ci/smoke-macos-dmg.sh"), packages.index("        id: macos-host"))
        self.assertLess(step.index("--skip-app-build"), step.index("verified=true"))

    def test_release_explicitly_requests_candidate_tier_and_publishes_after_artifact_checks(self):
        text = (ROOT / ".github/workflows/release.yml").read_text()
        tests = text.split("  tests:\n", 1)[1].split("  windows-sign:\n", 1)[0]
        self.assertIn("uses: ./.github/workflows/validation.yml", tests)
        self.assertIn("tier: release", tests)
        publish = text.split("  publish:\n", 1)[1].split("    runs-on:", 1)[0]
        self.assertIn("verify-windows", publish)
        self.assertIn("tests", publish)
        self.assertIn("verify-public-release.py", text)
        verify = text.split("  verify-windows:\n", 1)[1].split("  package:\n", 1)[0]
        self.assertNotIn("environment:", verify)
        self.assertNotIn("setup-windows", verify)
        self.assertIn("name: packages-windows", verify)

    def test_event_tier_expression_never_serializes_false_as_a_tier(self):
        expressions = module('test-release-workflow')
        workflow = (ROOT / '.github/workflows/validation.yml').read_text()
        expression = re.search(r'TIER: \$\{\{(.*?)\}\}', workflow)[1]
        candidate_expression = re.search(r'CANDIDATE_BUILD: \$\{\{(.*?)\}\}', workflow)[1]
        for event, ref, tier, expected in (
                ('pull_request', 'refs/pull/17/merge', '', 'pr'),
                ('merge_group', 'refs/heads/gh-readonly-queue/master/synthetic', '', 'pr'),
                ('push', 'refs/heads/master', '', 'release'),
                ('push', 'refs/tags/v9.8.7', 'release', 'release'),
                ('schedule', 'refs/heads/master', '', 'full'),
                ('workflow_dispatch', 'refs/heads/synthetic', 'candidate', 'release'),
                ('workflow_dispatch', 'refs/heads/master', 'full', 'full'),
                ('workflow_dispatch', 'refs/heads/master', 'release', 'release')):
            context = {'github': {'event_name': event, 'ref': ref}, 'inputs': {'tier': tier}}
            with self.subTest(event=event, tier=tier):
                self.assertEqual(expressions.evaluate(expression, context), expected)
                self.assertEqual(expressions.evaluate(candidate_expression, context),
                                 (event == 'push' and ref == 'refs/heads/master' and tier != 'release')
                                 or (event == 'workflow_dispatch' and tier == 'candidate'))

    def test_delegated_helpers_retain_the_real_runner_lock_check(self):
        text = (ROOT / '.github/workflows/validation.yml').read_text()
        helpers = text.split('  helpers:\n', 1)[1].split('  host:\n', 1)[0]
        self.assertIn('python3 tools/ci/check-package-lock.py', helpers)
        host = text.split('  host:\n', 1)[1].split('  ios:\n', 1)[0]
        self.assertIn('needs: plan', host)
        self.assertIn('helpers-checked: true', host)

    def test_delegated_helpers_run_every_helper_test_the_host_skips(self):
        # tools/check.sh skips its helper tests when the helpers job ran
        # them separately (IPTV_HELPERS_CHECKED): each must be in that job, by name or
        # through its tools/ci/test-*.py loop.
        script = (ROOT / 'tools/check.sh').read_text()
        guards = script.split('if [ "${IPTV_HELPERS_CHECKED:-false}" != true ]; then\n')[1:]
        skipped = '\n'.join(block.split('\nfi\n', 1)[0] for block in guards)
        tests = re.findall(r'python3 (\S+\.py)', skipped)
        self.assertIn('python3 -m unittest discover -s tools -p test_version.py', skipped)
        self.assertIn('hosts/tizen-nsh/test_sections.py', tests)
        text = (ROOT / '.github/workflows/validation.yml').read_text()
        helpers = text.split('  helpers:\n', 1)[1].split('  host:\n', 1)[0]
        self.assertIn('for script in tools/ci/test-*.py; do python3 "$script"; done', helpers)
        self.assertIn('python3 -m unittest discover -s tools -p test_version.py', helpers)
        for test in tests:
            if re.fullmatch(r'tools/ci/test-[^/]+\.py', test):
                continue
            self.assertIn(f'python3 {test}\n', helpers, test)

    def test_reusable_lanes_can_run_without_the_optional_shared_builder(self):
        for filename, jobs in {
            "package-check.yml": ("desktop-packages",),
            "android-smoke.yml": ("build", "smoke"),
            "unsigned-candidate.yml": ("windows-package", "sbom"),
        }.items():
            workflow = (ROOT / ".github/workflows" / filename).read_text()
            for job in jobs:
                guard = workflow.split(f"\n  {job}:\n", 1)[1].splitlines()[0]
                self.assertIn("if:", guard)
                self.assertIn("!cancelled()", guard)
        smoke = (ROOT / ".github/workflows/android-smoke.yml").read_text()
        self.assertIn("!cancelled() && needs.build.result == 'success'", smoke)

    def test_master_candidate_owns_shipping_builds_and_release_uses_same_recipe(self):
        text = (ROOT / '.github/workflows/validation.yml').read_text()
        self.assertIn("&& 'validation-master'", text)
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'pull_request' }}", text)
        self.assertIn("inputs.tier != 'release'", text)
        candidate = text.split('  candidate:\n', 1)[1].split('  seal-candidate:', 1)[0]
        self.assertIn('needs: [plan, tools]', candidate)
        self.assertIn("needs.plan.outputs.candidate_build == 'true'", candidate)
        self.assertIn('needs: [plan, validation, candidate]', text)
        self.assertIn('uses: ./.github/workflows/unsigned-candidate.yml', candidate)
        self.assertIn('release-candidate.py seal', text)
        release = (ROOT / '.github/workflows/release.yml').read_text()
        self.assertIn('release-candidate.py select', release)
        self.assertIn('uses: ./.github/workflows/unsigned-candidate.yml', release)
        unsigned = (ROOT / '.github/workflows/unsigned-candidate.yml').read_text()
        self.assertNotIn('environment:', unsigned)
        self.assertNotIn('secrets.', unsigned)
        self.assertIn('check-linux-packages.sh', unsigned)
        self.assertIn('check-windows-package.ps1', unsigned)
        self.assertIn('prepare-android.sh', unsigned)
        self.assertNotIn('bundleDebug', (ROOT / '.github/workflows/android.yml').read_text())
        smoke = (ROOT / '.github/workflows/android-smoke.yml').read_text()
        self.assertIn('testDebugUnitTest lintDebug assembleDebug', smoke)
        self.assertIn('testReleaseUnitTest lintRelease', (ROOT / 'tools/ci/prepare-android.sh').read_text())

    @unittest.skipUnless(shutil.which("bash"), "candidate seal gate uses Bash")
    def test_failed_master_gate_leaves_a_retryable_seal_before_artifact_downloads(self):
        expressions = module("test-release-workflow")
        workflow = (ROOT / ".github/workflows/validation.yml").read_text()
        seal = workflow.split("  seal-candidate:\n", 1)[1]
        condition = re.search(r"if: \$\{\{(.*?)\}\}", seal)[1]
        for event, ref, tier, cancelled, expected in (
            ("push", "refs/heads/master", "", False, True),
            ("push", "refs/heads/master", "", True, False),
            ("push", "refs/heads/master", "release", False, False),
            ("push", "refs/tags/v9.8.7", "", False, False),
            ("workflow_dispatch", "refs/heads/master", "candidate", False, False),
            ("pull_request", "refs/pull/17/merge", "", False, False),
        ):
            with self.subTest(event=event, ref=ref, cancelled=cancelled):
                self.assertEqual(expressions.evaluate(condition, {
                    "github": {"event_name": event, "ref": ref}, "inputs": {"tier": tier},
                    "workflow_cancelled": cancelled,
                }), expected)

        before_downloads = seal.split("      - uses:", 1)[0]
        self.assertIn("VALIDATION_RESULT: ${{ needs.validation.result }}", before_downloads)
        self.assertIn("BUILD_RESULT: ${{ needs.candidate.result }}", before_downloads)
        check = textwrap.dedent(before_downloads.split("        run: |\n", 1)[1])
        environment = {"PLAN_RESULT": "success", "VALIDATION_RESULT": "success", "BUILD_RESULT": "success"}
        for job, result, expected in ((None, "", 0), ("PLAN_RESULT", "failure", 1),
                                      ("VALIDATION_RESULT", "cancelled", 1), ("BUILD_RESULT", "skipped", 1)):
            with self.subTest(job=job, result=result):
                env = os.environ | environment | ({job: result} if job else {})
                run = subprocess.run(["bash", "-euo", "pipefail", "-c", check], env=env,
                                     capture_output=True, text=True)
                self.assertEqual(run.returncode, expected, run.stderr)
                if expected:
                    self.assertIn("Re-run failed jobs", run.stdout)

    def test_android_release_compiles_both_abis_before_one_assembly(self):
        unsigned = (ROOT / '.github/workflows/unsigned-candidate.yml').read_text()
        native = unsigned.split('  android-native:\n', 1)[1].split('  android-package:\n', 1)[0]
        self.assertIn('abi: [arm64-v8a, armeabi-v7a]', native)
        self.assertIn('ABIS: ${{ matrix.abi }}', native)
        self.assertIn('tools/sync-host.sh --release', native)
        self.assertIn('name: android-native-${{ matrix.abi }}', native)
        android, desktop = unsigned.split('  android-package:\n', 1)[1].split('  package:\n', 1)
        self.assertIn('needs: android-native', android)
        self.assertIn('pattern: android-native-*', android)
        self.assertLess(android.index('merge-android-native.py'), android.index('prepare-android.sh'))
        self.assertEqual(unsigned.count('name: signing-input-android'), 1)
        self.assertIn('needs: dispatch-plan', desktop)
        self.assertIn('fromJSON(inputs.desktop-matrix || needs.dispatch-plan.outputs.desktop', desktop)

    def test_candidate_android_assembly_uses_the_native_matrix_result(self):
        expressions = module('test-release-workflow')
        workflow = ROOT / '.github/workflows/unsigned-candidate.yml'
        assembly = expressions.jobs(workflow)['android-package']
        condition = re.search(r'if: \$\{\{(.*?)\}\}', assembly)[1]
        self.assertIn('!cancelled()', condition)
        for result, cancelled, expected in (
                ('success', False, True), ('failure', False, False), ('skipped', False, False),
                ('cancelled', True, False), ('success', True, False)):
            with self.subTest(result=result, cancelled=cancelled):
                self.assertEqual(expressions.evaluate(condition, {
                    'needs': {'android-native': {'result': result}}, 'workflow_cancelled': cancelled,
                }), expected)

    def test_smoke_streams_are_encoded_beside_the_apk_build_and_the_guide_just_in_time(self):
        workflow = (ROOT / '.github/workflows/android-smoke.yml').read_text()
        build = workflow.split('  build:\n', 1)[1].split('\n  streams:\n', 1)[0]
        streams = workflow.split('\n  streams:\n', 1)[1].split('\n  smoke:\n', 1)[0]
        smoke = workflow.split('\n  smoke:\n', 1)[1]
        # The APK build no longer waits for the encoder.
        self.assertNotIn('make-hls-fixtures.sh', build)
        self.assertNotIn('make-iptv-fixtures.py', build)
        self.assertNotIn('needs:', streams.split('    steps:', 1)[0])
        # Streams depend only on their script and are reused while it is unchanged.
        self.assertIn("printf '%s\\n' \"$STREAMS_ARGS\"; cat tools/make-hls-fixtures.sh; } | sha256sum", streams)
        self.assertIn('${ImageVersion:-unversioned}', streams)
        self.assertIn('STREAMS_ARGS: --android-smoke', streams)
        self.assertIn('bash tools/make-hls-fixtures.sh $STREAMS_ARGS', streams)
        # The artifact is published before a bounded, non-fatal cache save.
        save = streams.index('actions/cache/save')
        self.assertLess(streams.index('name: android-smoke-streams'), save)
        self.assertIn('timeout-minutes: 5\n        continue-on-error: true', streams[save:])
        self.assertLess(streams.index('actions/cache/restore'), streams.index('bash tools/make-hls-fixtures.sh'))
        self.assertIn("if: steps.cache.outputs.cache-hit != 'true' && inputs.cold != true && github.ref == 'refs/heads/master'",
                      streams)
        self.assertNotIn('make-iptv-fixtures.py', streams)
        # The time-centred playlist and guide are made after both downloads, before serving.
        self.assertIn('needs: [build, streams]', smoke)
        guide = smoke.index('python3 -I tools/make-iptv-fixtures.py --channels 300')
        self.assertLess(smoke.index('name: android-smoke-streams'), guide)
        self.assertLess(smoke.index('name: android-smoke-input'), guide)
        self.assertLess(guide, smoke.index('tools/mock-server.py 8089 fixtures/hls'))
        self.assertIn('path: hosts/android/app/build/outputs/apk/debug\n', smoke)

    def test_large_epg_case_is_not_silently_omitted(self):
        script = (ROOT / "tools/ci/import-stress.sh").read_text()
        self.assertIn("--release", script)
        self.assertIn("--test memory", script)
        self.assertIn("--include-ignored", script)
        self.assertNotIn("catalog_stream_memory::", script)

    def test_linux_tools_are_built_once_for_consumers_and_never_in_release_validation(self):
        expressions = module('test-release-workflow')
        files = ROOT / '.github/workflows'
        validation = expressions.jobs(files / 'validation.yml')
        tools = validation['tools']
        self.assertIn("inputs.tier != 'release'", tools)
        self.assertIn('uses: ./.github/workflows/ci-tools.yml', tools)
        self.assertIn("needs.plan.outputs.linux-cli-ready != 'true'", tools)
        for lane in ('packages', 'smoke', 'candidate'):
            self.assertIn('needs: [plan, tools]', validation[lane])
            self.assertIn("prepared-linux-tools: ${{ needs.tools.outputs.cli-ready == 'true' }}", validation[lane])
            self.assertIn("!cancelled()", validation[lane])
            self.assertNotIn("needs.tools.result == 'success'", validation[lane])
        builder = (files / 'ci-tools.yml').read_text()
        self.assertIn("continue-on-error: true", builder)
        self.assertIn("uses: ./.github/actions/nsh-cli", builder)
        self.assertNotIn("uses: ./.github/actions/android", builder)
        for name in ('nsh-cli', 'android', 'cargo-tool'):
            action = (ROOT / f'.github/actions/{name}/action.yml').read_text()
            self.assertIn("github.workflow != 'Release'", action)

    def test_candidate_notices_and_sbom_build_tools_from_source(self):
        candidate = (ROOT / ".github/workflows/unsigned-candidate.yml").read_text()
        tool_steps = re.findall(r"uses: \./\.github/actions/cargo-tool\n(.*?)(?=\n      - |$)", candidate, flags=re.DOTALL)
        self.assertEqual(len(tool_steps), 4)
        for step in tool_steps:
            self.assertIn("cache: 'false'", step)

    def test_real_gtk_lifecycle_is_in_ordinary_linux_pr_validation(self):
        checks = module("check-plan")
        steps = {step["name"]: step["argv"] for step in checks.plan(tier="pr")}
        command = steps["native-gtk-lifecycle"]
        self.assertIn("tools/ci/run-xvfb.py", command)
        self.assertIn(checks.GTK_LIFECYCLE, command)
        self.assertIn("--ignored", command)
        self.assertIn("--exact", command)
        self.assertEqual(command[command.index("--features") + 1], checks.COVERAGE_FEATURES)


class FuzzCommand(unittest.TestCase):
    def test_unknown_duplicate_or_shell_target_is_rejected_before_any_build(self):
        fuzz = module("run-fuzz")
        for selection in ('["unknown"]', '["m3u", "m3u"]', '["m3u; true"]', '"m3u"', '[1]'):
            with self.subTest(selection=selection), self.assertRaises(ValueError):
                fuzz.command({"TARGETS": selection, "SECONDS_PER_TARGET": "30"})
        for seconds in ("0", "301", "bad"):
            with self.assertRaises(ValueError):
                fuzz.command({"SECONDS_PER_TARGET": seconds})
        self.assertEqual(fuzz.command({"TARGETS": '["m3u"]', "SECONDS_PER_TARGET": "30"}),
                         ["bash", "tools/ci/fuzz-imports.sh", "30", "m3u"])
        self.assertEqual(len(fuzz.command({"TARGETS": "[]"})), 12)


class PublicArtifacts(unittest.TestCase):
    def test_public_verification_rejects_modified_or_extra_assets_before_attestation(self):
        verifier = module("verify-public-release")
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            artifact = directory / "Synthetic.tar.gz"
            artifact.write_bytes(b"synthetic package bytes")
            verifier.release.write_checksums(directory)
            runner = Mock()
            verifier.verify(directory, "synthetic/repo", "a" * 40, "refs/tags/v1.0.0", runner)
            self.assertEqual(runner.call_count, 2, "both package and checksum file need provenance")
            for call in runner.call_args_list:
                self.assertIn("--source-digest", call.args[0])
                self.assertIn("a" * 40, call.args[0])
                self.assertIn("refs/tags/v1.0.0", call.args[0])
            runner.reset_mock()
            artifact.write_bytes(b"modified")
            with self.assertRaises(SystemExit):
                verifier.verify(directory, "synthetic/repo", "a" * 40, "refs/tags/v1.0.0", runner)
            runner.assert_not_called()
            verifier.release.write_checksums(directory)
            (directory / "extra.txt").write_text("unlisted")
            with self.assertRaises(SystemExit):
                verifier.verify(directory, "synthetic/repo", "a" * 40, "refs/tags/v1.0.0", runner)


if __name__ == "__main__":
    unittest.main()
