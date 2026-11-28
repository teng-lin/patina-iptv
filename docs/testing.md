# Testing

Manual and measured validation for Patina: fixtures, Android and iOS runs,
accessibility journeys and performance journeys. [CI](ci.md) describes the
automated lanes.

## Contents

- [Synthetic data and fixtures](#synthetic-data-and-fixtures):
  [ending or looping live](#fixture-streams-ending-or-looping-live)
- [Disposable Android validation](#disposable-android-validation):
  [emulator](#running-the-emulator),
  [casting](#android-casting-platform-test),
  [physical devices](#physical-device-acceptance),
  [signed release](#signed-release-acceptance)
- [iOS simulator validation](#ios-simulator-validation):
  [entitlements](#entitlements), [build](#build-install-and-launch),
  [playlist](#synthetic-playlist), [AirPlay](#native-airplay-in-patina)
- [Accessibility journeys](#accessibility-journeys):
  [matrix](#repeatable-matrix), [TalkBack](#android-talkback-run),
  [desktop](#desktop-keyboard-and-platform-screen-reader),
  [results](#recording-results)
- [Reproducible performance journeys](#reproducible-performance-journeys):
  [workloads](#workloads-and-endpoints), [CI policy](#report-and-ci-policy),
  [Android and TV](#android-evidence-and-opt-in-physical-tv-runs)
- [Open device checks](#open-device-checks)

## Synthetic data and fixtures

Every run on this page uses only synthetic data: generated playlists, XMLTV,
provider catalogs and local mock streams
([synthetic setup](../README.md#try-it-with-synthetic-data)). Never run a
journey against a real-provider profile or an existing household profile.
Prepare synthetic profiles outside personal app storage.

### Fixture streams: ending or looping live

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

## Disposable Android validation

Use source AVDs created for synthetic testing and a dedicated validation APK. Do
not clone a personal profile containing accounts or provider credentials; create a fresh test profile instead. Never reset tab36/tv36,
remove their userdata/cache/encryption images, or clear an installed viewer profile.
The validation application ID is `com.hantekllc.iptvrs.validation`; release and
ordinary debug builds keep the existing ID. The validation ID is permitted only as
the fixed debug suffix, never an arbitrary value or a release-package override.

Build the fixture APK and start the local mock server as described in
`tools/ci/android-smoke.sh`. Set `IPTV_VALIDATION_APP_ID` to the validation ID for
Gradle's `assembleDebug`. The smoke helper refuses a normal app APK before install.
Ensure `adb`, `emulator` and `apkanalyzer` are on PATH from the Android SDK.

### Running the emulator

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
whole seconds from 1 through 1800 (enough for a 20-minute playback run), and
optionally `IPTV_SMOKE_HOLD_FILE` to a path whose creation ends the hold early (the smoke removes an existing file when
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

To launch an unmodified, non-debuggable production-ID APK on an owned copy, pass
`--release`. Record the artifact's own source with `--artifact-commit` and
`--artifact-nsh` (full SHAs), which can differ from the checkout:

```sh
python3 tools/ci/disposable-avd.py "$HOME/.android/avd/tab36.avd" \
  "${apk:?}" --release --port 5580 \
  --artifact-commit "${commit:?}" --artifact-nsh "${nsh:?}"
```

It installs the APK, resolves the tablet or Leanback launcher and waits for normal
startup, without touching private app storage or development fixture hooks. A
`launch-passed-journeys-pending` result leaves import, playback, picture in
picture, backup/restore and recording to be driven through the visible UI while
`IPTV_SMOKE_HOLD` keeps the emulator running.

These tools do not claim actual execution, shipping ARM playback, screen-reader
speech, or install-over-previous acceptance. Record each separately against the
artifact hash and device/OS/configuration. Tests in `test-disposable-avd.py` use
small generated image files; they verify path/ownership guards and interruption
cleanup without starting an emulator.

### Android casting platform test

The optional casting instrumentation test uses a fresh copy of the handheld
profile. It exercises the Android Keystore pairing store with synthetic
records, LAN identity fencing, and the Wi-Fi multicast lock. It checks the
foreground service lease and the controls its media session advertises
through Home, screen-off, and session replacement. The test supplies
synthetic playback state at the Kotlin host boundary. Delivery of media
commands to the Rust sender and playback on a receiver are outside its scope.

After preparing the native validation library with `tools/sync-host.sh`,
build the app and test APKs together. Gradle creates the debug instrumentation
variant only for the validation application ID. The Python disposable profile
runner supplies the owned-run token required by the instrumentation test.

```sh
(cd hosts/android && IPTV_VALIDATION_APP_ID=com.hantekllc.iptvrs.validation \
  ./gradlew assembleDebug assembleDebugAndroidTest)
python3 tools/ci/disposable-avd.py "$HOME/.android/avd/tab36.avd" \
  hosts/android/app/build/outputs/apk/debug/app-debug.apk \
  --cast-platform-test hosts/android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
```

The runner snapshots and verifies both APKs before launching the disposable
profile, then installs those copies. The run directory records the two APK
hashes and the instrumentation result. For an app APK built with JaCoCo
Android test coverage enabled, pass
`--cast-platform-coverage` to save the execution data in that run directory.

### Physical-device acceptance

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

#### Signed release acceptance

Install the signed ARM APK on a dedicated test device. Serve the
[generated fixtures](#fixture-streams-ending-or-looping-live) from a
tester-controlled public HTTP endpoint backed by `tools/mock-server.py`.
Pass that endpoint's public hostname and port to `--host` when generating
the playlist so its guide, artwork and stream URLs use the same address.
The signed build requires publicly reachable stream addresses; do not weaken
that transport restriction to obtain a pass. Add the
printed playlist URL in Patina, run the journey and record the result
against the signed APK's hash.

## iOS simulator validation

Use a simulator created for the run. Do not reuse a
simulator that holds accounts, provider data or another run's state, and never
erase or delete a simulator you did not create.

### Entitlements

The simulator reads an app's entitlements from its executable's
`__TEXT,__entitlements` section, where Xcode's linker puts them, not from the
code signature. `nsh package --target ios-simulator` signs the app ad hoc and
links no such section, and an ad hoc signature carrying `keychain-access-groups`
does not launch (POSIX error 163). Without the entitlements the simulator's
Keychain refuses every item: the log shows `platform credential broker is locked
or unavailable` and adding a playlist fails with "The server refused access"
before any request reaches the server.

So `iptv-app` links them itself on the simulator only
([`simulator_entitlements.rs`](../crates/iptv-app/src/host/native/simulator_entitlements.rs)):
`build.rs` writes `application-identifier` and `keychain-access-groups`, both the
`id` in [`nsh.json`](../crates/iptv-app/nsh.json), and a `#[used]` static puts
them in the section. Device builds are unaffected; they take their entitlements
from the provisioning profile when signed ([release](release.md)). No linker
flag or re-signing is needed. Check a package with:

```sh
tools/check-ios-simulator-entitlements.sh target/ios-sim/Patina.app
```

The iOS CI lane checks that the compiled simulator library (`libiptv_app.rlib`)
carries the section ([validation tiers](ci.md)). It links no app, so checking
the packaged app is a manual step: run the check above after packaging, as in
the next section.

### Build, install and launch

On macOS with Xcode, the pinned NSH CLI built from `../nsh-patina`
([NSH pin](development.md#nsh-pin)) and the `aarch64-apple-ios-sim` Rust target:

```sh
root=$PWD
bash tools/fetch-mpvkit.sh
export IPTV_RS_MPVKIT="$root/target/mpvkit"
source tools/ci/locked-build-env.sh
NSH="$root/../nsh-patina/target/release/nsh"
(cd crates/iptv-app && "$NSH" package . --target ios-simulator \
  --icon icon-ios.png -o "$root/target/ios-sim")
tools/check-ios-simulator-entitlements.sh target/ios-sim/Patina.app

udid=$(xcrun simctl create "Patina Synthetic" \
  com.apple.CoreSimulator.SimDeviceType.iPad-Air-11-inch-M4 \
  com.apple.CoreSimulator.SimRuntime.iOS-27-2)
xcrun simctl boot "$udid"
xcrun simctl install "$udid" target/ios-sim/Patina.app
xcrun simctl launch "$udid" com.hantekllc.iptvrs
```

For a run with the synthetic playlist below, export its fixture variables
before `nsh package`. Pick the device type and runtime from `xcrun simctl list devicetypes` and
`xcrun simctl list runtimes`. When other runs share the Mac, give the simulator
and the mock server a name and port of your own.

### Synthetic playlist

The simulator reaches the Mac at `127.0.0.1`:

```sh
tools/make-hls-fixtures.sh
python3 -I tools/make-iptv-fixtures.py --channels 200 --host 127.0.0.1:8089
tools/mock-server.py 8089 fixtures/hls &
```

That serves [ending channels](#fixture-streams-ending-or-looping-live): each
ends, and the app reconnects. For a run longer than a channel's media, start the
server with `--loop`.

`nsh package` builds the release profile, which refuses playlists whose
streams are on a private host such as the mock server ("content stream URL
must name a public HTTP(S) host"). For a fixture run, package with the
development fixture hook ([security model](security.md)), which only a build
with debug assertions honors. It is compiled in, so export these before
packaging, then install and launch that package:

```sh
export CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true \
  IPTV_RS_DEV_PLAYLIST=http://127.0.0.1:8089/iptv/playlist.m3u
```

Add `http://127.0.0.1:8089/iptv/playlist.m3u` as a playlist link in the app.
It loads the 200 channels, and the mock server's log shows the request. The
app's own log (`xcrun simctl spawn "$udid" log stream --predicate
'process == "Patina"'`) must not report the credential broker. Even the
release package, without the hook, must reach the mock server: it then
reports that the playlist cannot be read, not that the server refused access.

Afterwards stop the mock server and remove the simulator you created:
`xcrun simctl shutdown "$udid" && xcrun simctl delete "$udid"`.

### Native AirPlay in Patina

The normal Patina live player's **Cast to TV** control opens Apple's named-device
picker directly on iPad. No protocol selection is needed. The Apple video route picker belongs to the same runtime as
the player. Choose only the physical receiver approved for the run and let its
owner handle any approval. Do not use screen mirroring. The separate developer
[probe](../tools/apple-airplay-probe/README.md) remains a diagnostic control; it is not the product acceptance journey.

Use a disposable app identity and matching development profile for physical
fixture tests so the normal Patina app and its Keychain group remain separate.
Run the normal `App` root with no probe flag or test-support feature. A package
using the documented private-fixture hook is a development fixture build; it
does not validate production URL admission policy. Keep all media generated.

Start a generated live HLS channel in Patina, choose a receiver, and verify video
on the approved receiver. Check pause/resume with the play bar, changing to another generated channel
while retaining the route, and choosing this iPad in the same device picker to
return playback locally. Global Stop must retire the remote stream too.
The local player must not retain a source while remote playback owns the
one-stream account. Cancel receiver approval and revoke playback authorization
to check relay cleanup. Background and lock the iPad during a separately
approved session to validate continuity and return controls. Record receiver
observations separately from Patina's remote status; compilation and the
probe's success do not establish full product acceptance. Stop the fixture
origin and the probe or product session after the run.

## Accessibility journeys

Patina's automated checks inspect native names, roles, selection/checked state,
modal exposure, logical navigation, measured layout and player lifetime. They do
not establish that a platform screen reader speaks, moves or activates correctly.
Keep those results separate from the assistive-technology runs below.

### Repeatable matrix

Test with the app's Small, Medium, Large and Extra large text settings; repeat
the largest setting with system text at 1.3. Include long generated source/channel/program names
and a translated interface. Use 960×540 logical points for TV/small desktop and
1280×800 for tablet. A clipped program title may use ellipsis, but its accessible
name must retain the full title. Actions must remain visible or scroll into view.

| Journey | Expected behavior | Deterministic coverage |
| --- | --- | --- |
| First source setup | Source-type choices and fields have useful names; errors are readable; Back leaves the editor before the wizard | `screens_look`, `cov_settings_wizard`, `interface_languages` |
| Guide, groups and list changes | One selected content identity; D-pad follows the guide; sorting/reordering/refresh does not restore by old row number | `tablet_rows`, `guide_nav`, `personalization_order`, `accessibility_visibility` |
| Player and track chooser | Controls/choices expose roles and selected state; inactive controls and covered video are absent from accessibility | `tv_player`, `tv_replay`, `accessibility_visibility` |
| Parental PIN and recovery | Keypad, recovery and manager use their own focus ranges; Cancel remains visible; disabled controls cannot activate while checking; credential results remain private across a background-recording window hide and require a fresh PIN before redisplay | `parental`, `accessibility_visibility`, presentation `parental::dialog_tests` |
| VOD details, seasons and episodes | Catalog and episode selection expose content identity; Back and resume retain the intended item | `vod_library` |
| Nested modal dismissal | Covered controls cannot receive accessibility focus; Back exposes only the next layer and restores content selection | `accessibility_visibility` |
| Video contrast and lifetime | Opaque focus/text colors stay readable over black, white and bright video; UI transitions keep the same player and stream | theme and grid-cell unit tests; `accessibility_visibility` |

Run the affected suites while iterating:

```sh
cargo test -p iptv-core -p iptv-presentation parental
cargo test -p iptv-app --features test-support --lib
cargo test -p iptv-app --features test-support --test app accessibility_visibility
cargo test -p iptv-app --features test-support --test app parental
cargo test -p iptv-app --features test-support --test app vod_library
cargo test -p iptv-app --features test-support --test app recordings
```

The complete app suite in `tools/check.sh` includes these tests in Linux/macOS CI
and the Windows check. Before finishing also run the required core tests, portable
wasm build and native app build from [AGENTS.md](../AGENTS.md).

### Android TalkBack run

Use a disposable `tab36` or `tv36` emulator/profile
([running the emulator](#running-the-emulator)). Install the candidate APK and
synthetic sources. Record Android version, app commit, emulator versus hardware, resolution, text setting, system font scale,
input device, TalkBack version and enabled service. Package presence alone is not
an enabled screen-reader run.

1. Enable TalkBack in Android Accessibility settings. Verify it is active using
   `adb -s SERIAL shell dumpsys accessibility`; keep all adb commands scoped to
   that serial. For a disposable emulator only, the service can also be enabled
   with `settings put secure enabled_accessibility_services` using the exact
   installed service name, followed by `settings put secure accessibility_enabled 1`.
2. Run each matrix row using swipe navigation/double tap on tablet, then D-pad
   arrows/OK/Back on TV. Check spoken names, roles and current states; activate
   the controls through TalkBack, not just by screen coordinates. Record speech
   checks as unverified if nobody listened to the output.
3. Open each modal while video plays. Traverse all controls; hidden/covered
   controls must not be reachable. Close nested choosers/PIN/settings with Back
   and verify the origin channel/program rather than a row number.
4. Set Extra large and system font scale 1.3; repeat PIN/recovery, source setup,
   Record now and episode selection. Reach every primary action by D-pad and
   touch, including after scrolling up again. Repeat over synthetic black/white
   and bright streams. Observe that playback does not restart on modal changes.
5. Record failures with synthetic labels, the exact transition and expected versus
   observed behavior. Restore the emulator settings or discard the profile.

Use `uiautomator dump --compressed` when checking modal exposure: the default
uncompressed dump intentionally includes nodes marked unimportant for accessibility.
For a reproducible native focus/activation check while TalkBack remains enabled,
build the [shell probe](../tools/ci/android/AccessibilityActionProbe.java). Set
`serial` to the serial of the emulator under test:

```sh
IPTV_A11Y_OUT=/var/tmp/patina-accessibility
mkdir -p "$IPTV_A11Y_OUT"
javac --release 11 -cp "$ANDROID_HOME/platforms/android-36/android.jar" \
  -d "$IPTV_A11Y_OUT" tools/ci/android/AccessibilityActionProbe.java
"$ANDROID_HOME/build-tools/35.0.1/d8" --output "$IPTV_A11Y_OUT" \
  "$IPTV_A11Y_OUT/AccessibilityActionProbe.class"
adb -s "${serial:?}" push "$IPTV_A11Y_OUT/classes.dex" /data/local/tmp/patina-accessibility.dex
adb -s "${serial:?}" shell \
  "CLASSPATH=/data/local/tmp/patina-accessibility.dex app_process /system/bin AccessibilityActionProbe 'Add playlist'"
```

The label must name a control in a synthetic test profile. The probe requests
native accessibility focus and ACTION_CLICK without suppressing the active
screen-reader service. It is an Android shell test tool, not shipped app code;
check success against the resulting UI as well as its return value.

An accessibility tree dump checks native semantics, not speech output or touch
exploration. A headless `screen_reader=true` environment checks application
behavior only. Physical TV/tablet results must be identified separately from AVDs.

### Desktop keyboard and platform screen reader

On each available desktop, run setup, guide, player, settings, PIN, recordings and
VOD with keyboard only. Check Tab/Shift+Tab where supported, arrows/Enter, Escape
and Backspace without stealing text-editing keys. Resize to the small viewport,
change text size, and verify focus remains visible and actions remain reachable.
Repeat with the OS screen reader (VoiceOver on macOS, Narrator on Windows, Orca
on Linux where supported). Verify names/states, modal boundaries and activation.
Do not infer desktop screen-reader success from a passing Android or headless run.

### Recording results

For each candidate, record the command/suite and result separately from manual
AT observations, with the pull request or issue rather than in this file.
Include skipped matrix rows and the reason. This section is a procedure, not a
blanket accessibility certification; untested host integrations and unobserved
speech remain open validation limits.

## Reproducible performance journeys

### Running the journeys

Run from a clean committed source tree with the pinned NSH checkout and native dependencies installed:

```sh
python3 tools/performance/journeys.py run
```

The canonical command runs on Linux with Xvfb, software GL and ffmpeg available.
It builds the debug `test-support` app and native GTK harnesses once, copies them
to a private temporary directory, and runs each scenario three times, sequentially.
Every scenario creates a fresh isolated synthetic HOME and removes it afterward.
`target/performance/summary.json` is written before the build and after every
sample, including failed samples. Each sample attests the project's compile-time eight-character Git hash
(`build_version.rs` explicitly slices the full hash) and build mode, so a stale supplied harness cannot be labeled with the checkout's current commit. `--output`, `--repetitions`, `--timeout` (seconds,
300 by default), `--harness /absolute/path/to/app-tests` and `--native-harness` customize a run;
CI passes the already-built executables with `--harness-manifest`.
Do not run another timing workload alongside it. The timeout is an evidence
completeness deadline, not a performance budget. The base-compared fixture policy
requires at least three repetitions and a deadline no longer than 300 seconds;
short diagnostic runs retain samples but cannot become valid canonical reports.

### Workloads and endpoints

`tools/performance/fixtures.json` versions the generated workload specification;
its SHA-256 identifies the report's fixture. Program times are relative to the
scenario's clock, and local file paths are deliberately excluded from the hash.
During opt-in counter journeys, the guide minute ticker and periodic recordings-screen
UI reload are paused, and periodic EPG checks are disabled; the cached cold profile also disables startup playlist
and guide refresh. This excludes unrelated minute-boundary/background work from
the exact action counters. The native recording executor still starts and playback
uses its normal authorization and connection reservations. The recordings UI normally
reloads jobs, progress and the retired schedules every second (three reader jobs); including
that poll made an otherwise identical selection count differ by three when a tick
crossed the interval. The performance profile excludes that unrelated view poll,
while ordinary app tests continue to exercise it.
The base-compared specification also binds each journey to its exact scenario test.
Changes to fixture generation or scenario identity must update that specification. No provider data,
network stream, credentials, or real media is needed.

| Journey | Fixture/cache state | Measured interval | Deterministic work |
| --- | --- | --- | --- |
| `cached-guide` | 2 channels, 144 hour-long programs; imported SQLite cache, last channel set; new UI root | Immediately before headless root creation to cached current program mounted, with the existing 500 ms settling interval | Owner's reader-lane jobs; one native source open across startup and guide arrival |
| `busy-guide` | 10,000 channels, 70,000 day-long programs; warmed scrolling; SQLite `BEGIN IMMEDIATE`, favorite write queued behind it | 24 Down actions to mounted navigation under the held writer | Reader-lane jobs during the burst; native source opens |
| `selection` | Actual App, one cached channel, no previous playback; realized persistent native surface; generated 160×90 MPEG2 TS loopback media | Normal Enter handler dispatch on a mounted cached guide to the same root's current-source renderer callback | Same app owner's reader-lane jobs and actual native source opens |

The cold measurement is **headless root startup**, not an OS process launch or
physical display scanout. Fixture import and harness loading are excluded. The
busy journey reuses the existing isolated writer-contention scenario and its
deterministic checks. The canonical journey retains its measured maximum key-to-mounted-frame
as an informational observation; the isolated `busy-guide-navigation` check in
`tools/check.sh` (`tools/ci/check-plan.py`) enforces the non-waiting bound that
catches blocking on SQLite's 5 s busy timeout, while the strict 50 ms
responsiveness budget is gated on `IPTV_STRICT_GUIDE_LATENCY=1` for controlled
performance hosts. Guide
navigation and the pending favorite exercise both UI progress and an actual
blocked writer. Filtering/read independence remains covered by the existing
isolated `a_store_read_does_not_wait_for_a_busy_writer` scenario in `tools/check.sh`.

The selection endpoint is **first-render submission**, not display scanout. The
native GTK test runs `Root::of::<iptv_app::App>()` with a fresh generated encrypted
SQLite profile and waits for its cached guide and persistent native surface.
A test-support driver dispatches Enter through the same `input::key_event` path
as the normal registered handler, starting a monotonic interval immediately before
the action. The actual App then authorizes the selected stream, mounts its source
prop, and receives `firstRender {token}` from its own persistent native player.
The end retains that originating token and must match the token expected at the
action. This is one end-to-end app journey, with reader/open counters from that
same owner, without combining timings from independent executions.

The renderer emits only after a successful `mpv_render_context_render` call for
the current loaded entry, with `MPV_RENDER_UPDATE_FRAME` and a PRESENT frame that
is neither REDRAW nor REPEAT. A held frame from the previous stream cannot finish
the new token's interval. The event occurs once per changed source identity/token
and remains marked through PiP or GL context recreation. NEXT_FRAME_INFO is only
queried while first-render evidence is pending. It confirms renderer submission,
not GPU completion, compositor presentation, remote/OS key delivery, or physical
first-picture latency. A separate ordinary headless regression checks source
acceptance/open counts, but supplies no timing segment to the native journey.

The canonical command requires Linux's native backend. On macOS/Windows the
headless scenarios can be validated independently; the complete report remains
invalid without its actual App native-render journey.

A player `ready` event means ready state; `size` means reported dimensions.
Android sends `size` from both `onVideoSizeChanged` and `onRenderedFirstFrame`;
the shared size event cannot distinguish these callbacks. Neither event carries
its originating source token. The parser requires `first-render` for the native
segment and rejects acceptance/ready/size proxies as missing evidence. Android
must retain a callback's originating identity before it can support this metric.

### Report and CI policy

Schema 1 records commit, dirty status, platform/backend, hardware class, fixture hash and
specification, build mode, repetition count, unique invocation identities, deadline, raw per-sample monotonic
microsecond events, selection identity, source token, work counts, median, nearest
rank p95, maximum, and explicit validity/errors. No raw URL, provider name, channel
label, credential, or resolved address is recorded. Owner reader counters count accepted reader-lane jobs (including queued work),
not individual SQL queries or writes. They only exist with `test-support`; normal release builds add no counter or file writes.
Existing native-open counters are reused. The player surface is unchanged.

Each invocation receives its own generated ID, retained separately as the collector's
expected identity and echoed by its producer; mismatches and reused IDs cannot count as independent
repetitions even if the number of sample objects is correct. HEAD and tracked-source
cleanliness are checked again after harness preparation and during sampling.
Missing, duplicate, stale, non-monotonic, timed-out or zero-sample evidence is
invalid. Failed scenarios stay in the report even if they wrote a terminal event.
An invalid run never substitutes a zero duration or quietly drops a failed sample.
Ordinary CI gates the committed work ceilings; wall-clock, frame and memory data
remain evidence rather than new shared-runner performance gates. Existing
isolated latency tests keep their established limits. Deliberately adding a read
to the busy navigation burst or reopening the full-app selection twice exceeds its guard. Counter snapshots
follow a 500 ms observation interval after the endpoint; that interval is not
added to the selection/native latency.
Fabricated regression cases exercise these failures without relying on clocks:

```sh
python3 tools/performance/test_journeys.py
```

PR CI compares the candidate baseline with the **base revision** before running
measurements. Removing a journey/counter or increasing a ceiling requires an entry
in `tools/performance/changes.json` containing `base_sha256`, `candidate_sha256`,
and a specific rationale of at least 30 characters. Hashes cover canonical JSON
(`json.dumps(value, sort_keys=True).encode()`). Fixture specification or endpoint changes also require `fixture_base_sha256`
and `fixture_candidate_sha256` covering canonical fixture JSON, plus a new rationale.
A record for one exact base/candidate hash pair cannot authorize another pair. Reviewers must assess the rationale and fixture change; the
record is evidence for review, not automatic permission. Counter types and complete
journey/counter sets are independently checked. There are no adjustable timing
tolerances. The initial policy is introduced when the base has no baseline file;
subsequent PRs execute the base revision's comparator against the base's copies.
An unresolved base commit fails closed. CI uploads the JSON even
when the policy, build, scenario, or guard fails, with the journey and counter in
the diagnostic.

### Android evidence and opt-in physical TV runs

`tools/perf-android.sh SERIAL [--cold]` remains an interactive profiling aid for an
already installed app with a synthetic playlist. Its launch `TotalTime` is an
activity-launch measure, guide marks are a UI proxy, and size/ready measurements
are uncorrelated proxies, not budgets.
With Bash 4.4 or newer, use `--json PATH` to also retain a versioned Android observation envelope. Its
`valid: false` and explicit unsupported measurements prevent an exploratory log
from being promoted to a valid first-picture journey report. Prepare its
profile with the [generated fixtures and local mock server](#synthetic-data-and-fixtures).

Physical TV measurement is opt-in and separate from emulator validation:

1. Choose an explicitly authorized disposable low-end TV/device and record its
   model, SoC, RAM, Android version, refresh rate and thermal/power settings. Do
   not select a physical serial automatically. Do not clear personal app data.
2. Install the `profile` build, prepare only generated playlists/EPG/media using
   `tools/make-iptv-fixtures.py` and `tools/make-hls-fixtures.sh`, and record the
   fixture hash and exact commit/build mode. Keep the same cache state per run.
3. Collect at least ten runs after a documented warmup, with no competing work.
   Record activity/guide/ready/size proxies under their actual names; a first-picture
   budget requires an identified renderer callback (or an independently documented
   camera method), not the size event. Missing evidence invalidates that metric.
4. Store results with hardware class `physical-tv`, backend and device metadata.
   Emulator evidence uses `emulator` and is never pooled with TV samples or used
   to establish the physical budget. Compare matching hardware/build/fixture only.
5. Retain medians/tails and invalid sample counts, frame/memory traces and thermal
   context before proposing a device budget. A budget is accepted only with its
   endpoint and hardware confirmed.

## Open device checks

- Spoken output, touch exploration, native VOD and episode playback, decoded
  bright, black and white video, physical devices and desktop screen readers
  were unverified at the #68 baseline.
- Channel swipe (#285): on a physical Android phone with TalkBack off and on,
  including the Next channel and Previous channel actions from TalkBack's
  actions menu, and the haptic tick on commit.
- Channel swipe on a physical iPad with VoiceOver: the custom actions and the
  adjustable swipes up and down.
- Guide activation (#192) on a physical Android tablet with Samsung TalkBack:
  explore to a guide row, double-tap and confirm that the channel plays.
