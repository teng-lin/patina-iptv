# Issue #141: physical Android tablet validation

Part of #141: the tablet journey of #88 on a physical Android tablet, run on
20 November 2026. It covers the touch journey, rotation, real split screen
with a second app, another app's audio, picture in picture and the
background lifecycle. Checked against the
[interaction contract](../interaction.md) ("Touch gestures in the guide",
"Interruptions and the background") and the emulator record in
[issue-88-tablet-journey.md](issue-88-tablet-journey.md).

All data is synthetic: the 300-channel playlist and XMLTV from
`tools/make-iptv-fixtures.py`, the three 300 s HLS test streams from
`tools/make-hls-fixtures.sh --android-smoke` and a 60 s generated sine tone
(the "other app's" audio). `tools/mock-server.py` served them on this
host. No provider data, account or credential was used. The screenshots
show only Patina, the synthetic tone and Samsung's Calculator. The system
status bar is cropped out, and the picture-in-picture shots are cropped to
the window.

## Device and build

| Item | Value |
|---|---|
| Device | Samsung Galaxy Tab A9+ Wi-Fi (SM-X210, `gta9pwifi`), arm64-v8a |
| OS | Android 16 (API 36), One UI build `BP2A.250605.031.A3.X210XXUDEZI1`, security patch 2026-08-05 |
| Display | 1200×1920 at 240 dpi (landscape 1920×1200 px, 1280×800 dp); the user's system font scale 1.15, left as it was |
| Input | Touchscreen (`chipone-tddi`) only; no hardware keyboard or mouse attached |
| Connection | Wireless adb; the app reached the mock server through `adb reverse tcp:8141` (the host's firewall blocks the LAN port) |
| Commit | `0738213675d7a389e6a34860f48e41935834775f` (`origin/master`, contains #134, #143, #150, #151) |
| APK | Debug build, validation ID `com.hantekllc.iptvrs.validation`, `ABIS=arm64-v8a`, `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true`, development playlist `http://127.0.0.1:8141/iptv/playlist.m3u`, built with `tools/sync-host.sh` and Gradle `assembleDebug` |
| APK SHA-256 | `34e527228782d51e33b8b610ad405c2fc12890bccdc65a2ec46c6fcc5b87ffd4` |
| Toolchain | Rust 1.99.1 (pinned), NSH `8183542` (`nsh.rev`) |

Stream requests are the mock server's `index.m3u8` requests; every player
open in this run made exactly one, eight in all. Opens are the app's
`iptv player 0 open` log lines (logcat tag `nsh`). "Same SurfaceView"
means `dumpsys SurfaceFlinger --list` showed the player's
`SurfaceView[…TvActivity]@0` (`90c3117`, layers `#1832`–`#1834`) before and
after the step.

## Results

| # | Check | Result | Evidence |
|---|---|---|---|
| 1 | Install and first run with a synthetic playlist; the guide loads | **Pass** | Fresh install, `iptv pip: supported true, tv false`. Touch alone: Add playlist, Playlist link (prefilled), Add playlist: `guide switched to 300 imported channels`, 73,559 programmes; Watch showed the guide ([screenshot](../screenshots/issue-141/tab-a9-guide-first-run.png)) |
| 2a | Tap a channel: full screen | **Pass** | A tap on a channel name played it full screen: 1 open, 1 stream request. A tap on the channel already playing in the preview went full screen with 0 requests |
| 2b | Visible ‹ back | **Pass, with one unreproduced miss** | Tapping the video showed the ‹ and the play bar ([screenshot](../screenshots/issue-141/tab-a9-full-screen-play-bar.png)). The ‹ returned to the guide with the preview playing on (`play bar: back`, `left full screen`, 0 requests): 9 of 9 probe taps within its circle, after a new tune, and after a cold start. Observation 1 describes the miss |
| 2c | ⋯ More options | **Pass** | The guide header's ⋯ opened the channel and programme menu ([screenshot](../screenshots/issue-141/tab-a9-more-options.png)) |
| 2d | Vertical, horizontal and diagonal swipes tune nothing | **Pass** | `input swipe`, 300 ms each: vertical scrolled rows 1→12 only; horizontal panned 2:30→5:30 PM only, rows unchanged ([screenshot](../screenshots/issue-141/tab-a9-guide-after-horizontal-swipe.png)); a 5:3 diagonal panned (5:30→8:30 PM, rows unchanged); a 1:2 diagonal scrolled (rows 12→23, time unchanged); a drag starting on a channel name scrolled; an 80 ms fling panned into the next day. **0 new stream requests (3 → 3), no player, menu, details or record line** |
| 2e | A tap with a small wobble still plays | **Pass** | Down, 8 px sideways and 2 px down in 150 ms on a channel: it played full screen (1 open, 1 request) |
| 3 | Rotation keeps the channel and playback, no new stream | **Pass** | With `accelerometer_rotation 0`: full screen landscape → portrait ([screenshot](../screenshots/issue-141/tab-a9-portrait-full-screen.png)) → landscape, then the guide portrait → landscape. The same channel kept playing, same SurfaceView, same process, 0 opens, 0 requests (4 → 4). The user's rotation settings were restored afterwards |
| 4 | Real split screen with a second app | **Pass** | Calculator moved beside Patina with SystemUI's own command (`dumpsys activity service SystemUIService WMShell splitscreen moveToSideStage <task> 1`): Patina's task 954×1177 px. The guide ([screenshot](../screenshots/issue-141/tab-a9-split-screen-guide.png)) and full screen in the half ([screenshot](../screenshots/issue-141/tab-a9-split-screen-play-bar.png)) kept playing. Focusing and typing in Calculator (`topResumedActivity` Calculator, Patina still resumed) paused nothing. `exitSplitScreen` back to full width. 0 opens, 0 requests throughout. The play bar overflows at that width, **#163** (observation 2) |
| 5 | Another app's audio takes the focus (#143) | **Pass** | Samsung's sound player, opened on the pushed tone, appeared over Patina, which stayed visible: `focusLoss on surface 0 -> Some(Pause)`, `focus given up`. The focus stack held only the sound player ([screenshot](../screenshots/issue-141/tab-a9-other-app-audio-paused.png)). Pausing and then closing it: no `focus requested`, no resume. 10 s later Patina was still paused with ▶ shown ([screenshot](../screenshots/issue-141/tab-a9-stays-paused-after-other-app.png)) and the focus stack was empty. Tapping play: `focus requested`, playback resumed on the same stream (0 requests) |
| 5b | Headphones unplugged, Bluetooth disconnect | **Not run** | Not reliably simulated over adb: no headset or Bluetooth device was available to plug or unplug |
| 5c | A real phone call | **Not possible** | Wi-Fi-only tablet, no telephony |
| 6 | Picture in picture from the play bar and from Home | **Pass: no real bug** | Play bar button: `pip: asked to enter`, `pip: entered`; the task went to `mWindowingMode=pinned` at 650×366 with the activity PAUSED and still holding the audio focus. The system window showed Previous channel, Pause and Next channel ([screenshot](../screenshots/issue-141/tab-a9-pip-from-play-bar.png)). Home while playing: auto-entered (`isAutoPipEnabled=true`, `pip: entered`, [screenshot](../screenshots/issue-141/tab-a9-pip-from-home.png)). 0 requests either way. Expanding back: `pip: left`, 0 requests. Closing the window with its ✕: `pip: left`, `focus given up`, activity STOPPED. The return reopened once (1 request). PiP works on this hardware. Why `enterPictureInPictureMode` returned false on `tab36` (observation 1 of the #88 record) is not established; #88 did not investigate it and named the emulator's window manager only as a likely cause |
| 7a | Home and return: the stream stops, then reopens once | **Pass** | With picture in picture refused (`appops … PICTURE_IN_PICTURE ignore`, reset afterwards): Home → `focus given up`, activity STOPPED, both decoders released (`c2.qti.avc.decoder`, `c2.android.aac.decoder` `RELEASED`, `Codec shutdown complete`), no request in the background. Return → 1 open, 1 request. The same held when Calculator covered Patina full screen (1 open on return) |
| 7b | An explicit stop stays stopped after Home and return | **Pass** | `input keyevent KEYCODE_MEDIA_STOP` reached the app on hardware: `key=stop … action=Some(Stop)`, `player: stopped by the viewer`, `focus given up`, guide with "Nothing playing". Home, then return after 8 s: no open, 0 requests, no audio focus ([screenshot](../screenshots/issue-141/tab-a9-stop-stays-stopped-after-home.png)) |
| 8 | Hardware keyboard and mouse | **Not run** | None attached (`dumpsys input` lists only the touchscreen, keys and the headset jack) |

No crash was seen, and no player error or refused load. No gesture,
rotation, split-screen change, picture-in-picture change or other app's
audio opened a stream.

## Observations

1. **‹ missed three taps once (not reproduced).** In the first full-screen
   session after the first playback, three taps on the ‹'s drawn centre
   (65, 60) px over about a minute did nothing: `input tap` twice, and a
   120 ms press. Logcat showed the app's window received the touches (`ViewPostIme
   pointer 0/1`) but Patina logged no action. The play bar stayed up for
   that whole minute instead of auto-hiding, while play/pause on the same
   bar worked. A tap lower on the circle (66, 95) then worked. Afterwards
   the same centre tap worked every time: nine probes from (40–90, 30–85)
   all returned, while (66, ≥110) below the circle did nothing, as
   expected. It also worked after tuning another channel and after a cold
   start. No system overlay covered that area (`dumpsys input`). Not
   filed, because it could not be reproduced. Worth watching for in
   later hardware runs.
2. **Half-width split screen hides the play bar's last buttons at text
   1.15: #163.** At Patina's 954 px (≈636 dp) half, the picture-in-picture
   button is cut in half and the guide button is off-window
   ([screenshot](../screenshots/issue-141/tab-a9-split-screen-play-bar.png)).
   The #88 emulator run saw this only at text 2.0. On this tablet it
   happens at the device's ordinary font scale.
3. **Picture in picture works on hardware.** The `enterPictureInPictureMode`
   false on `tab36` did not reproduce here, from the play bar or from Home.
   The emulator image's windowing mode remains the likely cause there.
   Nothing to fix in Patina.
4. **The media Stop key reaches the app on hardware** (it did not on the
   emulator), so the stop-then-Home case of #141 is covered here.
5. In split screen Samsung draws its window-handle pill over Patina's
   header (the guide's title) until it fades. This is system chrome and
   needs no change in Patina.
6. In portrait (800 dp wide) the play bar drops the channel name and
   quality badges and keeps every button reachable. Recorded only; not a
   defect.

## Not covered (rest of #141)

- Headphone unplug and Bluetooth disconnect on real hardware (no device to
  unplug), and a real phone call (Wi-Fi tablet).
- Hardware keyboard and mouse (none attached).
- TalkBack: not run, because enabling it changes the user's accessibility
  settings.
- Replay, movie and episode playback on Android: the fixture is the M3U
  smoke playlist, with no catch-up replay tapped and no VOD catalogue.
- Physical iPad, and the iPad's audio session (#138).

## Device restoration

After the run the tablet was restored and checked: `user_rotation 0`,
`accelerometer_rotation 1` (the original values), font scale untouched
(1.15), the validation APK uninstalled (`pm list packages` shows only the
user's own `com.hantekllc.iptvrs`, which was never touched), the pushed
tone deleted from `/sdcard/Download` (the user's own files there left
alone), the `adb reverse` removed, and the Calculator and sound-player
tasks this run opened removed from recents. No other setting was changed.
