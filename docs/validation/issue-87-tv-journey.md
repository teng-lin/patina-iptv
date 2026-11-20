# Issue #87 TV remote journey validation

Evidence for #87: the complete TV remote watching journey, its short and
long presses, Back, choosers and stream continuity, checked against the
[TV key table](../tv-controls.md) and the
[interaction contract](../interaction.md). Run on 20 November 2026.

All data is synthetic: the 300-channel generated playlist and XMLTV from
`tools/make-iptv-fixtures.py`, generated HLS test streams from
`tools/make-hls-fixtures.sh --android-smoke`, served by `tools/mock-server.py`
on the host, and the three local channels of the headless app scenarios. No
provider data was used.

**Physical devices: none.** Everything below is an emulator or the headless
app. No result here stands for a physical Android TV or remote.

## Build

| Item | Value |
|---|---|
| Commit validated | `b82e9109` (this branch's code change; later commits change tests and docs only) |
| Android APK | Debug build, validation ID `com.hantekllc.iptvrs.validation`, x86_64, `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true`, development playlist `http://10.0.2.2:8089/iptv/playlist.m3u`, built with `tools/sync-host.sh` and Gradle `assembleDebug` |
| APK SHA-256 | `2ea77e8e79682801b5ae793ea0159ac7dd1c9045a868c8ec57971b8e8a4930a7` |
| Rust | 1.99.1 (the pinned toolchain) |

## Hosts

| Host | Profile | OS | Input | Viewport |
|---|---|---|---|---|
| Android TV (emulator) | Disposable copy of `tv36` (`tv_1080p`, 1920×1080 at 320 dpi), run `run-2mu53hls` on `emulator-5582`, `operation.json` `"result": "passed"` | Android 16 (API 36) Android TV x86_64 image, emulator 37.2.13.0, `swiftshader`, no audio, TalkBack off | Injected key events (`ShortKeys` with real Down/Up and a timed hold; `input keyevent --longpress` with Down/Repeat/Up), one injected tap | 960×540 dp |
| Headless app | `cargo test -p iptv-app --features test-support --test app` | Linux | Raw Android key phases on the window root (`iptv_app::use_remote_key_phases`), TV idiom, Android OS | 960×540 |

"No new stream" below means the app logged no further `player 0 open` line
(Android) or `iptv_app::player_opens` did not change (headless). On Android
that does not show the native surface survived; the headless scenarios check
the one `tv.player` node is never removed, reparented or hidden.

## Android TV emulator results

The smoke ran its TV steps first (see `tools/ci/android-smoke.sh`), then held
the owned emulator (`IPTV_SMOKE_HOLD`) for these manual key checks. Video
decoded in the emulator (test pattern in the screenshots).

| Step | Result | Evidence |
|---|---|---|
| Add the synthetic playlist, play in the guide, full screen | Pass | Smoke "add the synthetic playlist" to "full screen" |
| Long OK with a timed hold (no repeats) opens the play bar once, no short action on release | Pass | Smoke "shared TV controls and long OK release": `long press by timer: ShowControls`, then Right and Back in `Controls`; no `Select` or `ShowPanels` on release. Manual repeat: Down, timer `ShowControls`, no Up action; [tv-long-ok-play-bar.png](../screenshots/issue-87/tv-long-ok-play-bar.png): Play/pause focused, still playing |
| Long OK with Android repeats (`input keyevent --longpress`) | Pass | `key=ok phase=Down ctx=Player action=None`, `long press by timer: ShowControls`, `key=ok phase=Repeat ctx=Controls action=None`; no Up action |
| Rapid taps (two OK pairs queued back to back) | Pass | Guide `Select` (full screen) then Player `ShowPanels`, each once, 1 ms apart |
| Browse the guide without retuning | Pass | Down, Down, Up, Up over 4 s: four `MoveUp/MoveDown`, no `player 0 open` |
| OK on the playing channel returns it to full screen | Pass | `Select` → `mode=Fullscreen`, no new stream |
| Zap, then the previous channel | Pass | Down `ChannelDown` → `player 0 open: session 3`; Right `PreviousChannel` → `session 4`; one open each |
| Play bar: Right skips disabled controls; Picture size chooser; pick; focus returns | Pass | Menu `ShowControls`, four Rights, `play bar: Picture size`, chooser Down and OK in `ctx=Menu`; [tv-chooser-returns-to-picture.png](../screenshots/issue-87/tv-chooser-returns-to-picture.png): Picture focused again; no new stream |
| Back: the bar, then full screen to the guide on the playing channel | Pass | `key=back ctx=Controls`, then `ctx=Player`; [tv-back-to-guide-playing-channel.png](../screenshots/issue-87/tv-back-to-guide-playing-channel.png): the guide on Synth Travel 0009 (▸), video in the mini-player |
| Background, resume, kill, recreate, restore | Pass | Smoke "background and resume" to "interrupt then recover a restore" |
| A surface change during a hold | Not run natively | adb could not land a tap inside the 400 ms hold: both tries arrived after the long OK had opened the guide menu (`long press by timer: ShowMenu`). Covered headless below |
| Replay and video-on-demand seek and tracks | Not run on Android | The smoke fixture has no catch-up archive, movies or series |

No crash or refused player load (smoke "checks"); 0 player errors.

## Headless results

`tv_remote_journey` drives the Android key path (raw Down, Repeat and Up,
the app's long-press timer on the engine clock) on a TV idiom:

| Scenario | Checks |
|---|---|
| `long_ok_rapid_taps_and_a_surface_change_during_a_hold` | Long OK with 50 ms repeats and without repeats: the bar opens once with Play/pause focused, no card. Three rapid OK taps: card, hidden, card; no stream opens. OK held as the info card's timer runs out: the card stays and the release presses its Guide action. OK held in the guide while a tap plays Local B: the early release does nothing (`Short dropped`); held while a tap plays Local C: no menu by the timer, no controls by a repeat, no card by the release (`Long dropped`). One `tv.player` throughout |
| `the_remote_journey_browses_without_retuning_and_returns_to_the_guide` | Hold Left: groups; choose Local A (1 open); Menu: bar; channel list over video, browse to Local B for 3 s: no open; zap to B (2), previous channel to A (3), ⏮ to B (4); Back to the guide, browse: no open; Up to the mini-player, OK: B full screen, no open; Back. One `tv.player`, never removed or reparented |
| `a_press_belongs_to_its_owner_and_a_prompt_takes_its_keys` | OK held on the info card while a tap opens More controls: no Search on release. OK held in the guide while a tap opens a PIN prompt, released under the prompt, prompt cancelled before the threshold: no menu by a late timer. Raw Back on a PIN prompt plus Android's Back: one step. The OK that submits the PIN, held with repeats: no play bar, no card, no menu. Back held on the card while a tap takes its TV guide action: Android's Back after the release is swallowed |
| `holds_keep_their_card_decide_once_and_menu_does_not_toggle` | Menu held with repeats: the menu opens once and stays. Card timeout 1 s, long press 2 s, one repeat at 400 ms: the card outlives its timeout and a 1.5 s release presses its action. On the play bar, a hold decided by its repeat (no timer) refocuses Play/pause; Menu held hides the bar and its repeats do not reopen it |
| `a_back_or_ok_held_into_a_pin_prompt_stays_cut_short` | Back held in the guide while a tap opens a PIN prompt: its release and Android's Back leave the prompt open; a fresh Back closes it. A Back guard left armed by a dropped Back with no callback does not swallow a fresh Back on a PIN prompt. OK held into a prompt, a repeat under it, the prompt cancelled by touch: later repeats and the release select nothing |
| `extra_large_text_keeps_every_focused_bar_control_on_screen` | Extra large text at 960×540: exactly one bar control focused at a time, each whole on screen, live skipping Rewind |

Also: `tv_replay` (replay seek by remote opens the archive once; a subtitle
track picked by remote reaches the same player with no stream opened and
focus returns to Tracks), `tv_shared_controls` (mini-player, chooser focus
identity), `accessibility_visibility` (inactive and covered controls out of
the accessibility tree; PIN and recordings at the largest text; Back from
modals restores channel identity after reordering) and the
`iptv-nav` `hold` unit tests. Each fix was checked against its regression: with
it reverted (the card and strip as one owner, no cancel on capture, no
consumed key, raw Back acting on a PIN prompt, no swallowed Back, no
deadline-long card, a repeat-decided hold passed as a repeat, Menu repeats
acting; and in the second review round: no guard clear before a prompt, the
cut-short key not consumed, its Back release not swallowed, the card
deadline renewed on Down only, Menu repeats reaching the bar), at least one
of these scenarios fails. Keyboard `M` registers without repeats (unit test
`keyboard_menu_fires_once_while_held_and_moves_repeat`).

## Acceptance criteria

| #87 criterion | Status | Evidence |
|---|---|---|
| Complete synthetic journey: launch → favourites/groups → preview → full screen → controls → browse without retuning → previous channel → back to the guide | Partly (headless and emulator) | `tv_remote_journey` journey scenario and the emulator rows above cover launch, the groups column, full screen, controls, browsing without retuning, previous channel and the return to the guide. Not covered: choosing the Favourites group, adding or playing a favourite, and a preview before the first full screen (`guide.autoplay` is off, so OK plays full screen directly) |
| Down/Repeat/Up phases, long OK release, rapid taps, context changes during a hold | Met headless; partly native | Headless phase scenarios and `iptv-nav` `hold`; emulator long OK (timed and with repeats) and rapid taps. Native context change during a hold not run |
| Back dismisses the nearest surface; a chooser, PIN prompt or settings visit returns to the right content | Met | [Back](../tv-controls.md#back) order; emulator chooser and Back; headless `tv_shared_controls`, `accessibility_visibility` |
| Replay/VOD seeking and track selection by remote keep the player and stream | Partly | Replay: headless `tv_replay` (seek: one archive open; track: none). VOD by remote and any Android replay: not run |
| Focus readable at TV distance and with large text; inactive/covered controls cannot capture input | Partly | Headless extra large text on the bar and `accessibility_visibility`; emulator screenshots at 1080p. Not judged at a real viewing distance |
| Compare with identified Patina/TiviMate builds | Not run | See below |
| Emulator evidence plus a physical Android TV and remote pass | Partly | Emulator above. Physical pass not run |

## Not run

- **Physical Android TV and remote.** No physical TV was used in this run;
  physical devices were deliberately left untouched. TV model, OS,
  resolution, key source and limitations on real
  hardware remain to be recorded.
- **Comparison with a TiviMate build.** Not installed or run here; no
  TiviMate behaviour is claimed. The key table follows Patina's code; the
  reference documentation was used only as a behavioural benchmark, without
  its wording or assets. Premium-gated reference features were not examined.
- A surface change during a hold on the emulator (timing; see above).
- Replay and video on demand on Android (no archive, movies or series in the
  smoke fixture); video-on-demand seek and tracks by remote headless.
- TalkBack speech, system font scale above 1.0 on the emulator, and IR
  remotes that send no repeats (simulated only by holds without repeats).
- Native player surface and frame continuity on Android across Back and
  full screen (only stream-open counts were checked there).
