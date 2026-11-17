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

After testing, the physical test packages and the temporary port-8089 reverse
mapping were removed. The normal physical application package was never cleared
or uninstalled. The tablet test package was also removed while awaiting the
closed cover; it can be reinstalled from the retained final APK for rerun.

The final emulator runners use complete isolated AVD configurations under an
ignored build directory, selected with `ANDROID_AVD_HOME`; they do not write the
original AVD directories.

## Checks

- Initial Android `aarch64-linux-android` typecheck: passed in 1m 10s.
- Temporary host Java unit tests: 25 passed after copying the two original
  synthetic player-contract JSON fixtures to the relocated test root. The
  first run failed only because those relative fixture files were absent.
- Final fresh x86_64, arm64-v8a and armeabi-v7a Rust libraries and debug
  APK: passed. Production source is `f88f506`; all three native libraries
  contain About/build hash `5ffa9a5f`. Later test/documentation commits do not
  change these production bytes. Final explicit Android typecheck passed in
  6.11s and Gradle assembly in 11s.
  Final APK SHA-256:
  `a4c97965c5b4df8b0f280c9a635dbec3d93f15afc533f85bdf7525b675151f57`.
  `aapt` verified the separate application ID and all three expected ABIs.
- Final APK, isolated tab36 API 36 smoke: passed, including first import,
  playback,
  multiview, background/resume, refresh during playback, persisted identity,
  kill/reopen, activity recreation and interrupted restore recovery.
- Final APK, isolated tv36 API 36 smoke: passed through the same journey,
  plus shared
  TV controls and long-OK release handling. No crash or refused relay load on
  either profile. Each observed two recoverable player errors during the
  generated 30-second fixture journey; these are recorded in the smoke logs.
- Physical Samsung SM-X210 tablet, API 36 smoke: blocked by closed hardware
  cover (`mLastSleepReason=cover_close`, `mWakefulness=Dozing`). A preliminary
  test APK started and opened its separate store, but the cover prevented
  input/playback validation. The final APK has not run on this physical tablet.
  No pass claimed; the cover must be opened for rerun. The failed runner restored
  device settings.
- Physical Chromecast TV, API 34: fresh armeabi-v7a library built and the
  final APK installed. The complete long-fixture smoke passed, including
  both players resuming, persisted refresh identity, kill/reopen, activity
  recreation and interrupted restore recovery. No crash or refused relay load;
  two network errors recovered. First two smoke attempts are retained as
  failures (details below).
- Additional native media Stop check on both final-APK emulators and the
  physical Chromecast TV: passed.
  A ready native player acknowledged Stop and did not reopen during the
  following three seconds.
- CJK guide and settings captures on both profiles: visually inspected for
  `zh`, `zh-Hant`, `ja` and `ko`; generated CJK names/titles render without
  missing-glyph boxes. Existing programme ellipsis remains present.
- Extra-large Patina font: guide and settings inspected on both profiles;
  labels expand and long names ellipsize. Broader organisation/custom-group/
  guide-assignment checks remain pending. Android system font scale 2.0 was
  also applied on both profiles: it visibly enlarges Patina text further,
  contradicting the previous documentation parenthetical. General-to-About
  D-pad navigation remained visible/usable; help and long page names ellipsize.
  System font scale was restored to 1.0 afterwards.
- TalkBack 16.0.0.738667889 bound on both profiles with touch exploration.
  The TV image had no bundled TalkBack; the identical APK bundled in tab36
  was installed only in the isolated TV emulator. With TalkBack active,
  tablet touch changed a switch and the accessibility tree reflected its
  checkable/checked state and full programme text; TV D-pad moved app selection.
  Actual spoken announcements, notice repetition and comprehensive focus return
  are unverified. The visible TalkBack focus outline currently spans the whole
  app window on both profiles; this does not establish correct item speech.
  TalkBack was disabled and enabled-service state restored afterwards.

Final APK, build logs, failed and successful smoke logs, original 30-second
playlists and screenshot evidence are retained locally under
`/home/blackmyth/src/patina-iptv/target/rust-review-android-validation/`.
The APK is `Patina-Review-5ffa9a5f.apk`. Screenshots are under `screenshots/`,
with `tab36/` and `tv36/` subdirectories and `<locale>-guide.png`,
`<locale>-settings.png` and `extra-large-*.png` names. Initial Android font scale
was 1.0 on both profiles. The four-language and extra-large captures were
repeated on the final APK. Both isolated VMs were exited before the root's
strict timing checks.

A pre-existing layout issue remains visible on tv36: the navigation rail's
Settings row is partly clipped at the bottom at 1920×1080, while D-pad selection
still opens Settings. `rail.rs` is unchanged by this PR. No complete
accessibility checklist pass is claimed.

## Physical TV failures retained for diagnosis

The TV uses 32-bit Android userspace. The first install attempt with only
x86_64/arm64 libraries failed with `INSTALL_FAILED_NO_MATCHING_ABIS`; a fresh
armeabi-v7a build corrected that environment mismatch.

The first installed final-APK smoke passed import, actual 1280×720 playback,
long OK and shared-bar Right navigation, then failed because it required a
`key=back ... ctx=Controls` log. The activity remained resumed/visible. A fresh
run queued Right, Back, MediaPause and MediaPlay together, before polling or
bar auto-hide. Right was reported in Controls and both following media keys
in Player, demonstrating that Back closed the bar. API 34's direct back
callback is a plausible reason for the missing key log; the callback path was
not instrumented. The temporary runner uses this balanced behavior check.

That second run reached two-player multiview, then failed its background/resume
check: surface 0 became a fatal `ERROR_CODE_PARSING_MANIFEST_MALFORMED` roughly
31 seconds after its first ready event, while surface 1 resumed. The generated
media were only 30 seconds long and the slower TV journey exceeded that before
backgrounding. No production cause or fix is claimed from this evidence.
Original failed logs and original 30-second playlists are retained. The next
run uses 3600-second synthetic HLS playlists which repeat the same generated
segments with explicit `EXT-X-DISCONTINUITY` markers; there is no encoding or
fixture mutation during that run. The complete long-fixture run passed,
including both-player resume, refresh, process/activity recreation and restore
recovery. This supports the finite-clip explanation without establishing an
exact root cause for the retained 30-second failure.

## Validation setup incident

Before creating complete isolated AVD configurations, the first read-only
emulator launches used the existing tab36 and tv36 configurations. They booted
then lost ADB transport. A tab36 restart incorrectly combined `-wipe-data` with
explicit temporary `-data` and `-cache` paths and `-read-only`. The emulator
removed original tab36 images before failing the temporary filesystem capacity
check. The original tab36 `userdata-qemu.img`, `cache.img` and `encryptionkey.img`
were lost; its `config.ini` remains unchanged. No still-open descriptor or accessible backup was
found. Original AVD state was not recreated or overwritten afterwards. Physical
applications and data were not involved. This is a validation setup mistake,
not an application result, and it must not be omitted from the handoff.
