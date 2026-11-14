#!/usr/bin/env python3
"""A release is signed with the production keystore or not at all; a rehearsal
is signed with a key that exists only for its run and never sees that keystore.

tools/ci/sign-android.sh runs in a copy of the checkout whose
package-android.sh is a stand-in that records what it was given and uses the
key the way the real script does (keytool and jarsigner of the installed JDK).
The last test runs the real package-android.sh, with apksigner, on synthetic
packages; it needs an Android SDK and is skipped without one.
"""
import base64
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile

CI_DIRECTORY = Path(__file__).resolve().parent
SCRIPT = CI_DIRECTORY / "sign-android.sh"
KEYSTORE_NAME = "rehearsal-android.p12"
SECRETS = {"KEYSTORE": base64.b64encode(b"synthetic production keystore\x00\x01").decode(),
           "STORE_PASSWORD": "synthetic-store-password", "KEY_PASSWORD": "synthetic-key-password",
           "KEY_ALIAS": "synthetic-release"}
SECRET_NAMES = {"KEYSTORE": "ANDROID_KEYSTORE_BASE64", "STORE_PASSWORD": "ANDROID_STORE_PASSWORD",
                "KEY_PASSWORD": "ANDROID_KEY_PASSWORD", "KEY_ALIAS": "ANDROID_KEY_ALIAS"}

# Stands in for package-android.sh: records its environment and the keystore,
# then signs and verifies a synthetic archive with the flags the real one uses.
RECORDER = r'''
import json, os, stat, subprocess, sys, zipfile
from pathlib import Path

record = {name: os.environ.get(name) for name in
          ("KEYSTORE_PATH", "KEY_ALIAS", "STORE_PASSWORD", "KEY_PASSWORD", "KEYSTORE", "REHEARSAL")}
keystore = Path(os.environ["KEYSTORE_PATH"])
record["mode"] = stat.S_IMODE(keystore.stat().st_mode)
record["bytes"] = keystore.read_bytes().hex()
record["temp"] = sorted(path.name for path in Path(os.environ["RUNNER_TEMP"]).iterdir())
if os.environ.get("STUB_USE_KEY"):
    listing = subprocess.run(["keytool", "-list", "-v", "-keystore", str(keystore), "-storetype", "PKCS12",
                              "-storepass:env", "STORE_PASSWORD"], capture_output=True, text=True)
    record["listing"] = listing.stdout + listing.stderr
    archive = Path(os.environ["RUNNER_TEMP"]) / "synthetic.aab"
    with zipfile.ZipFile(archive, "w") as bundle:
        bundle.writestr("base/synthetic.txt", "synthetic")
    signed = subprocess.run(["jarsigner", "-keystore", str(keystore), "-storepass:env", "STORE_PASSWORD",
                             "-keypass:env", "KEY_PASSWORD", str(archive), os.environ["KEY_ALIAS"]],
                            capture_output=True, text=True)
    verified = subprocess.run(["jarsigner", "-verify", str(archive)], capture_output=True, text=True)
    record["signed"] = [signed.returncode, verified.returncode, verified.stdout]
    archive.unlink()
Path(os.environ["STUB_RECORD"]).write_text(json.dumps(record))
print("synthetic package step ran")
sys.exit(int(os.environ.get("STUB_EXIT", "0")))
'''



def jdk():
    """keytool and jarsigner that run: macOS has stubs of both that only say no JDK is installed."""
    if not (shutil.which("keytool") and shutil.which("jarsigner")):
        return False
    return subprocess.run(["keytool", "-help"], capture_output=True).returncode == 0


JDK = jdk()


def require_jdk(test):
    """The signing job installs a JDK and CI runners carry one. Elsewhere its absence only skips."""
    if JDK:
        return
    if os.environ.get("CI"):
        test.fail("keytool and jarsigner are required in CI")
    test.skipTest("no JDK (keytool, jarsigner) installed")


class Signing(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name).resolve()
        self.checkout = self.home / "checkout"
        self.temp = self.home / "runner-temp"
        self.record = self.home / "record.json"
        (self.checkout / "tools/ci").mkdir(parents=True)
        self.temp.mkdir()
        shutil.copy(SCRIPT, self.checkout / "tools/ci/sign-android.sh")
        (self.home / "recorder.py").write_text(RECORDER)
        (self.checkout / "tools/ci/package-android.sh").write_text(
            f'#!/usr/bin/env bash\nexec "{sys.executable}" "{self.home / "recorder.py"}"\n')

    def sign(self, rehearsal, event="workflow_dispatch", **environment):
        """Only what is named here reaches the script: nothing is inherited from this process."""
        base = {"PATH": os.environ["PATH"], "RUNNER_TEMP": str(self.temp), "STUB_RECORD": str(self.record),
                "GITHUB_EVENT_NAME": event, "VERSION": "9.8.7"}
        if rehearsal is not None:
            base["REHEARSAL"] = rehearsal
        return subprocess.run(["bash", str(self.checkout / "tools/ci/sign-android.sh")], capture_output=True, text=True,
                              cwd=self.home, env={**base, **environment})

    def recorded(self):
        return json.loads(self.record.read_text())

    def assert_nothing_signed(self, result, message):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(message, result.stderr)
        self.assertFalse(self.record.exists(), "the package step must not run")
        self.assertEqual(list(self.temp.iterdir()), [], "no key material may be written or left")
        self.assertNotIn("::add-mask::", result.stdout)


class Release(Signing):
    def test_production_keystore_is_decoded_outside_the_checkout_and_removed(self):
        result = self.sign("false", event="push", **SECRETS)
        self.assertEqual(result.returncode, 0, result.stderr)
        record = self.recorded()
        self.assertEqual(record["KEYSTORE_PATH"], str(self.temp / "release.jks"))
        self.assertEqual(bytes.fromhex(record["bytes"]), base64.b64decode(SECRETS["KEYSTORE"]))
        self.assertEqual(record["mode"], 0o600)
        self.assertEqual({name: record[name] for name in ("STORE_PASSWORD", "KEY_PASSWORD", "KEY_ALIAS")},
                         {name: SECRETS[name] for name in ("STORE_PASSWORD", "KEY_PASSWORD", "KEY_ALIAS")})
        # The encoded keystore is not passed on to the tools that sign.
        self.assertIsNone(record["KEYSTORE"])
        self.assertEqual(record["temp"], ["release.jks"])
        self.assertEqual(list(self.temp.iterdir()), [])
        # A release never makes a key.
        self.assertNotIn("throwaway", result.stdout + result.stderr)
        self.assertNotIn("::add-mask::", result.stdout)

    def test_keystore_is_removed_when_signing_fails(self):
        result = self.sign("false", event="push", STUB_EXIT="7", **SECRETS)
        self.assertEqual(result.returncode, 7)
        self.assertEqual(list(self.temp.iterdir()), [])

    def test_release_without_any_credential_fails_and_makes_no_key(self):
        for event in ("push", "workflow_dispatch"):
            with self.subTest(event):
                result = self.sign("false", event=event)
                self.assert_nothing_signed(result, "release signing secrets are not set")
                for name in SECRET_NAMES.values():
                    self.assertIn(name, result.stderr)
                self.assertIn("never with a debug or throwaway key", result.stderr)

    def test_release_with_one_credential_missing_or_empty_fails(self):
        for name in SECRETS:
            for value in (None, ""):
                with self.subTest(missing=name, value=value):
                    environment = {key: text for key, text in SECRETS.items() if key != name}
                    if value is not None:
                        environment[name] = value
                    result = self.sign("false", event="push", **environment)
                    self.assert_nothing_signed(result, f"({SECRET_NAMES[name]})")

    @unittest.skipUnless(sys.platform == "linux", "the signing job decodes with GNU base64, which rejects what is not base64")
    def test_keystore_that_is_not_base64_fails_before_signing(self):
        result = self.sign("false", event="push", **{**SECRETS, "KEYSTORE": "!!! not base64 !!!"})
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.record.exists())
        self.assertEqual(list(self.temp.iterdir()), [])

    def test_keystore_that_decodes_to_nothing_fails_before_signing(self):
        result = self.sign("false", event="push", **{**SECRETS, "KEYSTORE": "\n"})
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.record.exists())
        self.assertEqual(list(self.temp.iterdir()), [])

    def test_an_unclear_mode_is_never_read_as_a_rehearsal(self):
        for value in (None, "", "True", "TRUE", "1", "yes", "true ", "rehearsal"):
            with self.subTest(value=value):
                self.assert_nothing_signed(self.sign(value), "REHEARSAL must be true or false")
                # Not even with the production keystore at hand.
                self.assert_nothing_signed(self.sign(value, **SECRETS), "REHEARSAL must be true or false")


class Rehearsal(Signing):
    def test_rehearsal_that_received_a_release_credential_refuses_to_run(self):
        for name, value in {**SECRETS, "KEYSTORE_PATH": "/synthetic/release.jks"}.items():
            with self.subTest(name):
                result = self.sign("true", **{name: value})
                self.assert_nothing_signed(result, f"{name} is set: a rehearsal must not receive release signing credentials")
                self.assertNotIn(value, result.stdout + result.stderr)
        self.assert_nothing_signed(self.sign("true", **SECRETS), "a rehearsal must not receive release signing credentials")

    def test_only_a_manual_run_may_sign_with_a_throwaway_key(self):
        for event in ("push", "schedule", "pull_request", ""):
            with self.subTest(event):
                self.assert_nothing_signed(self.sign("true", event=event), "refusing to sign with a throwaway key")

    def test_signing_material_is_refused_inside_the_checkout(self):
        inside = self.checkout / "target/temp"
        inside.mkdir(parents=True)
        for rehearsal, environment in (("true", {}), ("false", SECRETS)):
            with self.subTest(rehearsal=rehearsal):
                result = self.sign(rehearsal, RUNNER_TEMP=str(inside), **environment)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("RUNNER_TEMP is inside the checkout", result.stderr)
                self.assertFalse(self.record.exists())
                self.assertEqual(list(inside.iterdir()), [])

    def test_rehearsal_signs_with_a_key_made_for_the_run(self):
        require_jdk(self)
        result = self.sign("true", STUB_USE_KEY="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        record = self.recorded()
        # One PKCS12 store, outside the checkout, readable by its owner only.
        self.assertEqual(record["KEYSTORE_PATH"], str(self.temp / KEYSTORE_NAME))
        self.assertFalse(Path(record["KEYSTORE_PATH"]).is_relative_to(self.checkout))
        self.assertEqual(record["mode"], 0o600)
        self.assertEqual(record["temp"], [KEYSTORE_NAME])
        self.assertIsNone(record["KEYSTORE"])
        self.assertEqual(record["KEY_ALIAS"], "rehearsal")
        # A DER sequence, as PKCS12 is; keytool also opens a JKS file (magic FEEDFEED) when asked for PKCS12.
        self.assertTrue(record["bytes"].startswith("30"), record["bytes"][:8])
        listing = record["listing"]
        self.assertIn("Keystore type: PKCS12", listing)
        self.assertIn("Your keystore contains 1 entry", listing)
        self.assertIn("Alias name: rehearsal", listing)
        self.assertIn("Entry type: PrivateKeyEntry", listing)
        self.assertIn("Owner: CN=Patina release rehearsal, OU=Throwaway key, O=Not for distribution", listing)
        self.assertIn("Signature algorithm name: SHA256withRSA", listing)
        self.assertIn("2048-bit RSA key", listing)
        # jarsigner signs and verifies with it exactly as package-android.sh calls it.
        self.assertEqual(record["signed"][:2], [0, 0], record["signed"])
        self.assertIn("jar verified", record["signed"][2])

        # A random password, shared by store and key (PKCS12), masked before any use.
        password = record["STORE_PASSWORD"]
        self.assertRegex(password, r"\A[0-9a-f]{48}\Z")
        self.assertEqual(record["KEY_PASSWORD"], password)
        lines = result.stdout.splitlines()
        self.assertEqual(lines[0], f"::add-mask::{password}")
        self.assertEqual((result.stdout + result.stderr).count(password), 1)
        self.assertNotIn(password, listing)
        notice = [line for line in lines if line.startswith("::notice::")]
        self.assertEqual(len(notice), 1)
        self.assertRegex(notice[0], r"throwaway key \(certificate SHA-256 [0-9a-f]{64}\)\. They are not release artifacts\.")
        self.assertIn("synthetic package step ran", lines)

        # Nothing outlives the script.
        self.assertEqual(list(self.temp.iterdir()), [])

    def test_every_rehearsal_has_its_own_key_and_password(self):
        require_jdk(self)
        seen = []
        for _ in range(2):
            result = self.sign("true")
            self.assertEqual(result.returncode, 0, result.stderr)
            seen.append(self.recorded())
        self.assertNotEqual(seen[0]["STORE_PASSWORD"], seen[1]["STORE_PASSWORD"])
        self.assertNotEqual(seen[0]["bytes"], seen[1]["bytes"])

    def test_key_is_removed_when_signing_fails(self):
        require_jdk(self)
        result = self.sign("true", STUB_EXIT="7")
        self.assertEqual(result.returncode, 7)
        self.assertTrue(self.record.exists())
        self.assertEqual(list(self.temp.iterdir()), [])

    def test_a_key_left_by_an_earlier_attempt_is_not_reused(self):
        require_jdk(self)
        (self.temp / KEYSTORE_NAME).write_bytes(b"synthetic leftover")
        result = self.sign("true")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotEqual(bytes.fromhex(self.recorded()["bytes"]), b"synthetic leftover")
        self.assertEqual(list(self.temp.iterdir()), [])


def android_sdk():
    """(apksigner, aapt2, android.jar) of an installed SDK, or None. Linux only, as the signing job is:
    package-android.sh uses GNU tools."""
    home = os.environ.get("ANDROID_HOME")
    if not home or sys.platform != "linux":
        return None
    tools = Path(home) / "build-tools/35.0.1"
    found = (tools / "apksigner", tools / "aapt2", Path(home) / "platforms/android-35/android.jar")
    return found if all(path.is_file() for path in found) else None


class RealPackaging(Signing):
    """The real package-android.sh signs and verifies synthetic packages with the rehearsal key."""

    MANIFEST = ('<?xml version="1.0" encoding="utf-8"?>\n'
                '<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="invalid.synthetic.patina">\n'
                '  <uses-sdk android:minSdkVersion="26" android:targetSdkVersion="35"/>\n'
                '  <application android:label="Synthetic" android:hasCode="false"/>\n</manifest>\n')
    CONTENT = {"lib/arm64-v8a/libnsh.so": b"\x7fELFsynthetic", "assets/notices/THIRD-PARTY.html": b"synthetic",
               "assets/notices/components.json": b"{}", "assets/notices/NSH-RUNNERS.html": b"synthetic",
               "assets/notices/runner-components.json": b"{}", "assets/notices/gradle/index.json": b"{}"}

    def setUp(self):
        sdk = android_sdk()
        if not sdk or not JDK or not shutil.which("openssl"):
            self.skipTest("needs Linux, ANDROID_HOME with build-tools 35.0.1 and platform android-35, a JDK and openssl")
        super().setUp()
        _, aapt2, platform = sdk
        for name in ("package-android.sh", "check-android-artifacts.py"):
            shutil.copy(CI_DIRECTORY / name, self.checkout / "tools/ci" / name)
        unsigned = self.checkout / "target/android-unsigned"
        unsigned.mkdir(parents=True)
        manifest = self.home / "AndroidManifest.xml"
        manifest.write_text(self.MANIFEST)
        apk = unsigned / "app-release-unsigned.apk"
        subprocess.run([str(aapt2), "link", "-o", str(apk), "-I", str(platform), "--manifest", str(manifest)],
                       check=True, capture_output=True)
        with zipfile.ZipFile(apk, "a") as package:
            for name, data in self.CONTENT.items():
                package.writestr(name, data)
        with zipfile.ZipFile(unsigned / "app-release.aab", "w") as bundle:
            bundle.writestr("base/manifest/AndroidManifest.xml", self.MANIFEST)
            for name, data in self.CONTENT.items():
                bundle.writestr("base/" + name, data)
        (unsigned / "Patina-9.8.7-android-classpath-sbom.cdx.json").write_text("{}\n")

    def certificates(self):
        apksigner = android_sdk()[0]
        dist = self.checkout / "dist"
        apk = subprocess.run([str(apksigner), "verify", "--print-certs", str(dist / "Patina-9.8.7-android-tablet-tv.apk")],
                             check=True, capture_output=True, text=True).stdout
        aab = subprocess.run(["keytool", "-printcert", "-jarfile", str(dist / "Patina-9.8.7-android-tablet-tv.aab")],
                             check=True, capture_output=True, text=True).stdout
        return apk, aab

    def test_rehearsal_packages_are_signed_and_verified_with_the_throwaway_certificate(self):
        result = self.sign("true", ANDROID_HOME=os.environ["ANDROID_HOME"])
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(sorted(path.name for path in (self.checkout / "dist").iterdir()),
                         ["Patina-9.8.7-android-classpath-sbom.cdx.json", "Patina-9.8.7-android-tablet-tv.aab",
                          "Patina-9.8.7-android-tablet-tv.apk"])
        announced = [line for line in result.stdout.splitlines() if line.startswith("::notice::")][0]
        fingerprint = announced.split("certificate SHA-256 ")[1].split(")")[0]
        apk, aab = self.certificates()
        self.assertIn(f"Signer #1 certificate SHA-256 digest: {fingerprint}", apk)
        self.assertIn("CN=Patina release rehearsal, OU=Throwaway key, O=Not for distribution", apk)
        self.assertIn("Owner: CN=Patina release rehearsal, OU=Throwaway key, O=Not for distribution", aab)
        self.assertEqual(aab.replace(":", "").lower().count(fingerprint), 1)
        self.assertEqual(list(self.temp.iterdir()), [])

    def test_a_package_signed_with_another_key_fails_the_verification(self):
        """What package-android.sh checks for the rehearsal key is a real check."""
        real = (self.checkout / "tools/ci/package-android.sh").read_text()
        swapped = real.replace('cp "$unsigned/app-release.aab" "$aab"', 'cp "$unsigned/app-release.aab" "$aab"\n'
                               'keytool -genkeypair -noprompt -storetype PKCS12 -keystore "$RUNNER_TEMP/other.p12" -alias "$KEY_ALIAS" '
                               '-keyalg RSA -keysize 2048 -validity 2 -dname "CN=Synthetic other key" '
                               '-storepass:env STORE_PASSWORD -keypass:env KEY_PASSWORD\n'
                               'real_keystore=$KEYSTORE_PATH; KEYSTORE_PATH="$RUNNER_TEMP/other.p12"')
        swapped = swapped.replace('"$apksigner" verify "$apk"', 'KEYSTORE_PATH=$real_keystore\n"$apksigner" verify "$apk"')
        self.assertNotEqual(swapped, real)
        self.assertIn("real_keystore=$KEYSTORE_PATH", swapped)
        self.assertIn("KEYSTORE_PATH=$real_keystore", swapped)
        (self.checkout / "tools/ci/package-android.sh").write_text(swapped)
        result = self.sign("true", ANDROID_HOME=os.environ["ANDROID_HOME"])
        self.assertNotEqual(result.returncode, 0, result.stdout)
        # Both were signed and each verifies on its own; only the bundle's certificate is not the selected one.
        self.assertIn("jar verified", result.stdout)
        announced = [line for line in result.stdout.splitlines() if line.startswith("::notice::")][0]
        fingerprint = announced.split("certificate SHA-256 ")[1].split(")")[0]
        apk, aab = self.certificates()
        self.assertIn(f"Signer #1 certificate SHA-256 digest: {fingerprint}", apk)
        self.assertNotIn(fingerprint, aab.replace(":", "").lower())
        self.assertIn("Owner: CN=Synthetic other key", aab)
        (self.temp / "other.p12").unlink()
        self.assertEqual(list(self.temp.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
