#!/usr/bin/env python3
"""Synthetic structure-report checks; real analyzer probes require DESLOP_LIVE=1."""
from contextlib import redirect_stdout
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
FIXTURES = HERE / "fixtures/deslop"
spec = importlib.util.spec_from_file_location("deslop", HERE / "deslop.py")
deslop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deslop)
HEALTHY = (FIXTURES / "healthy.log").read_text()


def record(name="reference"):
    return json.loads((FIXTURES / f"{name}.json").read_text())


def diagnostic(lint, key, message):
    return {"lint": lint, "key": key, "message": message, "notes": [], "file": None, "line": None}


def cycle(members, lint="module-cycle", crate="synthetic"):
    key = ",".join(f"{crate}::{member}" for member in sorted(members))
    return diagnostic(lint, key, f"{len(members)} modules in `{crate}` use each other in a circle")


def difference(before, after):
    left, right = record(), record("head")
    left["diagnostics"], right["diagnostics"] = before, after
    return deslop.structural_diff(left, right)


def git(tree, *arguments):
    return subprocess.check_output(["git", "-C", str(tree), *arguments], text=True,
                                   stderr=subprocess.DEVNULL).strip()


def repository(path):
    path.mkdir()
    git(path, "init", "-q")
    git(path, "config", "user.email", "synthetic@example.invalid")
    git(path, "config", "user.name", "Synthetic fixture")
    (path / "tracked").write_text("initial\n")
    git(path, "add", ".")
    git(path, "commit", "-qm", "Synthetic initial tree")
    return path


def executable(path, source):
    path.write_text(f"#!{sys.executable}\n" + textwrap.dedent(source))
    path.chmod(0o755)
    return path


class Health(unittest.TestCase):
    def test_healthy_and_missing_build_script_fixtures(self):
        self.assertEqual(deslop.index_health(HEALTHY), [])
        problems = deslop.index_health((FIXTURES / "build-script-failed.log").read_text())
        self.assertEqual(len(problems), 1)
        self.assertIn("build script", problems[0])

    def test_exit_finish_empty_cache_and_duplicate_invocations(self):
        for log, expected in (
                (HEALTHY.replace("### exit: 0", "### exit: 101"), "exit is 101"),
                (HEALTHY.replace("Generating SCIP finished 1s", ""), "did not finish"),
                ("", "missing or empty"),
                ("### argv: 'unterminated\n", "malformed"),
                ("### argv: --version\n### exit: 0\n", "found 0"),
                (HEALTHY + HEALTHY, "found 2")):
            with self.subTest(expected=expected):
                self.assertIn(expected, "\n".join(deslop.index_health(log)))

    def test_real_errors_and_empty_config_error_exception(self):
        for error in ("ERROR actual failure", "2026-01-01 ERROR actual failure"):
            problems = deslop.index_health(HEALTHY.replace("Generating SCIP start...", error))
            self.assertEqual(len(problems), 1)
            self.assertIn(error, problems[0])
        # The sole ERROR line in the healthy fixture is the empty-config message.
        self.assertEqual(deslop.index_health(HEALTHY), [])
        self.assertEqual(deslop.index_health(HEALTHY.replace(deslop.BENIGN_ERROR, "")), [])

    def test_real_analyzer_timestamps_preserve_the_narrow_error_exception(self):
        self.assertEqual(deslop.index_health((FIXTURES / "timestamped-healthy.log").read_text()), [])
        for timestamp in ("2026-11-25T17:56:17.58467-05:00", "2026-11-25T22:56:17.58467Z"):
            with self.subTest(timestamp=timestamp):
                benign = timestamp + " " + deslop.BENIGN_ERROR
                self.assertEqual(deslop.index_health(HEALTHY.replace(deslop.BENIGN_ERROR, benign)), [])
                for error in (benign + " actual failure", timestamp + " ERROR actual failure"):
                    problems = deslop.index_health(HEALTHY.replace(deslop.BENIGN_ERROR, error))
                    self.assertEqual(len(problems), 1)
                    self.assertIn(error, problems[0])

    def test_markers_from_version_cannot_make_an_unhealthy_scip_healthy(self):
        log = "### argv: --version\nGenerating SCIP finished\n"
        log += "rust-analyzer: Loading build script fake run\n### exit: 0\n"
        log += "### argv: scip /synthetic\n### exit: 0\n"
        self.assertEqual(len(deslop.index_health(log)), 2)

    def test_scip_definition_errors_in_a_successful_process_invalidate_the_index(self):
        problems = deslop.index_health((FIXTURES / "scip-definition-errors.log").read_text())
        self.assertEqual(len(problems), 3)
        self.assertIn("enclosing definition with no name", problems[0])
        self.assertIn("alpha/src/lib.rs", problems[1])
        self.assertIn("beta/tests/common/mod.rs", problems[2])

    def test_wrapper_preserves_streams_arguments_stdin_and_exit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            real = executable(root / "real", '''\
                import sys
                print("stdout:" + sys.stdin.read() + ":" + sys.argv[1])
                print("stderr without newline", file=sys.stderr, end="")
                sys.exit(3)
                ''')
            wrapper = deslop.write_wrapper(root)
            log = root / "analyzer.log"
            result = subprocess.run([wrapper, "argument with spaces"], input="input", text=True,
                                    capture_output=True, env={**os.environ, "DESLOP_REAL_RA": str(real),
                                                              "DESLOP_RA_LOG": str(log)})
            self.assertEqual(result.returncode, 3)
            self.assertEqual(result.stdout, "stdout:input:argument with spaces\n")
            self.assertEqual(result.stderr, "stderr without newline")
            self.assertIn("### argv: 'argument with spaces'", log.read_text())
            self.assertIn("stderr without newline\n### exit: 3", log.read_text())


class Reports(unittest.TestCase):
    def test_each_input_difference_suppresses_the_diff(self):
        for field in deslop.INPUTS:
            with self.subTest(field=field):
                head = record("head")
                head[field] += "-different"
                if field == "nsh_rev":
                    head["nsh_head"] = head[field]
                output = deslop.comparison(record(), head, "merge", "fixture")
                self.assertIn(field + ":", output)
                self.assertIn("Inputs differ; no diff.", output)
                self.assertNotIn("| Lint |", output)

    def test_invalid_side_suppresses_diff_and_names_problem(self):
        for side in ("reference", "head"):
            left, right = record(), record("head")
            bad = left if side == "reference" else right
            bad.update(valid=False, problems=["synthetic incomplete index"])
            output = deslop.comparison(left, right, "merge", "fixture")
            self.assertIn("synthetic incomplete index", output)
            self.assertNotIn("| Lint |", output)

    def test_tool_count_duplicates_and_bad_shapes_are_rejected(self):
        item = cycle("ab")
        for value in ({"count": 2, "diagnostics": [item]},
                      {"count": 2, "diagnostics": [item, item]},
                      {"count": 1, "diagnostics": [{"lint": "tangle"}]},
                      {"count": True, "diagnostics": [item]}, []):
            with self.subTest(value=value), self.assertRaises(ValueError):
                deslop.parse_tool_output(json.dumps(value))
        self.assertEqual(deslop.parse_tool_output(json.dumps({"count": 1, "diagnostics": [item]})), [item])

    def test_reference_schema_and_validity_are_checked(self):
        for changes in ({"schema": 2}, {"valid": False}, {"dirty": "false"},
                        {"cfg": None}, {"nsh_head": "drift"}, {"seconds": {"index": 0}}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                deslop.validate_record({**record(), **changes})

    def test_cycles_shrink_and_grow_with_every_changed_member(self):
        for before, after, heading, member in (
                (cycle("abcd"), cycle("ab"), "4 → 2 modules", "− c, d"),
                (cycle("ab"), cycle("abcd"), "2 → 4 modules", "+ c, d")):
            output = difference([before], [after])
            self.assertIn(heading, output)
            self.assertIn(member, output)
            self.assertNotIn("synthetic::", output)

    def test_split_and_merge_pair_largest_overlap(self):
        output = difference([cycle("abcdef")], [cycle("abcde"), cycle("f")])
        self.assertIn("6 → 5 modules", output)
        self.assertIn("1 modules — f", output)
        self.assertIn("## New", output)
        output = difference([cycle("abcde"), cycle("f")], [cycle("abcdef")])
        self.assertIn("5 → 6 modules", output)
        self.assertIn("1 modules — f", output)
        self.assertIn("## Fixed", output)

    def test_pairing_is_deterministic_and_scoped(self):
        before = [cycle("ab"), cycle("cd"), cycle("ab", crate="other")]
        after = [cycle("ac"), cycle("bd"), cycle("ab", crate="other")]
        output = difference(before, after)
        self.assertEqual(output, difference(list(reversed(before)), list(reversed(after))))
        self.assertNotIn("other:", output)
        self.assertIn("  − b\n\n", output.replace("\n  +", "\n\n  +"))

    def test_new_fixed_unchanged_and_summary_limit(self):
        output = difference([cycle(["old"])], [cycle([f"item{i:02}" for i in range(12)])])
        self.assertIn("## New", output)
        self.assertIn("## Fixed", output)
        self.assertIn("old", output)
        self.assertIn("+2 more", output)
        self.assertNotIn("item10", output)
        self.assertIn("No structural changes.", difference([cycle("ab")], [cycle("ab")]))

    def test_tangle_members_remain_fully_qualified(self):
        output = difference([cycle("ab", lint="tangle")], [cycle("abc", lint="tangle")])
        self.assertIn("2 → 3 items", output)
        self.assertIn("+ synthetic::c", output)

    def test_fixture_other_lints_null_file_and_count_table(self):
        output = deslop.structural_diff(record(), record("head"))
        for text in ("31 places in 7 modules → read in 24 places in 6 modules",
                     "crates/synthetic/src/new.rs", "crates/synthetic/src/old.rs", "**4 → 4**"):
            self.assertIn(text, output)
        self.assertNotIn("### wide-directory", output)

    def test_size_parsing_and_string_fallback(self):
        for lint, before, after in (
                ("shallow-module", "shows 4 of its 20 functions", "shows 4 of its 21 functions"),
                ("long-file", "has 1000 lines", "has 1001 lines"),
                ("wide-directory", "holds 20 files and directories", "holds 21 files and directories"),
                ("future-lint", "unparsed old text", "unparsed new text")):
            with self.subTest(lint=lint):
                output = difference([diagnostic(lint, "synthetic", before)], [diagnostic(lint, "synthetic", after)])
                self.assertIn(f"{before} → {after}", output)
        output = difference([diagnostic("long-file", "x", "x has 1000 lines at old location")],
                            [diagnostic("long-file", "x", "x has 1000 lines at new location")])
        self.assertIn("No structural changes.", output)


class Measurement(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.tree = repository(self.root / "patina")
        self.nsh = repository(self.root / "nsh-patina")
        (self.tree / "nsh.rev").write_text(git(self.nsh, "rev-parse", "HEAD") + "\n")
        (self.tree / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "0.0.0"\n')
        git(self.tree, "add", ".")
        git(self.tree, "commit", "-qm", "Synthetic pins")

    def fake_tooling(self):
        tools = self.root / "bin"
        tools.mkdir()
        self.target = self.root / "target"
        index = self.target / "deslop-please/hash"
        index.mkdir(parents=True)
        (index / "index.stamp").write_text("stale NSH index")
        (index / "index.scip").write_text("keep index file")
        executable(tools / "cargo", f'''\
            import json, os, sys
            from pathlib import Path
            for name in {deslop.IGNORED_ENV!r}:
                assert name not in os.environ, name
            if sys.argv[1] == "metadata":
                print(json.dumps({{"target_directory": {str(self.target)!r}}}))
            else:
                assert sys.argv[1:] == ["check", "--workspace", "--all-targets", "--locked", "--target", {deslop.TARGET!r}]
                sys.exit(int(os.environ.get("SYNTHETIC_PREFLIGHT_EXIT", "0")))
            ''')
        stderr = HEALTHY.split("### argv: scip", 1)[1].split("\n", 1)[1].split("### exit:", 1)[0]
        real = executable(tools / "real-analyzer", f'''\
            import sys
            if sys.argv[1:] == ["--version"]:
                print("rust-analyzer synthetic")
            else:
                print({stderr!r}, file=sys.stderr)
            ''')
        executable(tools / "rustup", f"print({str(real)!r})\n")
        executable(tools / "rustc", 'print("synthetic_cfg")\n')
        executable(tools / "deslop-please", f'''\
            import json, os, subprocess, sys
            from pathlib import Path
            if sys.argv[1:] == ["--version"]:
                print("deslop-please synthetic")
            else:
                assert sys.argv[1:] == ["--json", "--target", {deslop.TARGET!r}]
                assert not Path({str(index / 'index.stamp')!r}).exists()
                assert Path({str(index / 'index.scip')!r}).read_text() == "keep index file"
                if not os.environ.get("SYNTHETIC_CACHE_HIT"):
                    subprocess.run(["rust-analyzer", "--version"], stdout=subprocess.DEVNULL, check=True)
                    subprocess.run(["rust-analyzer", "scip", "/synthetic"], check=True)
                if os.environ.get("SYNTHETIC_TOOL_ERROR"):
                    print("synthetic indexing failure", file=sys.stderr)
                    sys.exit(2)
                print(json.dumps({{"count": 1, "diagnostics": {record()['diagnostics'][:1]!r}}}))
                sys.exit(1)
            ''')
        return {**os.environ, "PATH": str(tools) + os.pathsep + os.environ["PATH"],
                **dict.fromkeys(deslop.IGNORED_ENV, "1")}

    def test_clean_pin_fresh_index_findings_exit_and_environment(self):
        environment = self.fake_tooling()
        with patch.dict(os.environ, environment, clear=True), redirect_stdout(io.StringIO()) as output:
            result = deslop.measure(self.tree, self.nsh)
        self.assertTrue(result["valid"], result["problems"])
        self.assertFalse(result["dirty"])
        self.assertEqual(len(result["diagnostics"]), 1)
        for name in deslop.IGNORED_ENV:
            self.assertIn(f"ignored {name}", output.getvalue())
        self.assertEqual(result["nsh_rev"], result["nsh_head"])
        self.assertEqual(result["features"], "default")
        self.assertEqual(result["target"], deslop.TARGET)
        deslop.validate_record(result)

    def test_dirty_nsh_cannot_use_the_callers_drift_override(self):
        (self.nsh / "tracked").write_text("dirty\n")
        with patch.dict(os.environ, IPTV_ALLOW_NSH_DRIFT="1"), patch.object(deslop, "target_directory") as metadata:
            result = deslop.measure(self.tree, self.nsh)
        self.assertFalse(result["valid"])
        self.assertIn("uncommitted changes", "\n".join(result["problems"]))
        metadata.assert_not_called()

    def test_different_nsh_commit_is_invalid(self):
        (self.nsh / "tracked").write_text("different\n")
        git(self.nsh, "commit", "-qam", "Synthetic different pin")
        result = deslop.measure(self.tree, self.nsh)
        self.assertFalse(result["valid"])
        self.assertIn("but nsh.rev pins", "\n".join(result["problems"]))

    def test_preflight_failure_cannot_publish_partial_index(self):
        environment = self.fake_tooling()
        environment["SYNTHETIC_PREFLIGHT_EXIT"] = "101"
        with patch.dict(os.environ, environment, clear=True), redirect_stdout(io.StringIO()):
            result = deslop.measure(self.tree, self.nsh)
        self.assertFalse(result["valid"])
        self.assertIn("exited 101", "\n".join(result["problems"]))
        self.assertEqual(result["seconds"]["index"], 0)
        self.assertTrue((self.target / "deslop-please/hash/index.stamp").exists())

    def test_missing_log_and_tool_error_are_invalid_measurements(self):
        environment = self.fake_tooling()
        for variable, problem in (("SYNTHETIC_CACHE_HIT", "log is missing or empty"),
                                  ("SYNTHETIC_TOOL_ERROR", "synthetic indexing failure")):
            with self.subTest(variable=variable), \
                    patch.dict(os.environ, {**environment, variable: "1"}, clear=True), \
                    redirect_stdout(io.StringIO()):
                result = deslop.measure(self.tree, self.nsh)
                self.assertFalse(result["valid"])
                self.assertIn(problem, "\n".join(result["problems"]))

    def test_untracked_does_not_mark_tree_dirty_but_tracked_edit_does(self):
        environment = self.fake_tooling()
        (self.tree / "untracked").write_text("synthetic")
        with patch.dict(os.environ, environment, clear=True), redirect_stdout(io.StringIO()):
            self.assertFalse(deslop.measure(self.tree, self.nsh)["dirty"])
            (self.tree / "tracked").write_text("changed")
            result = deslop.measure(self.tree, self.nsh)
            self.assertTrue(result["valid"], result["problems"])
            self.assertTrue(result["dirty"])

    def test_base_worktree_cleanup_and_target_isolation_even_on_failure(self):
        actual_run = deslop.run
        def check_base(tree, nsh, *, target_dir):
            self.assertEqual(nsh.resolve(), self.nsh.resolve())
            self.assertEqual(target_dir, self.root / "cargo-target/deslop-base")
            self.assertNotEqual(tree, self.tree)
            raise RuntimeError("synthetic interrupted measurement")
        with patch.object(deslop, "target_directory", return_value=self.root / "cargo-target"), \
                patch.object(deslop, "run", wraps=deslop.run) as commands, \
                patch.object(deslop, "measure", side_effect=check_base), \
                patch.object(deslop, "clean_environment", return_value={**os.environ, "RUSTUP_TOOLCHAIN": "stable"}):
            # Only rustup lookup is faked: worktree creation/removal use real Git.
            def dispatch(arguments, tree, environment=None):
                if arguments[:3] == ["rustup", "which", "rust-analyzer"]:
                    return "/synthetic/analyzer"
                return actual_run(arguments, tree, environment)
            commands.side_effect = dispatch
            with self.assertRaisesRegex(RuntimeError, "interrupted"):
                deslop.base_reference(self.tree, self.nsh, git(self.tree, "rev-parse", "HEAD"))
            self.assertTrue(any(call.args[0][:3] == ["git", "worktree", "prune"] for call in commands.call_args_list))
        listing = git(self.tree, "worktree", "list", "--porcelain")
        self.assertEqual(listing.count("worktree "), 1)

    def test_base_pin_mismatch_prints_preparation_and_removes_worktree(self):
        base = git(self.tree, "rev-parse", "HEAD")
        (self.tree / "nsh.rev").write_text("f" * 40 + "\n")
        git(self.tree, "commit", "-qam", "Synthetic changed pin")
        with patch.object(deslop, "target_directory", return_value=self.root / "target"), \
                patch.object(deslop, "measure") as measure, redirect_stdout(io.StringIO()) as output:
            self.assertIsNone(deslop.base_reference(self.tree, self.nsh, base))
        self.assertIn("--base cannot continue", output.getvalue())
        self.assertIn("git clone https://github.com/teng-lin/nsh.git", output.getvalue())
        self.assertIn("checkout --detach", output.getvalue())
        measure.assert_not_called()
        self.assertEqual(git(self.tree, "worktree", "list", "--porcelain").count("worktree "), 1)

    def test_missing_base_analyzer_uses_base_toolchain_and_cleans_up(self):
        actual_run = deslop.run
        def dispatch(arguments, tree, environment=None):
            if arguments[:3] == ["rustup", "which", "rust-analyzer"]:
                self.assertEqual(environment["RUSTUP_TOOLCHAIN"], "0.0.0")
                raise RuntimeError("synthetic missing component")
            return actual_run(arguments, tree, environment)
        with patch.object(deslop, "target_directory", return_value=self.root / "target"), \
                patch.object(deslop, "run", side_effect=dispatch), \
                patch.object(deslop, "measure") as measure, redirect_stdout(io.StringIO()) as output:
            self.assertIsNone(deslop.base_reference(self.tree, self.nsh, git(self.tree, "rev-parse", "HEAD")))
        self.assertIn("rustup component add rust-analyzer --toolchain 0.0.0", output.getvalue())
        measure.assert_not_called()
        self.assertEqual(git(self.tree, "worktree", "list", "--porcelain").count("worktree "), 1)


class Commands(unittest.TestCase):
    def invoke(self, arguments, *, reference=None, head=None, same_base=False, gh_error=None, runs=None):
        output = io.StringIO()
        commands = []
        def run(arguments, tree, environment=None):
            commands.append(arguments)
            if arguments[:3] == ["git", "rev-parse", "HEAD"]:
                return "a" * 40 if same_base else "b" * 40
            if arguments[:2] == ["git", "rev-parse"]:
                return "a" * 40
            if arguments[:2] == ["git", "show"]:
                return "0"
            if arguments[:2] == ["git", "merge-base"]:
                return "a" * 40
            if arguments[:3] == ["gh", "run", "list"]:
                if gh_error:
                    raise gh_error
                return json.dumps(runs or [])
            if arguments[:3] == ["gh", "run", "download"]:
                Path(arguments[-1], "report.json").write_text(json.dumps(reference or record()))
                return ""
            raise AssertionError(arguments)
        with patch.object(deslop, "run", side_effect=run), \
                patch.object(deslop, "measure", return_value=head or record("head")) as measure, \
                redirect_stdout(output):
            code = deslop.main(arguments, tree=Path("/synthetic/patina"))
        return code, output.getvalue(), commands, measure.call_count

    def test_on_master_never_indexes(self):
        code, output, commands, count = self.invoke([], same_base=True)
        self.assertEqual(code, 0)
        self.assertIn("HEAD is on master: nothing to compare", output)
        self.assertEqual(count, 0)
        self.assertFalse(any(command[0] == "gh" for command in commands))

    def test_missing_gh_no_run_and_auth_error_do_not_fallback(self):
        for error in (FileNotFoundError("gh is missing"), RuntimeError("not logged in"), None):
            with self.subTest(error=error):
                code, output, commands, count = self.invoke([], gh_error=error)
                self.assertEqual(code, 0)
                self.assertIn("--base", output)
                self.assertEqual(count, 0)
                self.assertFalse(any(command[:2] == ["git", "worktree"] for command in commands))

    def test_successful_artifact_matches_requested_merge_base(self):
        code, output, commands, count = self.invoke([], runs=[{"databaseId": 7}])
        self.assertEqual(code, 0)
        self.assertIn("deslop-report artifact, run 7", output)
        self.assertIn("## Changed", output)
        self.assertEqual(count, 1)
        listing = next(command for command in commands if command[:3] == ["gh", "run", "list"])
        self.assertIn("a" * 40, listing)
        self.assertIn("git fetch", output)

    def test_artifact_at_wrong_commit_is_rejected(self):
        other = record()
        other["commit"] = "c" * 40
        code, output, _, count = self.invoke([], reference=other, runs=[{"databaseId": 7}])
        self.assertEqual(code, 0)
        self.assertIn("not merge base", output)
        self.assertEqual(count, 0)

    def test_explicit_reference_warns_for_wrong_merge_base(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "reference.json"
            other = record()
            other["commit"] = "c" * 40
            path.write_text(json.dumps(other))
            code, output, _, count = self.invoke(["--reference", str(path)])
        self.assertEqual(code, 0)
        self.assertIn("Warning: reference commit", output)
        self.assertEqual(count, 1)

    def test_write_refuses_dirty_or_invalid_and_never_creates_report(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.json"
            for changes in ({"dirty": True}, {"valid": False, "problems": ["broken index"]}):
                with self.subTest(changes=changes):
                    head = {**record("head"), **changes}
                    code, output, _, _ = self.invoke(["--write", str(path)], head=head)
                    self.assertEqual(code, 1)
                    self.assertIn("Cannot write", output)
                    self.assertIn("HEAD seconds: preflight=1.00, index=2.00, total=3.00", output)
                    self.assertFalse(path.exists())

    def test_write_creates_parent_preserves_diagnostics_and_prints_cost(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "new/report.json"
            code, output, _, _ = self.invoke(["--write", str(path)])
            self.assertEqual(code, 0)
            self.assertEqual(json.loads(path.read_text()), record("head"))
            self.assertIn("preflight=1.00, index=2.00, total=3.00", output)
            self.assertNotIn("## Changed", output)

    def test_usage_error_is_two_and_operational_errors_are_advisory(self):
        with redirect_stdout(io.StringIO()), patch("sys.stderr", io.StringIO()):
            with self.assertRaises(SystemExit) as error:
                deslop.main(["--base", "--reference", "x"])
        self.assertEqual(error.exception.code, 2)
        with patch.object(deslop, "run", side_effect=RuntimeError("git unavailable")), redirect_stdout(io.StringIO()):
            self.assertEqual(deslop.main([]), 0)


@unittest.skipUnless(os.environ.get("DESLOP_LIVE") == "1", "requires the Linux pinned rust-analyzer")
class LiveAnalyzer(unittest.TestCase):
    def test_failed_and_healthy_build_script(self):
        environment = deslop.clean_environment()
        real = deslop.run(["rustup", "which", "rust-analyzer"], deslop.ROOT, environment)
        with tempfile.TemporaryDirectory(prefix="deslop-live-") as directory:
            root = Path(directory)
            tree, wrappers = root / "workspace", root / "wrapper"
            tree.mkdir()
            wrappers.mkdir()
            (tree / "src").mkdir()
            (tree / "Cargo.toml").write_text(
                '[package]\nname = "synthetic-deslop-live"\nversion = "0.0.0"\nedition = "2021"\n[workspace]\n')
            (tree / "src/lib.rs").write_text('include!(concat!(env!("OUT_DIR"), "/generated.rs"));\n')
            config = root / "ra-config.json"
            config.write_text('{}\n')
            deslop.write_wrapper(wrappers)
            for failing in (True, False):
                with self.subTest(failing=failing):
                    (tree / "build.rs").write_text('fn main() { panic!("synthetic build failure"); }\n' if failing else
                        'fn main() { std::fs::write(std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap())'
                        '.join("generated.rs"), "pub fn synthetic_generated() {}\\n").unwrap(); }\n')
                    log = root / f"analyzer-{failing}.log"
                    result = subprocess.run(
                        [wrappers / "rust-analyzer", "scip", tree, "--config-path", config,
                         "--output", root / f"index-{failing}.scip"], cwd=deslop.ROOT,
                        env={**environment, "DESLOP_REAL_RA": real, "DESLOP_RA_LOG": str(log)},
                        capture_output=True, text=True, timeout=180)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    problems = deslop.index_health(log.read_text())
                    if failing:
                        self.assertTrue(any("build script" in problem for problem in problems), problems)
                    else:
                        self.assertEqual(problems, [], result.stderr)


if __name__ == "__main__":
    unittest.main()
