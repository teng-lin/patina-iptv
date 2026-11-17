# Android emulator validation, November 16–17, 2026

Executed locally on November 17, approximately 04:23–04:35 UTC. Tablet smoke
passed; TV smoke passed on a bounded renderer-mode retry after its initial
first-video decode failure. This is emulator-only evidence;
no physical Android TV was contacted. Emulator audio was disabled throughout.

## Artifact and environment

- Application source: `422717536fa48dc323aff6e0c15b4c6ee2bfc102`.
- Execution manifest HEAD: `b9b89343dfac61bc3eed05ce58cba3d3d6c28e7a`.
  There was no `crates/` or `hosts/` diff between these revisions. Subsequent
  `PlayerContract.kt` and `RecordingHost.kt` edits only reformatted Kotlin;
  they are not represented as a new APK execution here.
- APK SHA-256:
  `43a399bd51a1149ef3ebc6dd4f8195efc2f8380b0aec899a7005e424ce3223da`.
- Dedicated debug application ID: `com.hantekllc.iptvrs.validation`; x86_64 only.
- NSH sibling checkout uses the repository's `9ed782cb` pin. Packaging used the
  existing NSH CLI and Android runtime assets through explicit `NSH` and
  `NSH_RUNTIME` paths, with an isolated `target/android-build` Cargo target.
- Incremental `tools/sync-host.sh` passed: crate build 10.2 seconds, generated
  Android library build 1 minute 17 seconds, APK packaging 1.1 seconds.
  Gradle `assembleDebug` passed in 12 seconds. Its omitted ARM-library notices
  reflect the intentional x86_64 validation build, not shipping ARM validation.
- Android Emulator 37.2.13.0, build 16740729; API 36 x86_64 images:
  Google APIs revision 7 (tablet) and Android TV revision 4 (TV).
- Generated fixtures: 300 channels, 24 groups and three 720p HLS streams;
  original segments repeated 120 times using `repeat-hls-fixtures.py`. Only the
  owned mock server on port 8089 supplied content. Release debug assertions were
  enabled for these local synthetic URLs; this is not a production APK.

## Results

| Check | Tablet copy | Initial TV copy |
| --- | --- | --- |
| Cold boot with fresh userdata, sound disabled | Pass, 34.5 seconds | Pass, 28.9 seconds |
| App startup and expected form factor | Pass; tablet, PiP supported | Pass; TV, PiP unsupported |
| Import synthetic playlist and guide | Pass, 300 channels | Pass, 300 channels |
| Guide video playback | Pass | Fail: `ERROR_CODE_DECODING_FAILED` |
| Fullscreen and two-player multiview | Pass | Not reached |
| Background/resume | Pass | Not reached |
| Refresh while playback stays open | Pass | Not reached |
| Kill/reopen after selection change | Pass | Not reached |
| Activity recreation | Pass | Not reached |
| Interrupted restore recovery | Pass | Not reached |
| Final no-crash/no-refused-load gate | Pass; zero player errors | Not reached |

The TV found one video and one audio track, then reported
`ERROR_CODE_DECODING_FAILED`; the smoke guard failed because player-ready did not
arrive within 60 seconds. Its emulator log also contained a missing ColorBuffer
and invalid-texture error. Their causal relationship is not established by the
captured logs. The initial run does not establish TV playback acceptance. The
tablet passed with the identical APK and media.

The TV retry at approximately 04:32–04:35 UTC used the **same APK and media** with
`--gpu swiftshader`, a mode listed by the installed emulator's own `-help-gpu`.
Every smoke phase passed, including TV long-OK controls, two-player multiview,
background/resume, refresh, process reopen, activity recreation and interrupted
restore recovery. The final gate found no crash, refused load or player errors.
Captured decoder names were `c2.goldfish.h264.decoder` and
`c2.android.aac.decoder`. The new `codec.log` retains filtered codec/ExoPlayer
diagnostics before each buffer clear. Smoke assertions were not weakened.

The successful mode change clears the observed blocker for this configuration;
one comparison does not establish deterministic GPU causality. The original
failure remains recorded above. Local and hosted smoke now default to the
supported `swiftshader` mode, with an explicit local `--gpu` diagnostic override.

## Isolation and cleanup

Each source AVD was copied to a unique, marker-owned directory, with qcow2 backing
chains flattened before boot. The runner revalidated the ownership token and AVD
name before passing `-wipe-data` to the **copy**; copied applications/accounts
could not autostart. All emulator home paths were private. Both runs used
`-no-audio`, `-no-snapshot`, `-no-window` and `swiftshader_indirect`.
The passing TV retry used `swiftshader` instead of `swiftshader_indirect`.
ADB operations selected only the verified emulator serials 5580 and 5582.

After completion, all owned emulator process groups, their copied profiles and the
owned fixture server were stopped/removed. The original `tab36` and `tv36` file
sets, sizes and modification timestamps matched their pre-run inventory; this
metadata check is not claimed as a byte-for-byte hash of every original image.
No disposable roots remained. The fresh-data guard's 12 Python regression tests,
Ruff and whitespace validation passed before execution.

## Retained local evidence

These generated artifacts are retained in the ignored build tree, not committed
as application source or mistaken for a hosted CI run:

- [Tablet manifest](../../../target/android-validation/tab36/run-_pata0fk/operation.json),
  [driver log](../../../target/android-validation/tab36/run-_pata0fk/driver.log),
  [emulator log](../../../target/android-validation/tab36/run-_pata0fk/emulator.log)
  and the adjacent `smoke/` screenshots/app logs.
- [TV manifest](../../../target/android-validation/tv36/run-uc5sl2u8/operation.json),
  [driver log](../../../target/android-validation/tv36/run-uc5sl2u8/driver.log),
  [emulator log](../../../target/android-validation/tv36/run-uc5sl2u8/emulator.log)
  and the adjacent `smoke/` failure screenshot/app logs.
- [Build/sync log](../../../target/android-validation/android-final-sync.log),
  [Gradle log](../../../target/android-validation/android-final-gradle.log) and
  [result receipt](../../../target/android-validation/android-validation-result.json).
- [Passing TV retry manifest](../../../target/android-validation/tv36-swiftshader/run-mc5tb5wi/operation.json),
  [driver log](../../../target/android-validation/tv36-swiftshader/run-mc5tb5wi/driver.log)
  and adjacent `smoke/codec.log`, app logs and screenshots.

No shipping ARM, install-over-previous, screen-reader speech or manual visual
acceptance is inferred from these automated emulator checks.
