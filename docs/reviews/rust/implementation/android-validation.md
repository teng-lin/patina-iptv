# Android implementation validation

This record covers the workspace review corrections. Only generated playlists,
EPG and HLS media are used. Checks still marked pending have not passed.

## Isolation

The validation APK uses temporary Gradle application ID
`com.hantekllc.iptvrs.rustreview`, with the existing host namespace. Its Android
UID, private files, Keystore and file-provider authority are separate from the
household installation. The regular application package is never cleared or
uninstalled. The temporary smoke runner reads only the test process log since
each marker, does not clear global logs, and takes no physical-device
screenshots. Local fixtures are reached with `adb reverse`.

The generated fixture contains 300 channels, local HLS test patterns and
synthetic CJK channel/programme names. The local mock server runs on port 8089.

The final emulator runners use complete isolated AVD configurations under an
ignored build directory, selected with `ANDROID_AVD_HOME`; they do not write the
original AVD directories.

## Checks

- Android `aarch64-linux-android` library typecheck: passed in 1m 10s.
- Temporary host Java unit tests: 25 passed after copying the two original
  synthetic player-contract JSON fixtures to the relocated test root. The
  first run failed only because those relative fixture files were absent.
- Fresh x86_64 and arm64-v8a Rust libraries and debug APK: passed.
  The Chromecast requires a further fresh armeabi-v7a build (pending).
  Initial APK SHA-256:
  `7e5677d28592680617b4214011413af5992bea1e1fe55c039493de80be8fbc65`.
  This is a preliminary dirty-tree build based on `97dbc6b0`; final committed
  and rebased source still needs its own build and validation.
- Isolated tab36 API 36 smoke: passed, including first import, playback,
  multiview, background/resume, refresh during playback, persisted identity,
  kill/reopen, activity recreation and interrupted restore recovery.
- Isolated tv36 API 36 smoke: passed through the same journey, plus shared
  TV controls and long-OK release handling. No crash or refused relay load on
  either profile. Each observed two recoverable player errors during the
  generated 30-second fixture journey; these are recorded in the smoke logs.
- Physical Samsung SM-X210 tablet, API 36 smoke: blocked by closed hardware
  cover (`mLastSleepReason=cover_close`, `mWakefulness=Dozing`). The separate
  test app started and opened its store, but the cover prevented input/playback
  validation. No pass claimed; physical cover must be opened for rerun. The
  failed runner restored device settings.
- Physical Chromecast TV, API 34 smoke: pending new armeabi-v7a library.
  First install attempt correctly failed with `INSTALL_FAILED_NO_MATCHING_ABIS`;
  this device has 32-bit-only Android userspace.
- TalkBack, enlarged text and CJK checklist on both emulator profiles: pending.

## Validation setup incident

Before creating complete isolated AVD configurations, the first read-only
emulator launches used the existing tab36 and tv36 configurations. They booted
then lost ADB transport. A tab36 restart incorrectly combined `-wipe-data` with
explicit temporary `-data` and `-cache` paths and `-read-only`. The emulator
removed original tab36 images before failing the temporary filesystem capacity
check. The original tab36 userdata, cache and encryption-key images were lost;
the configuration remains. No still-open descriptor or accessible backup was
found. Original AVD state was not recreated or overwritten afterwards. Physical
applications and data were not involved. This is a validation setup mistake,
not an application result, and it must not be omitted from the handoff.
