#!/usr/bin/env python3
"""PR selection and aggregate failures use the same explicit validation plan."""
import importlib.util
from pathlib import Path
import re
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("validation", Path(__file__).with_name("validation-plan.py"))
v = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(v)


# The test is deliberately conservative about computed include paths: new forms
# need an explicit scanner case before they can silently embed documentation.
RUST_STRING = re.compile(r'r(?P<hash>\#{0,255})"(?P<raw>.*?)"(?P=hash)|"(?:\\.|[^"\\])*"', re.S)
RUST_CHAR = re.compile(r"'(?:\\(?:[nrt0\\'\"]|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\})|[^'\\\n])'")


def rust_without_comments(text):
    """Erase Rust trivia while retaining literals and offsets (nested blocks too)."""
    chunks, index = [], 0
    while index < len(text):
        literal = RUST_STRING.match(text, index) or RUST_CHAR.match(text, index)
        if literal:
            chunks.append(literal[0])
            index = literal.end()
        elif text.startswith("//", index):
            end = text.find("\n", index)
            end = len(text) if end < 0 else end
            chunks.append(" " * (end - index))
            index = end
        elif text.startswith("/*", index):
            start, depth = index, 1
            index += 2
            while depth and index < len(text):
                if text.startswith("/*", index):
                    depth += 1
                    index += 2
                elif text.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
            if depth:
                raise AssertionError("unterminated Rust block comment")
            chunks.append(re.sub(r"[^\n]", " ", text[start:index]))
        else:
            chunks.append(text[index])
            index += 1
    return "".join(chunks)


def markdown_includes(source):
    text = rust_without_comments(source.read_text())
    for macro in re.finditer(r"\binclude_(?:str|bytes)\s*!\s*([({\[])", text):
        start, index = macro.end(), macro.end()
        closing = {"(": ")", "[": "]", "{": "}"}
        stack = [closing[macro[1]]]
        while stack and index < len(text):
            literal = RUST_STRING.match(text, index)
            if literal:
                index = literal.end()
                continue
            char = text[index]
            if char in closing:
                stack.append(closing[char])
            elif char in closing.values():
                if char != stack.pop():
                    raise AssertionError(f"{source}: mismatched include delimiter")
            index += 1
        if stack:
            raise AssertionError(f"{source}: unterminated include macro")
        expression = text[start:index - 1].strip().rstrip(",").strip()
        literal = RUST_STRING.fullmatch(expression)
        if literal:
            value = literal["raw"] if literal["raw"] is not None else expression[1:-1]
            if "\\" in value:
                raise AssertionError(f"{source}: escaped include path needs scanner support")
            if value.lower().endswith(".md"):
                yield (source.parent / value).resolve()
        elif re.search(r',\s*"[^"\\]*\.[A-Za-z0-9]+"\s*\)\s*$', expression) and not re.search(r"\.md\b", expression, re.I):
            # A computed prefix with a known non-Markdown suffix (e.g. OUT_DIR
            # simulator.entitlements) cannot embed a Markdown file.
            continue
        else:
            raise AssertionError(f"{source}: computed include path needs scanner support: {expression}")


class EmbeddedMarkdownScanner(unittest.TestCase):
    def test_literal_raw_and_deleted_markdown_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.rs"
            source.write_text('include_str ! ("deleted.md"); include_bytes! {r#"bytes.MD"#}; include_str!["square.md"]; include_str!("other.sql");')
            self.assertEqual(list(markdown_includes(source)), [(Path(directory) / name).resolve()
                                                              for name in ("deleted.md", "bytes.MD", "square.md")])

    def test_comments_are_rust_trivia_and_commented_macros_are_ignored(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.rs"
            source.write_text("let quote = '\"'; "
                              'include_str /* nested /* comment */ end */ ! ("help.md"); '
                              'include_bytes! /* comment */ ["bytes.md"]; '
                              'include_str! // comment\n {"line.md"}; '
                              '// include_str!("ignored.md");\n'
                              '/* include_str!("also-ignored.md"); */')
            self.assertEqual([path.name for path in markdown_includes(source)], ["help.md", "bytes.md", "line.md"])

    def test_unknown_computed_paths_fail_instead_of_hiding_consumers(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.rs"
            for expression in ('include_str!(DOC)', 'include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/help.md"))'):
                source.write_text(expression)
                with self.assertRaisesRegex(AssertionError, "computed include path needs scanner support"):
                    list(markdown_includes(source))


class Selection(unittest.TestCase):
    def test_docs_do_not_start_builds(self):
        plan = v.select(["README.md", "docs/architecture.md", "AGENTS.md"])
        self.assertFalse(any(plan[lane] for lane in v.LANES))

    def test_consumed_docs_select_their_actual_consumers(self):
        self.assertTrue(v.select(["docs/capabilities.md"])["host"])
        self.assertTrue(v.select(["docs/contributing.md"])["helpers_full"])
        self.assertFalse(v.select(["docs/contributing.md"])["host"])
        self.assertFalse(v.select(["docs/release.md"])["helpers_full"])

    def test_changed_lint_wrappers_execute_as_well_as_receive_syntax_checks(self):
        for path in ("tools/ci/lint.sh", "tools/ci/check-line-length.sh"):
            self.assertTrue(v.select([path])["host"], path)
            self.assertTrue(v.select([path])["helpers_full"], path)

    def test_platform_and_production_key_inputs_have_required_lanes(self):
        for path in ("crates/iptv-app/src/host/native/credential_key.rs", "crates/iptv-app/src/host/native/credential_key/linux.rs",
                     "tools/ci/isolated-credentials.py", "crates/iptv-store-sqlite/src/lib.rs"):
            self.assertTrue(v.select([path])["credentials"], path)
        for path in ("tools/fetch-mpvkit.sh", "tools/mpv-runtime-ios/runtime-ios.json",
                     "crates/iptv-player-mpv/src/mpv/ios.rs", "tools/ci/check-ios.sh",
                     "crates/iptv-app/nsh.json", "tools/check-ios-simulator-entitlements.sh",
                     "crates/iptv-app/src/host/native/apple_airplay.m", "tools/apple-airplay-probe/view.rs"):
            self.assertTrue(v.select([path])["ios"], path)
        self.assertFalse(v.select(["docs/architecture.md"])["ios"])

    def test_markdown_and_repository_metadata_use_light_checks(self):
        docs = ("hosts/tizen/README.md", "hosts/tizen-nsh/README.md", "crates/iptv-tui/README.md",
                "art/icon/README.md", "tools/README.md", "hosts/android/README.md", "crates/iptv-core/README.md",
                "docs/privacy-policy/index.html", "docs/screenshots/synthetic.png", "nested/NOTES.MD")
        metadata = (".github/pull_request_template.md", ".github/ISSUE_TEMPLATE/bug.yml",
                    ".github/ISSUE_TEMPLATE/config.yml", ".github/CODEOWNERS", ".github/dependabot.yml")
        for path in (*docs, *metadata):
            with self.subTest(path=path):
                plan = v.select([path])
                self.assertFalse(any(plan[lane] for lane in v.LANES), plan)
                self.assertEqual(plan["macos_host"], "skip")
                self.assertEqual(plan["helpers_full"], path in metadata)
                self.assertFalse(plan["actionlint"])

    def test_packaged_markdown_and_unknown_inputs_keep_consumers(self):
        for path in ("tools/mpv-runtime/README.md", "tools/mpv-runtime-unix/build/README.md",
                     "crates/iptv-app/fonts/README.md", "LICENSE", "about.toml"):
            with self.subTest(path=path):
                plan = v.select([path])
                self.assertTrue(plan["packages"])
                self.assertEqual(plan["windows-package"], not path.startswith("tools/mpv-runtime-unix/"))
                self.assertTrue(plan["android"])
        self.assertTrue(v.select(["tools/mpv-runtime-ios/README.md"])["ios"])
        for path in ("vendor/nsh-tv/README.md", "vendor/synthetic/README.md", ".gitattributes", "unknown.input"):
            self.assertTrue(v.select([path])["host"], path)
        # The explicit embedded-doc exception survives removal of the asset.
        self.assertTrue(v.select(["docs/capabilities.md"])["host"])

    def test_unrelated_workflows_select_helpers_and_actionlint_only(self):
        for name in ("prune-caches", "privacy-policy", "nsh-cli-releases", "release", "promote-native",
                     "mpv-runtime", "dependency-candidate", "new-standalone"):
            path = f".github/workflows/{name}.yml"
            with self.subTest(path=path):
                plan = v.select([path])
                self.assertFalse(any(plan[lane] for lane in v.LANES), plan)
                self.assertTrue(plan["helpers_full"])
                self.assertTrue(plan["actionlint"])
        # Light inputs cannot suppress selection from unrelated production edits.
        mixed = v.select([".github/workflows/prune-caches.yml", "crates/iptv-core/src/guide.rs"])
        self.assertTrue(mixed["host"])
        self.assertTrue(mixed["fuzz"])
        self.assertTrue(mixed["stress"])

    def test_workflow_graph_is_transitive_and_unknown_forms_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflows = root / ".github/workflows"
            workflows.mkdir(parents=True)
            (root / "fuzz").mkdir()
            (root / "fuzz/Cargo.toml").write_text((v.ROOT / "fuzz/Cargo.toml").read_text())
            (workflows / "validation.yml").write_text("jobs:\n  first:\n    uses: './.github/workflows/shared.yml' # comment\n")
            (workflows / "shared.yml").write_text('jobs:\n  second:\n    uses: "./.github/workflows/child.yaml"\n')
            (workflows / "child.yaml").write_text("jobs:\n  cycle:\n    uses: ./.github/workflows/shared.yml\n")
            self.assertEqual(v.validation_workflows(root), {f".github/workflows/{name}" for name in
                                                           ("validation.yml", "shared.yml", "child.yaml")})
            self.assertTrue(all(v.select([".github/workflows/child.yaml"], root=root)[lane] for lane in v.LANES))
            # Even a formerly classified lane becomes light when it is no longer called.
            self.assertFalse(any(v.select([".github/workflows/ci.yml"], root=root)[lane] for lane in v.LANES))
            for text in ("jobs:\n  x:\n    uses: ./.github/workflows/missing.yml\n",
                         'jobs: {x: {uses: "./.github/workflows/shared.yml"}}\n',
                         "jobs:\n  x:\n    uses: ${{ inputs.workflow }}\n",
                         'jobs:\n  x:\n    uses: "./.github\\u002fworkflows/ci.yml"\n',
                         "jobs:\n  x:\n    uses: *workflow-alias\n",
                         'jobs:\n  x:\n    "uses": "./.github\\u002fworkflows/ci.yml"\n',
                         'jobs:\n  x:\n    "us\\u0065s": "./.github\\u002fworkflows/ci.yml"\n',
                         'jobs: {x: {"us\\u0065s": "./.github\\u002fworkflows/ci.yml"}}\n',
                         '{jobs: {x: {"us\\u0065s": "./.github\\u002fworkflows/ci.yml"}}}\n',
                         'jobs:\n  x:\n    <<: {"us\\u0065s": "./.github\\u002fworkflows/ci.yml"}\n',
                         'jobs:\n  x: {"us\\u0065s": "./.github\\u002fworkflows/ci.yml"}\n',
                         'jobs:\n  x:\n    ?\n      uses\n    :\n      "./.github/\\\n      workflows/ci.yml"\n',
                         'jobs:\n  x:\n    {"us\\u0065s": "./.github\\u002fworkflows/ci.yml"}\n',
                         'jobs:\n   x: {"us\\u0065s": "./.github\\u002fworkflows/ci.yml"}\n',
                         'jobs:\n    x: {"us\\u0065s": "./.github\\u002fworkflows/ci.yml"}\n',
                         'env: &a {"us\\u0065s": "./.github\\u002fworkflows/ci.yml"}\njobs:\n  j:\n    *a\n',
                         'jobs:\n  x:\n      uses: "./.github\\u002fworkflows/ci.yml"\n'):
                (workflows / "validation.yml").write_text(text)
                self.assertIsNone(v.validation_workflows(root))
                self.assertTrue(all(v.select([".github/workflows/unrelated.yml"], root=root)[lane] for lane in v.LANES))

    def test_workflow_lint_is_selected_for_actions_and_full_tiers(self):
        self.assertTrue(v.select([".github/actions/setup/action.yml"])["actionlint"])
        self.assertTrue(v.select([], "full")["actionlint"])
        self.assertTrue(v.select([], "release")["actionlint"])
        self.assertFalse(v.select(["tools/ci/test-validation-plan.py"])["actionlint"])

    def assert_embedded_markdown_classified(self, root):
        found = set()
        for tree in ("crates", "hosts", "fuzz", "vendor"):
            for source in (root / tree).rglob("*.rs"):
                for included in markdown_includes(source):
                    path = included.relative_to(root).as_posix()
                    found.add(path)
                    self.assertFalse(v.documentation(path), f"{source}: embedded {path} needs a planner exception")
                    self.assertTrue(v.select([path])["host"], path)
        return found

    def test_rust_embedded_markdown_never_routes_as_documentation(self):
        self.assertIn("docs/capabilities.md", self.assert_embedded_markdown_classified(v.ROOT))

    def test_fuzz_and_vendor_markdown_consumers_cannot_bypass_the_guard(self):
        for tree in ("fuzz", "vendor"):
            with self.subTest(tree=tree), tempfile.TemporaryDirectory() as directory:
                root = Path(directory).resolve()
                source = root / tree / "src/synthetic.rs"
                source.parent.mkdir(parents=True)
                source.write_text('include_str!("../../docs/synthetic.md");')
                with self.assertRaisesRegex(AssertionError, "embedded docs/synthetic.md needs a planner exception"):
                    self.assert_embedded_markdown_classified(root)

    def test_packaging_helper_dependency_closure(self):
        for path in ("tools/ci/notices.hbs", "tools/ci/component-inventory.py", "tools/ci/crypto-native-notices.py",
                     "tools/ci/check-package-lock.py", "crates/iptv-cast/NOTICE-pyatv.md"):
            plan = v.select([path])
            self.assertTrue(plan["packages"], path)
            self.assertFalse(plan["android_package"], path)
            self.assertTrue(plan["windows-package"], path)
        plan = v.select(["tools/ci/verify-macos-dmg.py"])
        self.assertEqual([entry["platform"] for entry in plan["package_matrix"]["include"]], ["macos"])

    def test_pure_core_keeps_correctness_production_and_android_compile(self):
        plan = v.select(["crates/iptv-core/src/guide.rs"])
        for lane in ("host", "windows", "android"):
            self.assertTrue(plan[lane], lane)
        for lane in ("android_package", "packages", "windows-package", "smoke"):
            self.assertFalse(plan[lane], lane)
        # Rust source changes measure coverage so a floor drop fails the PR, not master.
        self.assertTrue(plan["coverage"])
        self.assertTrue(plan["fuzz"])
        self.assertTrue(plan["stress"])

    def test_macos_host_uses_packaging_only_when_both_checks_are_selected(self):
        cases = (
            (["Cargo.lock"], "pr", "host"),
            (["hosts/macos/Synthetic.swift"], "pr", "packages"),
            (["crates/iptv-core/src/guide.rs"], "pr", "host"),
            (["hosts/android/app/build.gradle.kts"], "pr", "skip"),
            (["hosts/ios/Synthetic.swift"], "pr", "skip"),
            (["tools/ci/smoke-macos-dmg.sh"], "pr", "skip"),
            ([], "full", "packages"),
            ([], "release", "host"),
        )
        for paths, tier, job in cases:
            with self.subTest(paths=paths, tier=tier):
                self.assertEqual(v.select(paths, tier)["macos_host"], job)
        self.assertFalse(v.select(["hosts/ios/Synthetic.swift"])["ios_on_host"])
        self.assertTrue(v.select(["Cargo.lock"])["ios_on_host"])

    def test_rust_sources_and_tests_select_coverage_but_docs_and_manifests_do_not(self):
        for path in ("crates/iptv-app/src/recordings_view.rs", "crates/iptv-app/tests/app/recordings.rs",
                     "crates/iptv-store-sqlite/tests/it/migrations.rs", "crates/iptv-web/src/lib.rs"):
            self.assertTrue(v.select([path])["coverage"], path)
        for path in ("README.md", "docs/ci.md", "crates/iptv-presentation/src/i18n/messages.tsv",
                     "hosts/android/app/build.gradle.kts", "Cargo.lock"):
            self.assertFalse(v.select([path])["coverage"], path)
        # Drafts still defer it until the pull request is marked ready.
        self.assertFalse(v.select(["crates/iptv-app/src/recordings_view.rs"], draft=True)["coverage"])

    def test_ci_helper_change_does_not_build_installers(self):
        plan = v.select(["tools/ci/test-sbom.py"])
        self.assertFalse(plan["host"])
        self.assertTrue(plan["helpers_full"])
        for lane in ("packages", "windows-package", "android_package", "coverage", "smoke"):
            self.assertFalse(plan[lane], lane)

    def test_graph_changes_validate_every_lane(self):
        for path in (".github/workflows/validation.yml", ".github/actions/setup/action.yml", "tools/ci/apt-install.sh",
                     "tools/ci/check-plan.py",
                     "tools/ci/native-candidates.json", "tools/ci/native_candidates.py", "tools/ci/run-xvfb.py"):
            plan = v.select([path])
            self.assertTrue(all(plan[lane] for lane in v.LANES), path)

    def test_dependency_changes_keep_native_builds_without_desktop_installers(self):
        for path in ("Cargo.lock", "Cargo.toml", "crates/iptv-guide/Cargo.toml", "rust-toolchain.toml",
                     ".cargo/config.toml", "vendor/nsh-tv/src/lib.rs"):
            plan = v.select([path])
            for lane in ("host", "windows", "android", "smoke", "stress"):
                self.assertTrue(plan[lane], (path, lane))
            self.assertEqual(plan["fuzz"], v.fuzz_targets([], True))
            for lane in ("packages", "windows-package", "coverage"):
                self.assertFalse(plan[lane], (path, lane))
            self.assertEqual(plan["package_matrix"]["include"], [], path)

    def test_synchronized_dependency_locks_do_not_select_desktop_installers(self):
        plan = v.select(["Cargo.lock", "tools/ci/package.Cargo.lock", "fuzz/Cargo.lock", "hosts/tizen-nsh/Cargo.lock"])
        for lane in ("host", "windows", "android", "helpers_full"):
            self.assertTrue(plan[lane], lane)
        self.assertFalse(plan["packages"])
        self.assertFalse(plan["windows-package"])
        package_lock_only = v.select(["tools/ci/package.Cargo.lock"])
        self.assertTrue(package_lock_only["android"])
        self.assertTrue(package_lock_only["smoke"])
        self.assertTrue(package_lock_only["helpers_full"])

    def test_native_and_ui_changes_keep_both_form_factors(self):
        for path in ("crates/iptv-app/src/playback/video.rs", "crates/iptv-runtime/src/lib.rs", "hosts/android/app/src/main/Synthetic.kt",
                     "crates/iptv-player-mpv/src/player.rs", "crates/iptv-nav/src/lib.rs"):
            self.assertTrue(v.select([path])["smoke"], path)

    def test_presentation_changes_run_android_smoke_but_domain_core_does_not(self):
        for path in ("crates/iptv-presentation/src/guide.rs",
                     "crates/iptv-presentation/src/i18n.rs",
                     "crates/iptv-presentation/src/i18n/messages.tsv"):
            self.assertTrue(v.select([path])["smoke"], path)
        self.assertFalse(v.select(["crates/iptv-core/src/recording.rs"])["smoke"])

    def test_app_harness_changes_need_host_tests_but_no_android_or_ios_build(self):
        paths = ["crates/iptv-app/tests/app/play_bar.rs", "crates/iptv-app/tests/app/fixtures/playlist.m3u"]
        plan = v.select(paths)
        self.assertTrue(plan["host"])
        self.assertTrue(plan["windows"])
        self.assertFalse(any(plan[lane] for lane in ("android", "ios", "smoke", "packages", "windows-package")))
        self.assertTrue(v.select([*paths, "crates/iptv-app/src/playback/play_bar.rs"])["smoke"])

    def test_leaf_workflow_edits_exercise_their_lane(self):
        for path, selected in v.LANE_WORKFLOWS.items():
            with self.subTest(path=path):
                plan = v.select([path])
                self.assertEqual([lane for lane in v.LANES if plan[lane]], [selected])
                self.assertTrue(plan["helpers_full"])

    def test_storage_and_epg_changes_keep_large_regressions(self):
        for path in ("crates/iptv-store-sqlite/src/lib.rs", "crates/iptv-import/src/lib.rs", "crates/iptv-epg/src/lib.rs",
                     "crates/iptv-providers/src/xtream/parse.rs", "crates/iptv-guide/src/combined.rs",
                     "crates/iptv-core/src/lib.rs", "crates/iptv-library/src/lib.rs",
                     "crates/iptv-settings/src/lib.rs", "crates/iptv-sqlcipher-init/src/lib.rs"):
            self.assertTrue(v.select([path])["stress"], path)

    def test_execution_wrappers_exercise_their_lanes(self):
        self.assertTrue(v.select(["tools/ci/run-xvfb.py"])["host"])
        self.assertEqual(v.select(["tools/ci/run-fuzz.py"])["fuzz"], v.fuzz_targets([], True))

    def test_linux_smoke_images_select_only_linux_packaging(self):
        images = list((v.ROOT / "tools/ci").glob("Dockerfile.*-smoke"))
        self.assertTrue(images)
        for image in images:
            path = image.relative_to(v.ROOT).as_posix()
            self.assertEqual([entry["platform"] for entry in v.select([path])["package_matrix"]["include"]], ["linux"], path)

    def test_parser_changes_select_related_targets(self):
        self.assertEqual(v.fuzz_targets(["crates/iptv-providers/src/m3u/parser.rs"]), ["catchup", "gzip", "m3u"])
        self.assertEqual(v.fuzz_targets(["crates/iptv-providers/src/xtream.rs"]), ["xtream"])
        self.assertEqual(v.fuzz_targets(["crates/iptv-net/src/playback/hls.rs"]), ["dash", "hls"])
        self.assertEqual(v.fuzz_targets(["fuzz/fuzz_targets/xmltv.rs"]), ["xmltv"])
        self.assertEqual(v.fuzz_targets(["fuzz/shared.rs"]), v.fuzz_targets([], True))

    def test_all_full_tiers_explicitly_select_every_lane(self):
        for tier in ("full", "release"):
            plan = v.select([], tier)
            delegated = ("android", "packages", "windows-package") if tier == "release" else ()
            self.assertTrue(all(plan[lane] for lane in v.LANES if lane not in delegated))
            self.assertFalse(plan["android_package"])
            for lane in delegated:
                self.assertFalse(plan[lane])
            self.assertEqual(plan["tier"], tier)
        with self.assertRaises(ValueError):
            v.select([], "typo")

    def test_platform_packaging_scripts_are_selected(self):
        for path in ("tools/ci/native-sources.py", "tools/ci/package-linux.sh", "tools/package-macos.sh"):
            self.assertTrue(v.select([path])["packages"], path)
        self.assertTrue(v.select(["tools/package-windows.ps1"])["windows-package"])
        self.assertTrue(v.select(["hosts/android/app/build.gradle.kts"])["android"])

    def test_nsh_packager_revision_selects_its_desktop_consumers(self):
        plan = v.select(["nsh.rev"])
        self.assertEqual([entry["platform"] for entry in plan["package_matrix"]["include"]], ["linux", "macos"])
        self.assertFalse(plan["windows-package"])
        self.assertTrue(plan["windows"])

    def test_mobile_packaging_scripts_do_not_select_desktop_installers(self):
        for path in ("tools/ci/package-android.sh", "tools/ci/package-ios.sh"):
            plan = v.select([path])
            self.assertFalse(plan["packages"], path)
            self.assertFalse(plan["windows-package"], path)

    def test_version_stamping_changes_exercise_all_package_consumers(self):
        plan = v.select(["tools/ci/version.py"])
        self.assertTrue(plan["windows-package"])
        self.assertTrue(plan["packages"])
        self.assertEqual([entry["platform"] for entry in plan["package_matrix"]["include"]], ["linux", "macos"])

    def test_windows_resource_changes_exercise_compilation_and_installer(self):
        for path in ("crates/iptv-app/windows.rc", "crates/iptv-app/icon.ico"):
            with self.subTest(path=path):
                plan = v.select([path])
                self.assertTrue(plan["windows"])
                self.assertTrue(plan["windows-package"])

    def test_installer_definition_and_compiler_setup_exercise_windows_packaging(self):
        for path in ("tools/ci/windows-installer.iss", "tools/ci/setup-inno.ps1"):
            with self.subTest(path=path):
                plan = v.select([path])
                self.assertTrue(plan["windows-package"])
                self.assertFalse(plan["packages"])
                self.assertFalse(plan["android_package"])
    def test_platform_specific_package_changes_do_not_build_the_other_installer(self):
        for path, platform in (("tools/ci/package-linux.sh", "linux"), ("tools/package-macos.sh", "macos")):
            plan = v.select(["Cargo.lock", path])
            self.assertEqual([entry["platform"] for entry in plan["package_matrix"]["include"]], [platform])
            self.assertFalse(plan["windows-package"], path)
        plan = v.select(["Cargo.lock", "tools/package-windows.ps1"])
        self.assertTrue(plan["windows-package"])
        self.assertFalse(plan["packages"])



class Aggregate(unittest.TestCase):
    def setUp(self):
        self.plan = v.select(["crates/iptv-core/src/guide.rs"])
        self.needs = self.successful_needs(self.plan)

    @staticmethod
    def successful_needs(plan):
        needs = {lane: {"result": "success" if plan[lane] else "skipped"} for lane in v.LANES}
        needs.update(plan={"result": "success"}, helpers={"result": "success"},
                     candidate={"result": "success" if plan["candidate_build"] else "skipped"})
        macos_host = plan["macos_host"]
        if macos_host != "skip":
            needs[macos_host].setdefault("outputs", {})["macos-host-verified"] = "true"
        for lane, evidence in v.HOST_VERIFICATIONS.items():
            if plan[f"{lane}_on_host"]:
                needs[lane]["result"] = "skipped"
                job = macos_host if lane == "ios" else "host"
                needs[job].setdefault("outputs", {})[evidence] = "true"
        return needs

    def test_shipping_candidate_builds_are_part_of_the_same_aggregate(self):
        self.plan = v.select([], 'release', candidate_build=True)
        self.needs = self.successful_needs(self.plan)
        self.assertEqual(v.gate(self.plan, self.needs), [])
        for result in ('failure', 'skipped', 'cancelled', None):
            self.needs['candidate']['result'] = result
            self.assertTrue(v.gate(self.plan, self.needs), result)
        for tier in ('pr', 'full'):
            with self.assertRaises(ValueError):
                v.select([], tier, candidate_build=True)

    def test_successful_selection_and_deliberate_skips_pass(self):
        self.assertEqual(v.gate(self.plan, self.needs), [])

    def test_a_draft_defers_heavy_lanes_and_never_passes_the_gate(self):
        # A shared CI input selects every lane, heavy ones included.
        ready = v.select([".github/actions/setup/action.yml"])
        draft = v.select([".github/actions/setup/action.yml"], draft=True)
        self.assertFalse(ready["draft"])
        self.assertEqual(ready["deferred"], [])
        self.assertEqual(draft["deferred"], list(v.DRAFT_DEFERRED))
        for lane in v.DRAFT_DEFERRED:
            self.assertTrue(ready[lane], lane)
            self.assertFalse(draft[lane], lane)
        self.assertEqual(draft["package_matrix"], {"include": []})
        # Fast lanes are unchanged, and macOS host checks move off the deferred packaging job.
        for lane in ("host", "windows", "android", "ios", "credentials", "stress", "fuzz"):
            self.assertEqual(draft[lane], ready[lane], lane)
        self.assertEqual(ready["macos_host"], "packages")
        self.assertEqual(draft["macos_host"], "host")
        # Every selected lane passing still fails a draft, naming what it deferred, so a
        # draft's result can never satisfy required checks on the commit once it is ready.
        errors = v.gate(draft, self.successful_needs(draft))
        self.assertEqual(len(errors), 1)
        self.assertIn("draft pull request", errors[0])
        self.assertIn("packages, windows-package, coverage, smoke", errors[0])
        self.assertEqual(v.gate(ready, self.successful_needs(ready)), [])
        # A failing lane is still reported beside the draft notice.
        needs = self.successful_needs(draft)
        needs["host"]["result"] = "failure"
        self.assertIn("selected host: failure", v.gate(draft, needs))
        # A draft whose change selects no heavy lane still says it is a draft.
        docs = v.select(["crates/iptv-core/src/guide.rs"], draft=True)
        self.assertEqual(docs["deferred"], [x for x in v.DRAFT_DEFERRED if self.plan[x]])

    def test_draft_selection_is_validated(self):
        for tier in ("full", "release"):
            with self.assertRaises(ValueError):
                v.select([], tier, draft=True)
        with self.assertRaises(ValueError):
            v.select([], draft="true")
        for plan in ({**self.plan, "draft": "yes"}, {**self.plan, "deferred": ["host"]},
                     {**self.plan, "deferred": ["smoke"]}):
            self.assertIn("draft selection is missing or invalid", v.gate(plan, self.needs))
        self.assertEqual(v.gate({k: x for k, x in self.plan.items() if k not in ("draft", "deferred")}, self.needs), [])

    def test_mac_and_ios_evidence_must_come_from_the_selected_package_runner(self):
        plan = v.select(["Cargo.lock", "tools/package-macos.sh"])
        needs = self.successful_needs(plan)
        self.assertEqual(v.gate(plan, needs), [])
        needs["host"]["outputs"].update(needs["packages"].pop("outputs"))
        self.assertIn("macOS host checks did not report success", v.gate(plan, needs))
        self.assertIn("selected ios: missing successful host verification", v.gate(plan, needs))

    def test_dependency_pr_requires_native_evidence_with_both_installers_skipped(self):
        plan = v.select(["Cargo.toml", "Cargo.lock"])
        needs = self.successful_needs(plan)
        self.assertEqual(needs["packages"]["result"], "skipped")
        self.assertEqual(needs["windows-package"]["result"], "skipped")
        self.assertEqual(v.gate(plan, needs), [])
        needs["host"]["outputs"].pop("macos-host-verified")
        self.assertIn("macOS host checks did not report success", v.gate(plan, needs))

    def test_host_and_standalone_routing(self):
        cases = (("credentials", "tools/ci/isolated-credentials.py", False),
                 ("credentials", "crates/iptv-store-sqlite/src/lib.rs", True),
                 ("ios", "tools/ci/check-ios.sh", False),
                 ("ios", "crates/iptv-core/src/guide.rs", True))
        for lane, path, on_host in cases:
            with self.subTest(lane=lane, path=path):
                plan = v.select([path])
                self.assertTrue(plan[lane])
                self.assertEqual(plan[f"{lane}_on_host"], on_host)
                needs = self.successful_needs(plan)
                self.assertEqual(v.gate(plan, needs), [])
                if on_host:
                    evidence = v.HOST_VERIFICATIONS[lane]
                    needs["host"]["outputs"].pop(evidence)
                    self.assertIn(f"selected {lane}: missing successful host verification", v.gate(plan, needs))
                    needs["host"]["outputs"][evidence] = "true"
                needs[lane]["result"] = "failure"
                self.assertTrue(v.gate(plan, needs))
                if not on_host:
                    plan[f"{lane}_on_host"] = True
                    self.assertIn(f"host routing is missing or invalid for {lane}", v.gate(plan, needs))

    def test_edits_to_standalone_workflows_exercise_their_own_setup_alongside_host_tests(self):
        for lane in ("credentials", "ios"):
            with self.subTest(lane=lane):
                paths = [f".github/workflows/{lane}.yml", "crates/iptv-core/src/guide.rs"]
                plan = v.select(paths)
                self.assertTrue(plan["host"])
                self.assertTrue(plan[lane])
                self.assertFalse(plan[f"{lane}_on_host"])
                needs = self.successful_needs(plan)
                self.assertEqual(v.gate(plan, needs), [])
                needs["host"].setdefault("outputs", {})[v.HOST_VERIFICATIONS[lane]] = "true"
                needs[lane]["result"] = "skipped"
                self.assertIn(f"selected {lane}: skipped", v.gate(plan, needs))
                self.assertEqual(
                    v.select(paths, "release", candidate_build=True),
                    v.select([], "release", candidate_build=True),
                )

    def test_android_compile_and_windows_failures_block_even_when_packages_skip(self):
        for lane in ("android", "windows"):
            self.needs[lane]["result"] = "failure"
            self.assertTrue(v.gate(self.plan, self.needs))
            self.needs[lane]["result"] = "success"

    def test_canceled_missing_or_unexpected_skipped_selected_job_fails(self):
        for result in ("cancelled", "failure", "skipped", None):
            self.needs["host"]["result"] = result
            self.assertTrue(v.gate(self.plan, self.needs))

    def test_missing_or_failed_classification_fails(self):
        for result in ("failure", "cancelled", "skipped", None):
            self.needs["plan"]["result"] = result
            self.assertTrue(v.gate(self.plan, self.needs))
        self.assertTrue(v.gate({}, self.needs))

    def test_missing_or_failed_helpers_fail(self):
        self.needs.pop("helpers")
        self.assertTrue(v.gate(self.plan, self.needs))

    def test_corrupted_plan_values_cannot_be_accepted_as_skips(self):
        for value in ("false", 0, None):
            self.plan["host"] = value
            self.assertTrue(v.gate(self.plan, self.needs))
        self.plan["host"] = True
        self.plan["fuzz"] = "[]"
        self.assertTrue(v.gate(self.plan, self.needs))

    def test_unexpected_failure_on_an_unselected_job_is_not_hidden(self):
        self.needs["coverage"]["result"] = "failure"
        self.assertTrue(v.gate(self.plan, self.needs))


if __name__ == "__main__":
    unittest.main()
