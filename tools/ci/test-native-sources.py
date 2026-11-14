#!/usr/bin/env python3
"""Source collection for bundled native libraries, without Homebrew, apt or network."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("native_sources", Path(__file__).with_name("native-sources.py"))
sources = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sources)

A, B, C, D = ("a" * 64, "b" * 64, "c" * 64, "d" * 64)
COMMIT = "0123456789abcdef0123456789abcdef01234567"

FORMULA = f'''class Synthetic < Formula
  desc "Synthetic media library"
  homepage "https://example.test/synthetic"
  url "https://example.test/releases/synthetic-1.2.3.tar.xz"
  mirror "https://mirror.example.test/synthetic-1.2.3.tar.xz"
  version "1.2.3"
  sha256 "{A}"
  license "LGPL-2.1-or-later"
  head "https://example.test/synthetic.git", branch: "main"

  livecheck do
    url "https://example.test/releases/"
    regex(/synthetic[._-]v?(\\d+(?:\\.\\d+)+)/i)
  end

  bottle do
    sha256 cellar: :any, arm64_sequoia: "{D}"
  end

  head do
    url "https://example.test/synthetic.git", branch: "main"
    depends_on "autoconf" => :build
  end

  depends_on "pkgconf" => :build

  on_linux do
    resource "linux-only" do
      url "https://example.test/linux-only.tar.gz"
      sha256 "{D}"
    end
  end

  on_macos do
    patch do
      url "https://example.test/patches/apple.diff"
      sha256 "{B}"
    end
  end

  resource "tables" do
    url "https://example.test/tables-4.tar.gz",
        using: :nounzip
    sha256 "{C}"
  end

  resource "codec" do
    url "https://example.test/codec.git",
        tag:      "v9",
        revision: "{COMMIT}"
  end

  resource "sample-media" do
    url "https://example.test/sample.ogg"
    sha256 "{D}"
  end

  resource "sample-font", :test do
    url "https://example.test/sample.ttf"
    sha256 "{D}"
  end

  def install
    url = "https://example.test/not-a-source"
    resource("tables").stage buildpath/"tables"
    system "./configure", "--prefix=#{{prefix}}"
  end

  test do
    resource "homebrew-sample" do
      url "https://example.test/sample.bin"
      sha256 "{D}"
    end
    resource("sample-media").stage testpath
    system bin/"synthetic", "--version"
  end
end

__END__
--- a/build.rb
+++ b/build.rb
 url "https://example.test/inside-an-embedded-patch"
'''


class Formulae(unittest.TestCase):
    def test_stable_archive_patches_and_resources_are_found(self):
        self.assertEqual(sources.formula_sources(FORMULA, "synthetic"), [
            {"role": "source", "url": "https://example.test/releases/synthetic-1.2.3.tar.xz", "sha256": A,
             "mirrors": ["https://mirror.example.test/synthetic-1.2.3.tar.xz"]},
            {"role": "patch", "url": "https://example.test/patches/apple.diff", "sha256": B},
            {"role": "resource:tables", "url": "https://example.test/tables-4.tar.gz", "sha256": C},
            {"role": "resource:codec", "url": "https://example.test/codec.git", "git": True,
             "revision": COMMIT, "tag": "v9"},
        ])

    def test_stable_block_and_git_main_source(self):
        text = f'''class Other < Formula
  stable do
    url "https://example.test/other.git", revision: "{COMMIT}"

    patch do
      url "https://example.test/fix.diff"
      sha256 "{B}"
    end
  end
end
'''
        self.assertEqual(sources.formula_sources(text, "other"), [
            {"role": "source", "url": "https://example.test/other.git", "git": True, "revision": COMMIT},
            {"role": "patch", "url": "https://example.test/fix.diff", "sha256": B},
        ])

    def test_sources_that_cannot_be_verified_are_refused(self):
        cases = {
            "no SHA-256": 'class X < Formula\n  url "https://example.test/x.tar.gz"\n  license "MIT"\nend\n',
            "not pinned to a commit": 'class X < Formula\n  url "https://example.test/x.git", tag: "v1"\nend\n',
            "names no stable source": 'class X < Formula\n  head do\n    url "https://example.test/x.git"\n  end\nend\n',
            "outside the formula class": 'url "https://example.test/x.tar.gz"\n',
        }
        for message, text in cases.items():
            with self.subTest(message=message):
                with self.assertRaisesRegex(ValueError, message):
                    sources.formula_sources(text, "x")

    def test_numbered_upstream_patches_are_expanded(self):
        pairs = f"""
    001 {B}
    002 {C}
  ]
"""
        named = f"""class X < Formula
  url "https://example.test/x-8.3.tar.gz"
  sha256 "{A}"

  patch_checksum_pairs = %w[{pairs}
  patch_checksum_pairs.each_slice(2) do |p, checksum|
    patch :p0 do
      url "https://example.test/x-8.3-patches/x83-#{{p}}"
      mirror "https://mirror.example.test/x83-#{{p}}"
      sha256 checksum
    end
  end
end
"""
        inline = named.replace("patch_checksum_pairs = %w[", "%w[").replace(
            "  ]\n\n  patch_checksum_pairs.each_slice", "  ].each_slice")
        self.assertIn("].each_slice(2) do |p, checksum|", inline)
        for text in (named, inline):
            with self.subTest(text=text):
                self.assertEqual(sources.formula_sources(text, "x")[1:], [
                    {"role": "patch", "url": "https://example.test/x-8.3-patches/x83-001",
                     "mirrors": ["https://mirror.example.test/x83-001"], "sha256": B},
                    {"role": "patch", "url": "https://example.test/x-8.3-patches/x83-002",
                     "mirrors": ["https://mirror.example.test/x83-002"], "sha256": C},
                ])

    def test_addresses_and_checksums_computed_at_install_time_are_refused(self):
        spliced = f'class X < Formula\n  url "https://example.test/x.tar.gz"\n  sha256 "{A}"\n\n' \
                  f'  resource "data" do\n    url "https://example.test/data-#{{version}}.tar.gz"\n    sha256 "{B}"\n  end\nend\n'
        with self.assertRaisesRegex(ValueError, "cannot resolve the address"):
            sources.formula_sources(spliced, "x")
        computed = f'class X < Formula\n  url "https://example.test/x.tar.gz"\n  sha256 "{A}"\n\n' \
                   '  patch do\n    url "https://example.test/fix.diff"\n    sha256 checksum\n  end\nend\n'
        with self.assertRaisesRegex(ValueError, "cannot resolve the checksum"):
            sources.formula_sources(computed, "x")

    def test_checksum_of_a_later_block_is_not_borrowed(self):
        text = f'''class X < Formula
  url "https://example.test/x.tar.gz"
  license "MIT"

  resource "later" do
    url "https://example.test/later.tar.gz"
    sha256 "{B}"
  end
end
'''
        with self.assertRaisesRegex(ValueError, "x.tar.gz has no SHA-256"):
            sources.formula_sources(text, "x")

    def test_installed_keg_is_described_by_its_own_formula(self):
        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory) / "Cellar/synthetic/1.2.3_1"
            (prefix / ".brew").mkdir(parents=True)
            (prefix / ".brew/synthetic.rb").write_text(FORMULA)
            component, text = sources.homebrew_component(prefix)
            self.assertEqual((component["name"], component["version"], component["recipe"]),
                             ("synthetic", "1.2.3_1", "synthetic-1.2.3_1/formula.rb"))
            self.assertEqual(text, FORMULA)
            (prefix / ".brew/synthetic.rb").unlink()
            with self.assertRaisesRegex(ValueError, "expected one installed formula"):
                sources.homebrew_component(prefix)


class Ubuntu(unittest.TestCase):
    def test_source_package_and_version_come_from_the_installed_package(self):
        self.assertEqual(sources.source_package("synthetic-media 7:1.2-3ubuntu4", "libsynthetic1:amd64"),
                         {"source": "synthetic-media", "source_version": "7:1.2-3ubuntu4"})
        with self.assertRaisesRegex(ValueError, "libsynthetic1"):
            sources.source_package("", "libsynthetic1:amd64")

    def test_libraries_are_grouped_by_source_package_version(self):
        rows = [
            {"package": "libsyntheticcodec1:amd64", "source": "synthetic-media", "source_version": "7:1.2-3"},
            {"package": "libsyntheticformat1:amd64", "source": "synthetic-media", "source_version": "7:1.2-3"},
            {"package": "libsyntheticformat1:amd64", "source": "synthetic-media", "source_version": "7:1.2-3"},
            {"package": "libother0:amd64", "source": "other", "source_version": "0.5-1build2"},
        ]
        self.assertEqual(sources.ubuntu_components(rows), [
            {"name": "other", "version": "0.5-1build2", "binaries": ["libother0:amd64"]},
            {"name": "synthetic-media", "version": "7:1.2-3",
             "binaries": ["libsyntheticcodec1:amd64", "libsyntheticformat1:amd64"]},
        ])

    def test_print_uris_lines_are_parsed_and_notes_ignored(self):
        output = f"""Reading package lists...
NOTICE: 'synthetic-media' packaging is maintained in the 'Git' version control system
'http://archive.example.test/pool/s/synthetic-media_1.2-3.dsc' synthetic-media_1.2-3.dsc 2198 SHA512:{"e" * 128}
'http://archive.example.test/pool/s/synthetic-media_1.2.orig.tar.xz' synthetic-media_1.2.orig.tar.xz 725946 SHA256:{A}
"""
        self.assertEqual(sources.print_uris(output), [
            {"url": "http://archive.example.test/pool/s/synthetic-media_1.2-3.dsc", "name": "synthetic-media_1.2-3.dsc",
             "size": 2198, "algorithm": "sha512", "checksum": "e" * 128},
            {"url": "http://archive.example.test/pool/s/synthetic-media_1.2.orig.tar.xz",
             "name": "synthetic-media_1.2.orig.tar.xz", "size": 725946, "algorithm": "sha256", "checksum": A},
        ])
        with self.assertRaisesRegex(ValueError, "no source description"):
            sources.print_uris("Reading package lists...\n")

    def test_source_description_lists_its_files(self):
        text = f"""-----BEGIN PGP SIGNED MESSAGE-----
Hash: SHA512

Format: 3.0 (quilt)
Source: synthetic-media
Version: 7:1.2-3
Checksums-Sha1:
 {"f" * 40} 725946 synthetic-media_1.2.orig.tar.xz
Checksums-Sha256:
 {A} 725946 synthetic-media_1.2.orig.tar.xz
 {B} 12816 synthetic-media_1.2-3.debian.tar.xz
Files:
 {"0" * 32} 725946 synthetic-media_1.2.orig.tar.xz

-----BEGIN PGP SIGNATURE-----
"""
        self.assertEqual(sources.dsc_files(text), [
            {"name": "synthetic-media_1.2.orig.tar.xz", "size": 725946, "algorithm": "sha256", "checksum": A},
            {"name": "synthetic-media_1.2-3.debian.tar.xz", "size": 12816, "algorithm": "sha256", "checksum": B},
        ])
        for bad in ("Format: 3.0\n", f"Checksums-Sha256:\n {A} 1 ../escape.tar\n", "Checksums-Sha256:\n short 1 x.tar\n"):
            with self.assertRaises(ValueError):
                sources.dsc_files(bad)

    def test_launchpad_names_keep_the_epoch_in_the_directory_only(self):
        self.assertEqual(sources.dsc_name("synthetic-media", "7:1.2-3"), "synthetic-media_1.2-3.dsc")
        self.assertEqual(
            sources.launchpad_url("synthetic-media", "7:1.2-3", "synthetic-media_1.2-3.dsc"),
            "https://launchpad.net/ubuntu/+archive/primary/+sourcefiles/synthetic-media/7:1.2-3/synthetic-media_1.2-3.dsc")


class Notes(unittest.TestCase):
    def test_note_names_the_release_archive(self):
        for platform in ("macos", "linux"):
            note = sources.source_note(platform, "9.8.7")
            self.assertIn(f"Patina-9.8.7-{platform}-native-sources.tar", note)
            self.assertIn("https://github.com/teng-lin/patina-iptv/releases/tag/v9.8.7", note)
            self.assertIn("sources.json", note)


def payload(text):
    data = text.encode()
    return data, hashlib.sha256(data).hexdigest()


class Collecting(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.files = {}
        self.attempts = []

    def download(self, url, destination):
        self.attempts.append(url)
        if url not in self.files:
            return False
        Path(destination).write_bytes(self.files[url])
        return True

    def macos_package(self, checksum=None, with_git=True):
        archive, digest = payload("synthetic upstream archive")
        self.files["https://mirror.example.test/synthetic-1.2.3.tar.xz"] = archive
        notices = self.root / "Patina.app" / sources.NOTICES["macos"]
        (notices / "synthetic-1.2.3_1").mkdir(parents=True)
        (notices / "synthetic-1.2.3_1/formula.rb").write_text("class Synthetic < Formula\nend\n")
        listed = [{"role": "source", "url": "https://example.test/releases/synthetic-1.2.3.tar.xz",
                   "mirrors": ["https://mirror.example.test/synthetic-1.2.3.tar.xz"], "sha256": checksum or digest}]
        if with_git:
            listed.append({"role": "resource:codec", "url": "https://example.test/codec.git", "git": True,
                           "revision": COMMIT})
        component = {"name": "synthetic", "version": "1.2.3_1", "recipe": "synthetic-1.2.3_1/formula.rb",
                     "sources": listed}
        (notices / "sources.json").write_text(json.dumps(sources.manifest("macos", "9.8.7", [component])))
        (notices / "SOURCE.txt").write_text(sources.source_note("macos", "9.8.7"))
        return self.root / "Patina.app"

    def collect_macos(self, package, git=True):
        def git_archive(url, revision, destination):
            Path(destination).write_bytes(b"synthetic commit archive")
            return git

        def fetch(component, stage):
            sources.fetch_homebrew(component, stage, download=self.download, git_archive=git_archive)

        output = self.root / "out" / sources.archive_name("macos", "9.8.7")
        return sources.collect("macos", package, output, fetch=fetch), output

    def test_macos_app_with_only_the_pinned_runtime_needs_no_archive(self):
        package = self.root / "Patina.app"
        (package / sources.NOTICES["macos"]).mkdir(parents=True)
        output = self.root / "out" / sources.archive_name("macos", "9.8.7")
        self.assertIsNone(sources.collect("macos", package, output))
        self.assertFalse(output.exists())

    def test_linux_package_without_a_manifest_fails(self):
        package = self.root / "Patina.AppDir"
        (package / sources.NOTICES["linux"]).mkdir(parents=True)
        with self.assertRaises(FileNotFoundError):
            sources.collect("linux", package, self.root / "out" / sources.archive_name("linux", "9.8.7"))

    def test_macos_archive_holds_verified_sources_recipe_and_manifest(self):
        description, output = self.collect_macos(self.macos_package())
        with tarfile.open(output) as archive:
            names = archive.getnames()
            inside = json.load(archive.extractfile("sources.json"))
            self.assertEqual(archive.extractfile("synthetic-1.2.3_1/synthetic-1.2.3.tar.xz").read(),
                             b"synthetic upstream archive")
            self.assertTrue(all(member.mtime == 0 and member.uid == 0 and member.mode == 0o644 for member in archive))
        self.assertEqual(names, ["SOURCE.txt", "sources.json", "synthetic-1.2.3_1/formula.rb",
                                 f"synthetic-1.2.3_1/synthetic-{COMMIT}.tar", "synthetic-1.2.3_1/synthetic-1.2.3.tar.xz"])
        self.assertEqual(inside, description)
        self.assertEqual(json.loads(output.with_suffix(".json").read_text()), description)
        self.assertEqual(output.with_suffix(".json").name, "Patina-9.8.7-macos-native-sources.json")
        listed = description["components"][0]["sources"]
        self.assertEqual(listed[0]["file"], "synthetic-1.2.3_1/synthetic-1.2.3.tar.xz")
        self.assertEqual(listed[1]["sha256"], hashlib.sha256(b"synthetic commit archive").hexdigest())
        # The primary address failed; the mirror supplied the same checksummed bytes.
        self.assertEqual(self.attempts, ["https://example.test/releases/synthetic-1.2.3.tar.xz",
                                         "https://mirror.example.test/synthetic-1.2.3.tar.xz"])

    def test_archive_is_reproducible(self):
        _, first = self.collect_macos(self.macos_package())
        before = first.read_bytes()
        _, second = self.collect_macos(self.root / "Patina.app")
        self.assertEqual(before, second.read_bytes())

    def test_checksum_mismatch_unreachable_source_and_wrong_commit_fail(self):
        with self.assertRaisesRegex(SystemExit, "checksum mismatch"):
            self.collect_macos(self.macos_package(checksum=D))
        self.files.clear()
        with self.assertRaisesRegex(SystemExit, "cannot fetch source of synthetic 1.2.3_1"):
            self.collect_macos(self.root / "Patina.app")
        self.setUp()
        with self.assertRaisesRegex(SystemExit, "cannot fetch source of synthetic 1.2.3_1 .resource:codec"):
            self.collect_macos(self.macos_package(), git=False)
        self.assertFalse((self.root / "out" / sources.archive_name("macos", "9.8.7")).exists())

    def test_wrong_archive_name_or_empty_manifest_is_refused(self):
        package = self.macos_package()
        with self.assertRaisesRegex(SystemExit, "names its source archive"):
            sources.collect("macos", package, self.root / "out/other.tar", fetch=lambda component, stage: None)
        manifest = package / sources.NOTICES["macos"] / "sources.json"
        manifest.write_text(json.dumps(sources.manifest("macos", "9.8.7", [])))
        with self.assertRaisesRegex(SystemExit, "does not describe"):
            self.collect_macos(package)

    def linux_package(self):
        notices = self.root / "AppDir" / sources.NOTICES["linux"]
        notices.mkdir(parents=True)
        component = {"name": "synthetic-media", "version": "7:1.2-3", "binaries": ["libsynthetic1:amd64"]}
        (notices / "sources.json").write_text(json.dumps(sources.manifest("linux", "9.8.7", [component])))
        (notices / "SOURCE.txt").write_text(sources.source_note("linux", "9.8.7"))
        return self.root / "AppDir"

    def collect_linux(self, apt):
        def fetch(component, stage):
            sources.fetch_ubuntu(component, stage, run=apt, download=self.download)

        output = self.root / "out" / sources.archive_name("linux", "9.8.7")
        return sources.collect("linux", self.linux_package(), output, fetch=fetch), output

    def source_files(self):
        original, original_digest = payload("synthetic original source")
        packaging, packaging_digest = payload("synthetic packaging")
        description = (f"Format: 3.0 (quilt)\nSource: synthetic-media\nChecksums-Sha256:\n"
                       f" {original_digest} {len(original)} synthetic-media_1.2.orig.tar.xz\n"
                       f" {packaging_digest} {len(packaging)} synthetic-media_1.2-3.debian.tar.xz\n").encode()
        return {"synthetic-media_1.2-3.dsc": description, "synthetic-media_1.2.orig.tar.xz": original,
                "synthetic-media_1.2-3.debian.tar.xz": packaging}

    def test_linux_sources_come_from_apt_at_the_exact_version(self):
        files = self.source_files()
        calls = []

        class Result:
            def __init__(self, returncode=0, stdout="", stderr=""):
                self.returncode, self.stdout, self.stderr = returncode, stdout, stderr

        def apt(arguments, cwd=None):
            calls.append(arguments)
            self.assertEqual(arguments[:2] + arguments[3:], ["apt-get", "source", "synthetic-media=7:1.2-3"])
            if arguments[2] == "--print-uris":
                return Result(stdout="".join(
                    f"'http://archive.example.test/pool/{name}' {name} {len(data)} SHA512:{hashlib.sha512(data).hexdigest()}\n"
                    for name, data in files.items()))
            for name, data in files.items():
                (Path(cwd) / name).write_bytes(data)
            return Result()

        description, output = self.collect_linux(apt)
        component = description["components"][0]
        self.assertEqual(component["origin"], "apt")
        self.assertEqual([call[2] for call in calls], ["--print-uris", "--download-only"])
        self.assertEqual(
            [(row["file"], row["url"], row["sha256"]) for row in component["sources"]],
            [(f"synthetic-media_1.2-3/{name}", f"http://archive.example.test/pool/{name}",
              hashlib.sha256(data).hexdigest()) for name, data in files.items()])
        with tarfile.open(output) as archive:
            self.assertEqual(archive.getnames(), [
                "SOURCE.txt", "sources.json", "synthetic-media_1.2-3/synthetic-media_1.2-3.debian.tar.xz",
                "synthetic-media_1.2-3/synthetic-media_1.2-3.dsc", "synthetic-media_1.2-3/synthetic-media_1.2.orig.tar.xz"])
        # A file that differs from the signed index is refused.
        files["synthetic-media_1.2.orig.tar.xz"] = b"different bytes"
        original = apt

        def tampering(arguments, cwd=None):
            result = original(arguments, cwd)
            if arguments[2] == "--download-only":
                (Path(cwd) / "synthetic-media_1.2.orig.tar.xz").write_bytes(b"tampered after indexing")
            return result

        self.setUp()
        with self.assertRaisesRegex(SystemExit, "checksum mismatch"):
            self.collect_linux(tampering)

    def test_superseded_version_falls_back_to_launchpad_and_its_description(self):
        files = self.source_files()
        base = "https://launchpad.net/ubuntu/+archive/primary/+sourcefiles/synthetic-media/7:1.2-3/"
        self.files = {base + name: data for name, data in files.items()}

        class Missing:
            returncode, stdout, stderr = 100, "", "E: Can't find a source to download version '7:1.2-3' of 'synthetic-media'"

        description, _ = self.collect_linux(lambda arguments, cwd=None: Missing)
        component = description["components"][0]
        self.assertEqual(component["origin"], "launchpad")
        self.assertEqual([row["url"] for row in component["sources"]], [base + name for name in files])
        # A file that does not match the description is refused.
        self.setUp()
        self.files = {base + name: data for name, data in files.items()}
        self.files[base + "synthetic-media_1.2.orig.tar.xz"] = b"different bytes"
        with self.assertRaisesRegex(SystemExit, "checksum mismatch"):
            self.collect_linux(lambda arguments, cwd=None: Missing)
        # Not on Launchpad either: fail.
        self.setUp()
        with self.assertRaisesRegex(SystemExit, "neither the configured deb-src indexes nor Launchpad"):
            self.collect_linux(lambda arguments, cwd=None: Missing)

    def test_missing_deb_src_configuration_is_not_papered_over(self):
        class Unconfigured:
            returncode, stdout, stderr = 100, "", "E: You must put some 'deb-src' URIs in your sources.list"

        with self.assertRaisesRegex(SystemExit, "enable-deb-src.sh"):
            self.collect_linux(lambda arguments, cwd=None: Unconfigured)
        self.assertEqual(self.attempts, [])


if __name__ == "__main__":
    unittest.main()
