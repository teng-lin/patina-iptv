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

## Fixture streams: ending or looping live

`tools/make-hls-fixtures.sh --android-smoke` encodes three 300 s channels
(`fixtures/hls/ch0`..`ch2`). `tools/mock-server.py` serves them in one of two
ways, and the playlist from `tools/make-iptv-fixtures.py` works with either,
since the channel URLs stay `/ch<N>/index.m3u8`:

| Variant | Start | What a channel does |
| --- | --- | --- |
| Ending (default) | `python3 tools/mock-server.py 8089 fixtures/hls &` | The generated VOD playlist, ending with `#EXT-X-ENDLIST`. A live channel on it ends after 300 s: the app's local live server answers 503 and the player reconnects. Use it to exercise that reconnect path; the CI smoke uses it. |
| Looping live | `python3 tools/mock-server.py --loop 8089 fixtures/hls &` | An endless live stream: a sliding playlist of the newest 30 segments (`--window`), without `#EXT-X-ENDLIST`, whose media sequence numbers rise with the clock as the 300 s of segments repeat (`live-<sequence>.ts`). Each repeat is served with its timestamps and continuity counters carried on, so the stream the app joins from the segments has one timeline at the wrap. It needs no `#EXT-X-DISCONTINUITY`, and it has none, because the app joins segments into one stream and would drop that tag. |

Use the looping live variant for any run longer than a channel's media (300 s
with `--android-smoke`, 30 s without), such as soak, timing, stream-request
counts or a held journey (`IPTV_SMOKE_HOLD`). With the ending variant those
runs see a reconnect each time the media ends, which adds noise
to timings, request counts and logs and can hide a real stop. With `--loop`
the server's log shows one `index.m3u8` reload about every second and one
rising `live-<sequence>.ts` per 2 s segment, with no reconnect. Catch-up
(`/archive/...`) still serves the ending VOD playlist in both variants.
Timestamps wrap at 2^33 ticks of 90 kHz (about 26.5 hours of server uptime),
as MPEG-TS allows, so a multi-day soak sees that wrap once a day.
`tools/ci/test-mock-server.py` checks both variants.

## Running the emulator

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
whole seconds from 1 through 1800 (enough for a 20-minute playback run such
as the looping live check of #178), and optionally `IPTV_SMOKE_HOLD_FILE` to a
path whose creation ends the hold early (the smoke removes an existing file when
the hold starts). Both the runner and the smoke refuse another value before any
device action; the runner adds the hold to the smoke's 1200 s budget. The smoke
waits after its last step, keeps the logs every 30 s, then still runs its crash
and refused-load checks. Address that emulator only by its `emulator-<port>`
serial; CI never sets the hold.

The existing smoke script restores changed device settings in its cleanup trap.
The disposable runner provides containment if that restoration is interrupted.
Direct smoke invocation requires an explicit emulator serial and the owned
`IPTV_SMOKE_DISPOSABLE_AVD` name. CI creates a fresh profile in a private
runner directory and reuses the hosted runner's SDK tools, installing the API 36
system image when needed. Local runs should use the copying runner to create an
owned profile.

These tools do not claim actual execution, shipping ARM playback, screen-reader
speech, or install-over-previous acceptance. Record each separately against the
artifact hash and device/OS/configuration. Tests in `test-disposable-avd.py` use
small generated image files; they verify path/ownership guards and interruption
cleanup without starting an emulator.

## Android casting platform test

The optional casting instrumentation test uses a fresh copy of the handheld
profile. It exercises the Android Keystore pairing store with synthetic
records, LAN identity fencing, the Wi-Fi multicast lock, and the foreground
service and media controls through Home, screen-off, and session replacement.
The test uses synthetic playback state and performs no receiver discovery or
connection.

After preparing the native validation library with `tools/sync-host.sh`,
build the app and test APKs together.

```sh
(cd hosts/android && IPTV_VALIDATION_APP_ID=com.hantekllc.iptvrs.validation \
  ./gradlew assembleDebug assembleDebugAndroidTest)
python3 tools/ci/disposable-avd.py "$HOME/.android/avd/tab36.avd" \
  hosts/android/app/build/outputs/apk/debug/app-debug.apk \
  --cast-platform-test hosts/android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
```

The runner verifies both APKs before launching the disposable profile. The
run directory records the two APK hashes and the instrumentation result. For
an app APK built with JaCoCo Android test coverage enabled, pass
`--cast-platform-coverage` to save the execution data in that run directory.

## Physical-device acceptance

Run the affected journey manually on a test tablet or TV with synthetic data.
Set `ABIS` to the device's ABI when running `tools/sync-host.sh` (`arm64-v8a`
or `armeabi-v7a` on ARM). Pass the mock server's LAN address and port with
`--host` when generating the playlist, and use the same address in
`IPTV_RS_DEV_PLAYLIST`. Follow the
[fixture build instructions](../tools/ci/android-smoke.sh) to build a debug APK
with the dedicated validation app ID. Set `serial` to the test device's serial
and install the APK manually:

```sh
adb -s "${serial:?}" install -r hosts/android/app/build/outputs/apk/debug/app-debug.apk
```

Record the device model, Android version, ABI, commit and APK hash along with
the journey and result.

### Signed release acceptance

Install the signed ARM APK on a dedicated test device. Serve the
[generated fixtures](#fixture-streams-ending-or-looping-live) from a
tester-controlled public HTTP endpoint backed by `tools/mock-server.py`.
Pass that endpoint's public hostname and port to `--host` when generating
the playlist so its guide, artwork and stream URLs use the same address.
The signed build requires publicly reachable stream addresses. Add the
printed playlist URL in Patina, run the journey and record the result
against the signed APK's hash.
