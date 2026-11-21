# Issue #88 tablet journey validation

Evidence for #88: a tablet viewer's journey by touch, through gestures,
rotation, split-screen widths, large text, the keyboard and interruptions,
checked against the [interaction contract](../interaction.md). Run on 20
November 2026.

All data is synthetic: the 300-channel generated playlist and XMLTV from
`tools/make-iptv-fixtures.py`, generated HLS test streams from
`tools/make-hls-fixtures.sh`, served by `tools/mock-server.py` on the host
(port 8093, every request logged: the stream-request counts below), and the
generated playlists, guides and on-demand catalogue of the headless app
scenarios. No provider data was used.

**Physical devices: none.** Everything below is the `tab36` emulator, an
iPad simulator or headless. No result here stands for a physical Android
tablet or a physical iPad.

**Follow-up (after #134).** The review fixes (audio focus held by
`AudioFocusState`, multiview interruptions, viewer actions, pan anchor,
drag threshold at a list's end) were checked again
headless and on two more disposable `tab36` runs, recorded under
[Follow-up runs](#follow-up-runs). The iPad simulator results predate
those fixes (commit `908632e9`).

## Build

| Item | Value |
|---|---|
| Commit validated | `5b8f5f42` (branch `feat/issue-88-tablet-journey`) |
| Android APK | Debug build, validation ID `com.hantekllc.iptvrs.validation`, x86_64, `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true`, development playlist `http://10.0.2.2:8093/iptv/playlist.m3u`, built with `tools/sync-host.sh` and Gradle `assembleDebug testDebugUnitTest` (the Kotlin player-contract tests passed) |
| APK SHA-256 | `ccba24f5dfe5e0642041197dcf31c6b9d9ab521fe4d4b351bbdd2ba54fcaa57d` |
| Rust | 1.99.1 (the pinned toolchain) |

## Hosts

| Host | Profile | OS | Input | Viewport | Text |
|---|---|---|---|---|---|
| Android tablet (emulator) | Disposable copy of `tab36` (Pixel Tablet, 2560×1600 at 320 dpi, 1280×800 dp), run `run-wvf0ejp3` on `emulator-5580`, `operation.json` `"result": "passed"` | Android 16 (API 36) Google APIs x86_64, `swiftshader`, no audio | Smoke's remote keys, then `adb shell input` touch taps and swipes, key events, rotation, `wm size`, `font_scale`, and the emulator's own incoming call (`adb emu gsm call`) | 1280×800 dp; portrait 800×1280 dp; 640×800 dp (`wm size 1280x1600`, a half-width split-screen window) | System font scale 1.0 and 2.0 |
| Headless (Android tablet idiom) | `cargo test -p iptv-app --features test-support --test app`: `tablet_gestures`, `tablet_journey` | Linux, environment `Idiom::Pad`, `Os::Android` | Touch drags and taps, keys, a software keyboard's height, native player events the harness injects | 1280×800, 800×1280, 640×800, 800×640, 400×800, 1280×400 | Font scale 1.0, 1.3 and 2.0 |
| iPad (simulator) | Fresh iPad Air 11-inch (M4) simulator on a separate macOS host (macOS 26.6.2, Xcode 27.2, iOS simulator runtime 27.2), built with `nsh package . --target ios-simulator` at `908632e9` (Rust 1.99.1, nsh 0.6.0 at `8183542`); the same generated fixtures and mock server | iOS 27.2 (simulator) | `idb` taps, swipes, rotation and typing | 1180×820 pt, landscape left and right (portrait and Split View: blocked by app configuration, #138) | System text size changed once |

The emulator's split-screen check resizes the display, not a second app's
window beside Patina: it shows the layout at that width, not Android's
split-screen lifecycle. Paths such as `target/android-validation/run-wvf0ejp3/`
are local run directories, not committed; the screenshots linked from
`docs/screenshots/issue-88/` are.

"Same SurfaceView" below means `dumpsys SurfaceFlinger --list` showed the
player's `SurfaceView` (`117c421`) with the same layers before and after
the step. Stream requests are the mock server's `index.m3u8` requests;
opens are the app's `iptv player 0 open` log lines.

## Criteria and evidence

The criteria are #88's acceptance criteria. The iPad column is the
simulator run at `908632e9`, before the follow-up.

| #88 criterion | Headless | Android `tab36` (emulator) | iPad |
|---|---|---|---|
| Vertical, horizontal and diagonal drags, flings and cancelled gestures give the documented scroll outcome, with zero unintended channel, play or record activations | Met: `tablet_gestures::tablet_drags_scroll_and_pan_without_playing_or_recording` on a generated 30-channel guide with 180-, 45-, 30- and 15-minute programmes: vertical drags scroll only, horizontal drags pan only, each diagonal follows its larger part (2.2:1 and 1.5:1 pan), a fling carries on, a system-cancelled drag rests without a fling; drags start on channel names, inside long programmes, on adjacent short ones and on replayable past ones. No channel tuned except the first channel's autoplay preview, no details, menu or recording job; a key-started autoplay preview is cancelled by a horizontal drag, a vertical one, a drag down at the list's top that scrolls nothing, and a drag on the player's channel list, and panning back to now does not start it again; a tap's choice survives a pan right after it; an 8-point sideways wobble stays a tap and plays full screen | Met for vertical, horizontal, both diagonals and a fling (`adb shell input swipe`): 7 opens and 9 stream requests before and after all five, same SurfaceView. [After the fling](../screenshots/issue-88/tablet-guide-after-fling.png): 22 hours ahead, the rows still where the vertical swipe left them. Follow-up: a 1.5:1 diagonal pans; with **Play the focused channel by itself** on, a drag down at the list's top right after a key moved the focus started no preview ([screenshot](../screenshots/issue-88/tablet-clamped-drag-no-tune.png)), while the same key without a drag did. Cancelled gesture: not run on the emulator | Vertical, horizontal and flings: met, 0 stream requests. **Fail before the follow-up:** diagonals up to about 1.5:1 scrolled the channels instead of panning (swipe (1000,700)→(700,500): rows 21→28, the time axis unchanged; [screenshot](../screenshots/issue-88/ipad-sim-diagonal-scrolled-rows.png)); 2:1 and 4:1 panned. **Met after #149** (NSH pin `09788806`): the same swipe pans the timeline and leaves the rows, 1:1.5 scrolls the rows, and an 8 dp sideways-wobble tap plays ([re-run](#ipad-diagonal-re-run-149)). Cancelled drags: not run |
| A channel, programme, movie and episode can each be reached and played by visible touch controls alone | Met: `tablet_journey::touch_alone_plays_a_channel_and_a_past_programme_and_returns` (a tap on a channel, the visible ‹ back, the ruler's ‹ to a past programme, a tap replays it) and `touch_alone_plays_a_movie_and_an_episode_through_rotation_split_screen_and_large_text` (the Movies and Shows shortcuts, a poster, Play, a show, an episode, the play bar's ‹ and play/pause, Close) | Channel: met (a tap played it full screen, 1 open, 1 stream request; the ‹ returned to the guide). Programme, movie and episode: not run (the smoke fixture is an M3U playlist with no movies or series; no replay was tapped) | Channel: met (a tap played it full screen with 1 stream request; the ‹, shown after a tap on the video, returned to the guide on that channel with the preview playing, no reopen; [screenshot](../screenshots/issue-88/ipad-sim-full-screen-play-bar.png)). The ⋯ in the guide header opened the menu. Programme, movie and episode: not run |
| Portrait, landscape and narrow split screen preserve content and progress and keep primary controls reachable at enlarged text | Met: the movie at 37 s of 2 min stays full screen with its progress, and the show's details keep the selected episode, across six presentations at text 1.3 and 2.0, on the same `tv.player` (mounted once, never removed or re-inserted), no stream opened; the play bar's ‹ and play/pause and the details' Close lie inside every window (`tablet_journey`) | Met for live TV full screen: portrait, text 2.0, then 640×800 dp kept the channel playing, same SurfaceView, no open, no stream request; ‹, play/pause and the progress line visible ([portrait at 2.0](../screenshots/issue-88/tablet-portrait-text-2.0-play-bar.png), [half width at 2.0](../screenshots/issue-88/tablet-half-width-text-2.0-play-bar.png)). Observation 2 | Landscape left ↔ right in the guide and full screen kept the channel and playback, no new request; a text size change reloaded nothing. Portrait and Split View: **blocked by app configuration** (`nsh.json` limits iPad to landscape and requires full screen), tracked in #138 |
| Keyboard appearance and dismissal and touch ↔ keyboard switching leave a usable layout and focus, without triggering player shortcuts while editing | Met: `tablet_journey::a_keyboard_over_search_edits_text_without_reaching_the_player` (the field stays above a 340–420 point keyboard in landscape, portrait and split; typed letters, Space, K and M neither pause, mute nor zap; after dismissal a key and a tap each drive the results) and `keyboard_dismissal` | Met: Search over the playing preview raised the keyboard (`mInputShown=true`) with the field above it; `input text` and Space, K, M, F key events went into the field ([screenshot](../screenshots/issue-88/tablet-search-keyboard.png)); no pause or mute logged, no open; Back hid the keyboard (`mInputShown=false`). Hardware keyboard and mouse: not run | Met: with the software keyboard up in Search, K, M and Space typed into the field; nothing paused, muted or zapped. Hardware keyboard and mouse: not run |
| PiP, Home and return, and interruption and route-change journeys follow the documented host policy, with no duplicate provider stream and no automatic restart of explicitly stopped playback | Met for the policy: `iptv_core::interruption` unit tests; `tablet_journey::calls_and_unplugged_headphones_pause_by_policy_and_never_restart_a_stop` (a call pauses and resumes, unplugged headphones and another app's media pause for good, a viewer's pause during a call is kept, a stop during a call stays stopped, no stream opened by any of it) | Call: met with the emulator's own incoming call: `focusLossTransient -> Pause`, the ringing call took the focus ([screenshot](../screenshots/issue-88/tablet-call-paused.png)), cancelling it gave `focusGain -> Resume`; no open, no stream request. Home and return: met, the stream stopped and the focus was given up on Home, and return opened it once (1 open, 1 stream request: no duplicate). PiP: **fail**, observation 1. Route change and another app's media: not run (no headset or second media app on the emulator). Explicit stop then return: not run (the media Stop key did not reach the app). Follow-up: a call's end kept the focus (no give-up and re-request); Home during a call gave the focus up (`focus given up`, the focus stack then held only the call), and after the call nothing resumed or reopened; return reopened once, still paused | Not run: Home and return, picture in picture, interruptions and the audio session (this change's interruption handling is Android's only) |
| Android's persistent player surface survives layout-only transitions without hide, detach or re-parent; background recovery follows platform lifecycle policy | Met: the `tv.player` node mounted once, never removed or re-inserted, through every presentation (`tablet_journey`, `pane_collapse`) | Met for layout: same SurfaceView through fling, rotation, text 2.0, the width change and the call. Home stops playback and the system destroys the surface's layers; return makes new layers for the same SurfaceView and reopens once, as the lifecycle policy says | Landscape rotations kept playback without a new request; the native view's lifetime was not inspected |
| Validated on tab36 and an available physical Android tablet; iPad recorded separately; actual touch and native evidence | `tab36` above (`adb` touch input, real audio focus and a real incoming call) | Physical Android tablet: **not run**, none available to this run | iPad simulator above, at `908632e9` (before the follow-up). Physical iPad: not run |

## Bugs and observations

1. **Picture in picture did not open on `tab36`.** The play bar's
   picture-in-picture button logged `iptv pip: asked to enter` and then
   `iptv pip: not entered` (`enterPictureInPictureMode` returned false), and
   Home did not enter it either, although the task's parameters showed
   `isAutoPipEnabled=true`. Home stopped playback as the background policy
   says; nothing played twice. Not investigated here; the emulator image's
   window manager (desktop windowing on a tablet) is a likely cause.
2. **At half width and text 2.0 the play bar's last buttons are cut off.**
   The ‹, play/pause and progress stay reachable, but the bar's row of
   secondary buttons runs past the window's right edge
   ([screenshot](../screenshots/issue-88/tablet-half-width-text-2.0-play-bar.png));
   at that size the times wrap to two lines. #86 marks collapsing the
   player's panels by class as Planned.
3. **Fixed in this change:** panning the timeline into a day of listings not
   yet read scrolled the channel rows back to the focused channel after a
   touch scroll (found by `tablet_gestures`).
4. **Observation:** with programmes of 5 minutes, a guide row showed no
   programmes at all headless (each row mounts 17 cell slots; the cause was
   not investigated). The gesture scenario uses 15 minutes and longer; not
   fixed here.
5. **Observation:** a very fast swipe (1,400 px in 80 ms) flung the timeline
   about 22 hours; the Now button brings it back.
6. **Fixed in the follow-up:** after the call ended the host gave up the
   audio focus and requested it again within 10 ms, while the app's resume
   reached the player. The focus is now kept until the app decides; the
   follow-up run showed no give-up after the call's end.
7. **Fixed (iPad simulator, #149):** diagonals up to about 1.5:1 scrolled
   the channels instead of panning. The timeline's pan activates once the
   finger moved 10 dp sideways; the iOS list's own recognizer starts
   along the finger's path, so it won a steep diagonal first. Lowering
   Patina's activation offset (tried at 7 dp) swallowed taps with a
   7–10 dp sideways wobble, because the pan's activation cancels the
   press, so it was reverted. The fix is in NSH's iOS host, pinned in
   `nsh.rev`: a scroll view inside a view whose pan names one axis it
   cannot scroll along waits for that pan to decide, and the pan judges
   each move along its line, so the offset the finger passes first wins
   even when one move passes both. Patina's 10 dp activation is
   unchanged, so taps stay taps. See the
   [re-run](#ipad-diagonal-re-run-149). Android `tab36` already panned a
   1.5:1 diagonal. The headless 1.5:1 case passes either way: the
   harness models no native list's slop.
8. **Observation (iPad simulator):** the play bar's buttons appear as empty
   0×0 groups in the accessibility tree, so VoiceOver may not reach them.
9. **Observation (iPad simulator):** playback stopped after about 100–200 s
   with "This stream's format isn't supported"
   (`ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED`, VTDecompression error
   −12906), three times including while idle; the cause (the simulator's
   decoder or the finite 300 s fixture) is tracked in #137.
10. **Observation (iPad simulator):** the simulator package has no
    entitlements, so the keychain was unavailable and adding a playlist
    failed until simulator entitlements were embedded at link time;
    tracked in #136.
11. **Not changed:** Back from a movie or an episode played full screen
    returns to the guide (the title playing on in the preview), not to its
    details. Returning to the details changes the shared Back order for
    every input and 11 existing scenarios that expect the guide; it is
    tracked in #147 (the contract marks it Planned).

No crash or refused player load was seen (the smoke's final checks), and
no stream was opened by a gesture, a rotation, a width or text change,
typing, or the call.

## Not run

- Physical Android tablet: none available.
- iPad: portrait and Split View (blocked by app configuration, #138); a
  hardware keyboard and mouse; cancelled drags; Home and return; picture in
  picture; interruptions and the audio session; VoiceOver; a physical
  iPad. The iPad host's audio session and foreground-only recording are
  unchanged by this change.
- On `tab36`: a replayed programme, a movie and an episode (the smoke's M3U
  fixture has none); a system-cancelled drag; headphone or Bluetooth route
  changes and another app taking the audio; an explicit stop followed by
  Home and return; a hardware keyboard and mouse; Android's real
  split-screen with a second app; TalkBack.
- Audio output: the emulator runs with `-no-audio`, so focus changes were
  checked by the system's focus stack and the app's logs, not by sound.
- Another app taking the audio for good, on the emulator: no second media
  app; the reacquisition rules are covered by `AudioFocusStateTest` on the
  JVM only.

## Follow-up runs

Both used the same APK, built from the follow-up code (branch
`fix/issue-88-review-followups`), validation ID, x86_64, development
playlist on port 8093: SHA-256
`29df777e795b283381c47a3300bdb7984bdaa5e5e1595d9cc07775cd209bdb20`.

| Run | Result | Checked |
|---|---|---|
| `run-ea9c7j_r` | Smoke steps passed; **recorded as incomplete**: a manual force-stop during the hold (to reload settings) ended the app, which the hold treats as a failure | A real incoming call (`adb emu gsm call`): `focusLossTransient on surface 0 -> Some(Pause)`, then `focusGain -> Some(Resume)` with no give-up or re-request; Home during a call gave the focus up and unregistered; no resume or reopen after that call; return reopened once, paused; a 1.5:1 diagonal swipe panned the timeline and scrolled no rows |
| `run-s7ebjl02` | `"result": "passed"` | With **Play the focused channel by itself** on (set in Settings by touch): a key moving the focus started its preview after 0.5 s (control); at the list's top, a key then a drag down started none |

### iPad diagonal re-run (#149)

Run on 20 November 2026 on a fresh iPad Air 11-inch (M4) simulator
(iOS 27.2, Xcode 27.2, macOS 26.6.2), landscape (1180×820 pt). The app was
built with `nsh package . --target ios-simulator` from the #149 branch with
NSH at `09788806` (teng-lin/nsh#1). The same change merged upstream as
longbridge/nsh#2724 and is pinned as `fd847c95`. The build used debug
assertions, so the development playlist, the generated 300-channel
fixtures and
`tools/mock-server.py` on port 8149 apply, and the #136 simulator
entitlements were embedded at link time. Input was `idb ui swipe` with
`--duration 0.5 --delta 5` unless noted. idb's default swipe, with no
duration, lands as a tap.

| Swipe | Result |
|---|---|
| 1.5:1, (1000,700)→(700,500), the issue's swipe | Panned the timeline about 1.5 hours, with the rows unchanged, in 8 of 10 runs, including 3 of 3 with `--delta 10`, where one move passes both 10-point offsets. ([before](../screenshots/issue-88/ipad-sim-diagonal-before-pan.png), [after](../screenshots/issue-88/ipad-sim-diagonal-panned.png)). The 2 other runs came first, while the Mac was under a load of several hundred from other builds. Nothing moved in either: no pan and no scroll. |
| 1.5:1, 450×300 | Panned 2.5 hours; the rows were unchanged |
| 2:1 | Panned; the rows were unchanged |
| 1:1.5, (1000,700)→(800,400) | Scrolled the rows by about 10 channels; the time axis was unchanged |
| Horizontal | Panned and flung on |
| An 8 pt sideways wobble on a channel's name (`--duration 0.15 --delta 2`) | A tap: the channel played full screen |

Stream requests to the mock server: 0 after all the swipes, then 1
(`/ch1/index.m3u8`) for the wobble tap's channel.

NSH's own native check is the gallery test
`examples/gallery/tests/pan-around-scroll.test.tsx`: a 1.5:1 diagonal, a
1:1.5 diagonal and an 8-point wobble tap on a sideways pan around a
vertical scroll view, run with `nsh test --target ios` on the same
simulator.

- **The 1.5:1 case:** with NSH's previous iOS gate it scrolled the list,
  in 2 of 2 runs. With the fix it panned, in 9 of 9 runs.
- **The other two cases:** in this simulator, a test that opens the app
  after the first one sometimes receives none of its touches. The same
  happens to the unchanged `pan-over-scroll` test, which failed 2 of 3
  runs that way. Their passing runs are the only evidence they give here.
