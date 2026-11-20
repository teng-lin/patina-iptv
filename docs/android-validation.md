# Disposable Android validation

Use only synthetic fixtures, source AVDs created for synthetic testing and a
dedicated validation APK. Do not clone a personal profile containing accounts or
provider credentials; create a fresh test profile instead. Never reset tab36/tv36,
remove their userdata/cache/encryption images, or clear an installed viewer profile.
The validation application ID is `com.hantekllc.iptvrs.validation`; release and
ordinary debug builds keep the existing ID. The validation ID is permitted only as
the fixed debug suffix, never an arbitrary value or a release-package override.

Build the fixture APK and start the local mock server as described in
`tools/ci/android-smoke.sh`. Set `IPTV_VALIDATION_APP_ID` to the validation ID for
Gradle's `assembleDebug`. The smoke helper refuses a normal app APK before install.
Ensure `adb`, `emulator` and `apkanalyzer` are on PATH from the Android SDK.

Run one form at a time, with a source profile that is not being modified:

```sh
python3 tools/ci/disposable-avd.py "$HOME/.android/avd/tab36.avd" \
  hosts/android/app/build/outputs/apk/debug/app-debug.apk --port 5580
python3 tools/ci/disposable-avd.py "$HOME/.android/avd/tv36.avd" \
  hosts/android/app/build/outputs/apk/debug/app-debug.apk --port 5582
```

The runner copies the complete AVD into `target/disposable-avds/patina-avd-*`,
including userdata, cache and encryption images. Emulator sound is always disabled
with `-no-audio`; this runner never targets physical devices. Rendering defaults to
the supported `swiftshader` software mode. `--gpu` records an alternate mode for
bounded diagnostics without changing smoke assertions. It rewrites copied writable paths,
creates a unique AVD name and isolates the Android emulator directories. It
resets the verified private copy with `-wipe-data` before boot so copied accounts
and apps cannot start; it never resets the source, deletes source locks or edits
the source profile. It refuses
symlink roots, external writable-image paths and scratch directories inside source
or Android profile roots. All adb operations specify the selected emulator serial;
the runner and smoke script verify the serial's AVD name before device mutations.

On exit or interruption, the runner stops/reaps only its own process groups and
removes only the copied profile whose private ownership marker matches its scratch
root and token. Copied qcow2 images are inspected with `qemu-img` and converted
to standalone images before boot; their backing chain may only read the copied or
original profile, or read-only SDK system-image files. Conversion never writes to
the source/backing image. Unknown backing paths or a missing qemu-img tool fail
before emulator startup. This prevents absolute qcow2 backing paths from escaping
a simple directory copy. It retains logs, screenshots and `operation.json` under a unique
`target/android-validation/run-*` directory, including commit, APK SHA-256, package,
source/copy identity, serial and result. An incomplete record is not a passing run.
An uncatchable process/machine termination can leave a copy behind: inspect it and
its marker manually; never substitute the original profile path into cleanup.

For manual journey checks on the same owned emulator, set `IPTV_SMOKE_HOLD` to
a number of seconds (and optionally `IPTV_SMOKE_HOLD_FILE` to a path whose
creation ends the hold early). The smoke waits after its last step, then still
runs its crash and refused-load checks. Address that emulator only by its
`emulator-<port>` serial; CI never sets the hold.

The existing smoke script restores changed device settings in its cleanup trap.
The disposable runner provides containment if that restoration is interrupted.
Direct smoke invocation requires an explicit emulator serial and the owned
`IPTV_SMOKE_DISPOSABLE_AVD` name. CI creates a fresh `patina-validation-ci` profile;
local runs should use the copying runner instead of declaring existing profiles
owned. Physical-device acceptance is separate and must not use this data-clearing
emulator harness.

These tools do not claim actual execution, shipping ARM playback, screen-reader
speech, or install-over-previous acceptance. Record each separately against the
artifact hash and device/OS/configuration. Tests in `test-disposable-avd.py` use
small generated image files; they verify path/ownership guards and interruption
cleanup without starting an emulator.
