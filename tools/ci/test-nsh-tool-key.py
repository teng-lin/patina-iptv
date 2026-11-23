#!/usr/bin/env python3
"""Check invalidation boundaries without compiling or downloading the toolchain."""
import importlib.util
import json
import os
import subprocess
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "nsh_tool_key", Path(__file__).with_name("nsh-tool-key.py")
)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
release_spec = importlib.util.spec_from_file_location(
    "nsh_cli_release", Path(__file__).with_name("nsh-cli-release.py")
)
releases = importlib.util.module_from_spec(release_spec)
release_spec.loader.exec_module(releases)


class ToolIdentityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        previous = os.getcwd()
        os.chdir(self.temp.name)
        self.addCleanup(os.chdir, previous)
        for action in ("nsh-cli", "android"):
            path = Path(f".github/actions/{action}/action.yml")
            path.parent.mkdir(parents=True)
            path.write_text(f"recipe: {action}\n")
        self.versions = {"git": "nsh-commit", "rustc": "rust-version", "java": "java-version"}
        self.sdk = Path(self.temp.name) / "sdk"
        self.build_tools = self.sdk / "build-tools"
        (self.build_tools / "35.0.1").mkdir(parents=True)
        (self.build_tools / "35.0.1/source.properties").write_text("Pkg.Revision=35.0.1\n")
        self.environment = patch.dict(os.environ, {
            "CARGO_HOME": str(Path(self.temp.name) / "cargo-home"),
            "ANDROID_HOME": str(self.sdk),
        })
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.commands = patch.object(
            module.subprocess, "check_output",
            side_effect=lambda args, **kwargs: self.versions[args[0]],
        )
        self.command_mock = self.commands.start()
        self.addCleanup(self.commands.stop)

    def test_application_changes_do_not_invalidate_tools(self):
        before = [module.identity(kind) for kind in ("cli", "android")]
        Path("Cargo.lock").write_text("different application dependencies\n")
        Path("app.rs").write_text("different application code\n")
        self.assertEqual(before, [module.identity(kind) for kind in ("cli", "android")])

    def test_nsh_and_rust_changes_invalidate_both_tools(self):
        for command in ("git", "rustc"):
            before = [module.identity(kind) for kind in ("cli", "android")]
            self.versions[command] += "-new"
            after = [module.identity(kind) for kind in ("cli", "android")]
            self.assertTrue(all(a != b for a, b in zip(before, after)))

    def test_java_changes_invalidate_only_android(self):
        cli, android = module.identity("cli"), module.identity("android")
        self.versions["java"] += "-new"
        self.assertEqual(cli, module.identity("cli"))
        self.assertNotEqual(android, module.identity("android"))

    def test_new_build_tools_invalidate_only_android(self):
        cli, android = module.identity("cli"), module.identity("android")
        (self.build_tools / "36.0.0").mkdir()
        (self.build_tools / "36.0.0/source.properties").write_text("Pkg.Revision=36.0.0\n")
        self.assertEqual(cli, module.identity("cli"))
        self.assertNotEqual(android, module.identity("android"))

    def test_build_tools_revision_is_part_of_identity(self):
        before = module.identity("android")
        (self.build_tools / "35.0.1/source.properties").write_text("Pkg.Revision=35.0.2\n")
        self.assertNotEqual(before, module.identity("android"))

    def test_missing_build_tools_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / "build-tools").mkdir()
            with patch.dict(os.environ, {"ANDROID_HOME": directory}):
                with self.assertRaises(ValueError):
                    module.identity("android")

    def test_recipe_changes_invalidate_the_affected_tool(self):
        cli, android = module.identity("cli"), module.identity("android")
        Path(".github/actions/android/action.yml").write_text("different NDK or ABI\n")
        self.assertEqual(cli, module.identity("cli"))
        self.assertNotEqual(android, module.identity("android"))
        Path(".github/actions/nsh-cli/action.yml").write_text("different build profile\n")
        self.assertNotEqual(cli, module.identity("cli"))

    def test_compiler_flags_invalidate_tools(self):
        before = module.identity("cli")
        with patch.dict(os.environ, {"RUSTFLAGS": "-C target-cpu=test-cpu"}):
            self.assertNotEqual(before, module.identity("cli"))

    def test_inherited_cargo_config_invalidates_cli(self):
        before = module.identity("cli")
        Path(".cargo").mkdir()
        Path(".cargo/config.toml").write_text('[build]\nrustflags = ["-C", "opt-level=2"]\n')
        self.assertNotEqual(before, module.identity("cli"))

    def test_compiler_probe_uses_the_build_directory(self):
        for kind, directory in (("cli", "."), ("android", "../nsh-patina")):
            module.identity(kind)
            self.command_mock.assert_any_call(["rustc", "-vV"], cwd=directory, text=True)

    def test_additional_cargo_flags_invalidate_tools(self):
        for flag in ("CARGO_BUILD_RUSTFLAGS", "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER", "CARGO_PROFILE_RELEASE_DEBUG", "CC_aarch64_linux_android"):
            for kind in ("cli", "android"):
                before = module.identity(kind)
                with patch.dict(os.environ, {flag: "changed"}):
                    self.assertNotEqual(before, module.identity(kind))

    def test_global_cargo_config_invalidates_both_tools(self):
        before = [module.identity(kind) for kind in ("cli", "android")]
        home = Path(os.environ["CARGO_HOME"])
        home.mkdir()
        (home / "config.toml").write_text('[build]\nrustflags = ["-C", "opt-level=2"]\n')
        after = [module.identity(kind) for kind in ("cli", "android")]
        self.assertTrue(all(a != b for a, b in zip(before, after)))

    def test_unknown_tool_is_rejected(self):
        with self.assertRaises(ValueError):
            module.identity("typo")


class IdentityStepTests(unittest.TestCase):
    def test_identity_step_errors_and_canonical_cache_paths(self):
        root = Path(__file__).resolve().parents[2]
        for action in ("nsh-cli", "android"):
            # Execute the actual identity step shell, not a second implementation.
            lines = (root / f".github/actions/{action}/action.yml").read_text().splitlines()
            start = next(i for i, line in enumerate(lines) if "id: identity" in line)
            start = next(i for i in range(start, len(lines)) if lines[i] == "      run: |") + 1
            end = start
            while end < len(lines) and lines[end].startswith("        "):
                end += 1
            script = "\n".join(line[8:] for line in lines[start:end])
            self.assertEqual(
                [line.strip() for line in lines if line.strip().startswith("path:")],
                ["path: ${{ steps.identity.outputs.path }}"] * (3 if action == "nsh-cli" else 2),
            )
            for fake_python in ("exit 1", "echo invalid-key", "exit 0", "echo " + "0" * 64):
                with self.subTest(action=action, fake_python=fake_python):
                    with tempfile.TemporaryDirectory() as directory:
                        executable = Path(directory) / "python3"
                        executable.write_text("#!/bin/sh\n" + fake_python + "\n")
                        executable.chmod(0o755)
                        workspace = Path(directory) / "app"
                        workspace.mkdir()
                        nsh = Path(directory) / "nsh-patina"
                        nsh.mkdir()
                        output = Path(directory) / "output"
                        env = dict(os.environ, PATH=directory + os.pathsep + os.environ["PATH"], GITHUB_OUTPUT=str(output))
                        result = subprocess.run(["bash", "-eo", "pipefail", "-c", script], env=env, cwd=workspace, capture_output=True)
                        if fake_python == "echo " + "0" * 64:
                            self.assertEqual(result.returncode, 0, result.stderr)
                            values = dict(line.split("=", 1) for line in output.read_text().splitlines())
                            suffix = "target/release/nsh" if action == "nsh-cli" else "target/android/release"
                            self.assertEqual(values["path"], str(nsh.resolve() / suffix))
                            self.assertNotIn("..", Path(values["path"]).parts)
                        else:
                            self.assertNotEqual(result.returncode, 0)
                            self.assertFalse(output.exists())


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.source = self.root / "nsh"
        self.source.write_bytes(b"synthetic executable")
        self.key = "a" * 64
        self.platform = "macOS-ARM64"
        self.tag = releases.tag_for(self.platform, self.key)
        self.manifest = self.root / releases.MANIFEST
        self.manifest.write_text(json.dumps({"tag": self.tag, "digest": releases.digest_file(self.source)}))
        self.license = self.root / releases.LICENSE
        self.license.write_bytes(b"synthetic license")
        self.release = {
            "tagName": self.tag,
            "isDraft": False,
            "isImmutable": True,
            "isPrerelease": True,
            "assets": [{"name": asset.name, "state": "uploaded", "size": asset.stat().st_size,
                        "digest": releases.digest_file(asset)} for asset in (self.source, self.manifest, self.license)],
        }

    def download(self, directory, binary=None):
        (directory / "nsh").write_bytes(self.source.read_bytes() if binary is None else binary)
        (directory / releases.MANIFEST).write_bytes(self.manifest.read_bytes())
        (directory / releases.LICENSE).write_bytes(self.license.read_bytes())

    def test_restore_rejects_wrong_bytes_without_replacing_existing_file(self):
        destination = self.root / "bin" / "nsh"
        destination.parent.mkdir()
        destination.write_bytes(b"existing executable")

        def download(*args):
            self.download(Path(args[-1]), b"corrupt executable")
            return subprocess.CompletedProcess(args, 0)

        with patch.object(releases, "lookup", return_value=self.release), patch.object(releases, "gh", side_effect=download):
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                releases.restore(self.platform, self.key, destination)
        self.assertEqual(destination.read_bytes(), b"existing executable")

    def test_verified_release_installs_an_executable(self):
        destination = self.root / "bin" / "nsh"

        def command(*args):
            if args[:2] == ("release", "download"):
                self.download(Path(args[-1]))
                return subprocess.CompletedProcess(args, 0)
            self.assertEqual(args[:2], ("attestation", "verify"))
            self.assertIn(releases.SIGNER, args)
            self.assertIn("refs/heads/master", args)
            return subprocess.CompletedProcess(args, 0)

        with patch.object(releases, "lookup", return_value=self.release), patch.object(releases, "gh", side_effect=command):
            self.assertTrue(releases.restore(self.platform, self.key, destination))
        self.assertEqual(destination.read_bytes(), self.source.read_bytes())
        self.assertTrue(os.access(destination, os.X_OK))

    def test_unverified_attestation_prevents_install(self):
        destination = self.root / "bin" / "nsh"

        def command(*args):
            if args[:2] == ("release", "download"):
                self.download(Path(args[-1]))
                return subprocess.CompletedProcess(args, 0)
            return subprocess.CompletedProcess(args, 1)

        with patch.object(releases, "lookup", return_value=self.release), patch.object(releases, "gh", side_effect=command):
            with self.assertRaises(subprocess.CalledProcessError):
                releases.restore(self.platform, self.key, destination)
        self.assertFalse(destination.exists())

    def test_missing_or_mutable_release_cannot_supply_an_executable(self):
        with patch.object(releases, "lookup", return_value=None), patch.object(releases, "gh") as command:
            self.assertFalse(releases.restore(self.platform, self.key, self.root / "bin" / "nsh"))
            command.assert_not_called()
        with self.assertRaises(ValueError):
            releases.asset_digests({**self.release, "isImmutable": False}, self.tag)

    def test_retry_uploads_only_missing_draft_assets_and_publishes_verified_bytes(self):
        draft = {**self.release, "isDraft": True, "isImmutable": False}
        partial = {**draft, "assets": draft["assets"][:1]}
        with (patch.object(releases, "lookup", side_effect=[partial, draft, self.release]),
              patch.object(releases, "gh", return_value=subprocess.CompletedProcess([], 0)) as command):
            releases.publish(self.tag, self.root, "b" * 40)
        self.assertEqual([call.args[:2] for call in command.call_args_list], [("release", "upload"), ("release", "edit")])
        self.assertEqual(command.call_args_list[0].args[3:], (str(self.manifest), str(self.license)))

    def test_publish_uses_only_artifacts_from_a_trusted_master_validation(self):
        artifact = self.root / f"patina-nsh-cli-release-{self.platform}-{self.key}"
        artifact.mkdir()
        self.download(artifact)
        event = self.root / "event.json"
        run = {"event": "push", "head_branch": "master", "path": ".github/workflows/validation.yml",
               "head_repository": {"full_name": releases.REPOSITORY}, "head_sha": "b" * 40}
        with (patch.dict(os.environ, GITHUB_REPOSITORY=releases.REPOSITORY, GITHUB_REF="refs/heads/master",
                         GITHUB_EVENT_NAME="workflow_run", GITHUB_EVENT_PATH=str(event)),
              patch.object(releases, "publish") as publish,
              patch.object(releases, "upstream_license", return_value=self.license.read_bytes()) as license_fetch):
            for incorrect in ({"event": "pull_request"}, {"head_repository": {"full_name": "fork/patina-iptv"}}):
                event.write_text(json.dumps({"workflow_run": run | incorrect}))
                with self.assertRaises(ValueError):
                    releases.publish_staged(self.root)
            publish.assert_not_called()
            event.write_text(json.dumps({"workflow_run": run}))
            self.assertTrue(releases.prepare(self.root))
            license_fetch.assert_called_once_with(run["head_sha"])
            self.assertEqual(json.loads((artifact / releases.MANIFEST).read_text()), json.loads(self.manifest.read_text()))
            releases.publish_staged(self.root)
            publish.assert_called_once_with(self.tag, artifact, run["head_sha"])


if __name__ == "__main__":
    unittest.main()
