# Issue #86 journey validation

Evidence for the last acceptance criterion of #86: the complete shared
viewing journey on a fresh current build for TV, tablet and desktop, checked
against the [interaction contract](../interaction.md). Run on 20 November 2026.

The journey: add a source, find TV, movie and series content, start
playback (choosing goes full screen), browse without interruption, and
return to the same content. Also checked: the visible ⋯ "More options"
control, visible Back following the input mode, the compact guide header,
the Movies and Shows empty states, and identity kept across resize and
rotation.

All data is synthetic: the 300-channel generated playlist and XMLTV from
`tools/make-iptv-fixtures.py`, generated HLS test streams from
`tools/make-hls-fixtures.sh`, served by `tools/mock-server.py` on the host,
and the generated catalogues of the headless app scenarios. No provider data
was used.

**Physical devices: none.** Everything below is emulator, headless or a
private desktop display. No result here stands for a physical TV, tablet or
desktop.

## Build

| Item | Value |
|---|---|
| Commit validated | `83277dd2` (master plus PR #126, merged as `f61f34d1`) |
| Android APK | Debug build, validation ID `com.hantekllc.iptvrs.validation`, x86_64, `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true`, development playlist `http://10.0.2.2:8089/iptv/playlist.m3u`, built with `tools/sync-host.sh` and Gradle `assembleDebug` as in `tools/ci/android-smoke.sh` |
| APK SHA-256 | `ad85b8e2d0bfad52c61a21f29433ac888a274e3788ea5a467ead726a5c86a9da` |
| Rust | 1.99.1 (the pinned toolchain) |
| Desktop | `cargo build -p iptv-app` and the `guide_window_preview` example (`--features test-support`), Linux, GTK 4 |

Debug builds show a key-timing line in the guide header ("keys N · … µs");
release builds do not.

## Hosts

| Host | Profile | OS | Input | Viewport | Text and locale |
|---|---|---|---|---|---|
| Android tablet (emulator) | Disposable copy of `tab36` (Pixel Tablet, 2560×1600 at 320 dpi, 1280×800 dp), run `run-aca2cr_l` on `emulator-5580` | Android 16 (API 36) Google APIs x86_64 image, emulator 37.2.13.0, `swiftshader`, no audio | Remote keys (smoke), then touch taps and D-pad keys | Landscape 1280×800 dp; portrait 800×1280 dp | App Medium, system font scale 1.0, en-US |
| Android TV (emulator) | Disposable copy of `tv36` (`tv_1080p`, 1920×1080 at 320 dpi, 960×540 dp), run `run-y28dunxp` on `emulator-5582` | Android 16 (API 36) Android TV x86_64 image, emulator 37.2.13.0, `swiftshader`, no audio | Remote keys, then touch taps | 960×540 dp | App Medium, system font scale 1.0, en-US |
| Desktop (headless) | `cargo test -p iptv-app --features test-support --test app`: 199 passed, 0 failed, 4 ignored (the timing gates CI runs alone) | Linux | Keyboard, pointer, touch and remote events the harness injects | 1920×1080, 1280×720, 960×540, 560×800, 480×800, 480×240, 480×320, 400×600 and others per scenario | Small to Extra large, pseudolocale and translated interfaces (`interface_languages`) |
| Desktop (real window) | `guide_window_preview` with a generated M3U, in a private GTK Broadway display driven by headless Chromium | Linux | Keyboard and pointer | 1280×720, 560×800, 400×600 | Medium in English; Extra large with the test pseudolocale (`IPTV_RS_TEST_PSEUDOLOCALE=1`) |

Each Android run used `tools/ci/disposable-avd.py` with a fresh wiped copy
of the source AVD, recorded `operation.json` with `"result": "passed"`, and
removed its copy. The smoke's new optional hold (`IPTV_SMOKE_HOLD`) kept the
app on the same owned emulator for the touch and rotation checks below, and
its crash and refused-load checks ran after them. Paths such as
`target/android-validation/run-aca2cr_l/smoke/01-first-run.png` are in the
local run directories, which are not committed; only the screenshots linked
from `docs/screenshots/issue-86/` are.

Smoke steps are cited by their names in `tools/ci/android-smoke.sh`: "install
and first start", "add the synthetic playlist", "play a channel in the guide",
"full screen", "shared TV controls and long OK release" (TV only),
"multiview with a second player", "background and resume", "back to the
guide", "refresh during playback", "kill and reopen after a change",
"recreate the activity", "interrupt then recover a restore" and "hardware
Search".

"No new stream" below means the app logged no further `player 0 open` line.
That shows no stream was opened again; it does not show that the native
player surface survived. Surface and frame continuity on Android were not
verified here (the screenshots only show video in the preview afterwards).

The Broadway display has no OpenGL, so the desktop real window shows no
video ("The current backend does not support OpenGL"). Its screenshots show
layout, focus and input behaviour only; video continuity on desktop rests on
the headless scenarios.

## Journey results

Pass means the step behaved as the contract says. Not run means nothing
here checked it.

### Android tablet (`tab36`, emulator)

| Step | Result | Evidence |
|---|---|---|
| Add a source (first run, Add playlist, wizard) | Pass | Smoke "install and first start" and "add the synthetic playlist"; `target/android-validation/run-aca2cr_l/smoke/01-first-run.png` to `03-playlist-added.png` ("300 channels in 24 groups"; run directory, not committed) |
| Find TV content in the guide | Pass | Smoke "play a channel in the guide"; `smoke/04-guide-preview.png` (run directory, not committed) |
| Choosing plays full screen (remote OK) | Pass | Smoke "full screen"; `smoke/05-fullscreen.png` (run directory, not committed) |
| Choosing plays full screen (touch tap on a channel) | Pass | Manual; one `player 0 open` for the tap |
| Visible Back in full screen (touch) | Pass | [tablet-full-screen-back.png](../screenshots/issue-86/tablet-full-screen-back.png) |
| Back returns to the same channel, video shown in the preview | Pass | Tap on ‹ logged `play bar: back`; the guide showed the chosen channel with video in the preview; no new stream. Surface continuity not verified |
| ⋯ visible for touch in the guide, opens the programme and channel menu | Pass | [tablet-guide-touch.png](../screenshots/issue-86/tablet-guide-touch.png); the menu listed programme and channel actions |
| Keys hide ⋯ and keep its room; Back stays visible on a tablet | Pass | Manual D-pad after touch: focus highlight shown, ⋯ hidden, title unmoved, ‹ kept |
| Find movies and series: Movies with an M3U-only library | Pass | [tablet-movies-empty-state.png](../screenshots/issue-86/tablet-movies-empty-state.png): explains M3U has live channels only, offers Add playlist |
| Close Movies returns to the same channel | Pass | Manual |
| Rotation to portrait keeps channel and focus, opens no stream | Pass | [tablet-portrait-compact-header.png](../screenshots/issue-86/tablet-portrait-compact-header.png): compact header with the shortcuts on their own row, video in the preview; no new stream during rotation. Surface continuity not verified |
| Background, resume, kill, recreate, restore: same channel by identity | Pass | Smoke "background and resume" through "interrupt then recover a restore" |
| Movie and series playback, resume | Not run on Android | The smoke fixture is an M3U playlist with no movies or series; only the desktop headless scenarios cover this |
| Split screen | Not run | |
| Keyboard and mouse on the tablet | Not run on Android | Headless `pointer::tablet_back_controls`, `tv_shared_controls::a_tablet_shows_the_more_control_before_any_tap` |

### Android TV (`tv36`, emulator)

| Step | Result | Evidence |
|---|---|---|
| Add a source | Pass | Smoke "install and first start" and "add the synthetic playlist" |
| Choosing plays full screen (remote OK) | Pass | Smoke "full screen" and a manual OK on another channel |
| Long OK opens the controls without a short-OK action | Pass | Smoke "shared TV controls and long OK release" |
| Back returns to the guide on the same channel | Pass | Manual: `key=back … ctx=Player action=Some(Back)`, then the guide on the chosen channel with video in the preview. Surface continuity not verified |
| No ⋯ and no visible Back with the remote | Pass | [tv-guide-remote.png](../screenshots/issue-86/tv-guide-remote.png): the programme title uses the ⋯'s room |
| Touch on a TV shows ‹ and ⋯ | Pass | [tv-guide-after-touch.png](../screenshots/issue-86/tv-guide-after-touch.png) |
| Touch: tap a channel plays full screen; the player's "TV guide" returns to the same channel | Pass | Manual |
| Movies with an M3U-only library, remote and touch | Pass | Same message and Add playlist; remote Right from the categories focuses the action; Close shows only after touch |
| Background, resume, kill, recreate, restore | Pass | Smoke "background and resume" through "interrupt then recover a restore" |
| Movie and series playback | Not run on Android | The smoke fixture is an M3U playlist with no movies or series |

### Desktop

| Step | Result | Evidence |
|---|---|---|
| Add a source (wizard) | Pass (headless) | `cov_settings_wizard::the_wizard_and_the_choosers`; the real-window run used a seeded M3U source |
| Choosing plays full screen: keyboard Enter, pointer click | Pass (real window, headless) | Real window: Enter and a click on a channel each went full screen with the play bar; headless `personalization_overlay`, `cov_nav`, `cov_nav_touch` |
| Escape, then the visible ‹, return to the same channel | Pass (real window) | First Escape hid the banner, second returned to the guide on the chosen channel (▸); a click on ‹ did the same |
| ⋯ for pointer, hidden for keys | Pass (real window, headless) | Shown after a click, absent while keys drove it; `tv_shared_controls::touch_and_pointer_reach_the_menu_without_a_shortcut` |
| Compact header at 560×800 and 400×600 | Pass | Shortcuts on their own row under the preview, channel kept; at 400×600 the row scrolls sideways (Add playlist starts off screen) |
| Resize keeps channel, preview, title and episode on one player, no extra opens | Pass (headless) | `pane_collapse::resizing_the_guide_keeps_the_focused_channel_and_the_playing_preview`, `pane_collapse::resizing_movies_and_shows_keeps_the_title_and_the_episode` |
| Movies and Shows: play, resume by identity | Pass (headless) | `vod_library::movies_and_series_reach_playback_and_resume_by_identity`, `desktop_clicks_through_continue_watching_and_recently_added` |
| Empty, failed and disabled imports with their actions | Pass (headless) | `vod_library::desktop_retries_a_failed_import_and_refreshes_an_empty_one`, `an_import_switched_off_opens_that_playlists_import_switch`, `the_empty_state_covers_every_playlist_the_catalogue_reads` |
| Movies empty state at 400×600 for pointer | **Fail** | [desktop-400x600-movies-empty-squeezed.png](../screenshots/issue-86/desktop-400x600-movies-empty-squeezed.png) (English, Medium text); bug 1 below, fixed since by #130 (headless only) |
| Extra large text with the pseudolocale, 1280×720 | Pass | [desktop-1280x720-extra-large-pseudo.png](../screenshots/issue-86/desktop-1280x720-extra-large-pseudo.png): shortcuts and preview visible |
| Large text and translations keep accessible names | Pass (headless, Settings only) | `interface_languages::pseudolocalized_labels_keep_their_accessible_name_at_large_text_sizes` walks the Settings screen; it does not cover the journey's guide, player or Movies actions |
| Video in the real window | Not run | Broadway has no OpenGL |

## Bugs and observations

1. **Movies and Shows empty state is unusable at 400×600 for pointer and
   touch.** Repro: desktop window 400×600, M3U-only library, click Movies.
   Seen in English at Medium text (the committed screenshot) and again with
   Extra large text in the pseudolocale (not committed). The categories column keeps its full width; the message
   column shrinks to about 60 points, wraps a word or a few letters per line,
   clips its title, and puts Add playlist below the window. Keyboard users
   can reach it: Right moves focus to the action and the categories slide
   away. At 560×800 it fits. This conflicts with "Movies and Shows already
   show one pane at a time in every class" and with the large-text criterion.
   **Fixed by #130** after this run: in Compact the empty catalogue is one
   pane, its message and action beside the narrow rail with the categories
   gone. `vod_empty_compact` checks at 400×600, Medium and Extra large text,
   (and Extra large with the test pseudolocale) that the message keeps most
   of the width and Add playlist and Close stay inside the window, also with
   the remote on the rail, and that Left, Right and Enter still reach the
   rail and the action. The real window was not run again.
2. **Observation, not a bug yet:** with the guide's groups column open at
   400×600 and Extra large text, the rail and groups take most of the width
   and the guide rows show two letters. The contract marks collapsing these
   by class as Planned.
3. **Observation:** the Movies screen's own rail lists Live TV, Movies, Shows
   and Recordings, while the guide's rail also has Search, Watchlist,
   Playlists and Settings. "The same destinations in every layout class" is
   Planned; this records the current gap.
4. **Observation:** at 400×600 the compact header's shortcut row scrolls
   sideways without a scroll bar, so Add playlist is reachable but only half
   of a label hints that more follows.
5. **Observation:** the test pseudolocale (`IPTV_RS_TEST_PSEUDOLOCALE=1`)
   expands only the app's own strings. Strings translated through
   `iptv_core::i18n`, such as the guide's "No information", stay in English
   in the pseudolocale screenshot, so it does not test their length.

No crash or refused player load was seen on either emulator, and no stream
was opened by a rotation, a Back or closing Movies.

## Acceptance criteria

| #86 criterion | Status | Evidence |
|---|---|---|
| Document the route, activation and pane-collapse contract | Met | [Interaction contract](../interaction.md) |
| Discoverable destinations and actionable empty, loading and error states | Partly (bug 1, fixed by #130; observation 3) | Emulators show the M3U-only state with Add playlist; headless covers in progress, failed, disabled and empty with their actions |
| Resize, rotation, split screen and input switching keep identity, position and playback, with no extra opens | Partly | Resize: headless `pane_collapse`, including the same player node. Rotation and touch/key switching: tablet emulator kept the channel and opened no stream; surface continuity not verified. Split screen: not run |
| Large text and long translations leave primary actions visible or reachable | Partly | Met at 1280×720 in the real window; headless `interface_languages` covers Settings only; bug 1 at 400×600 (fixed by #130, headless `vod_empty_compact`); observation 5 |
| A tablet with keyboard and mouse keeps touch controls and no permanent remote hints | Partly | Headless `pointer::tablet_back_controls` and the tablet ⋯ scenarios; on the emulator touch and D-pad alternated correctly. No hardware keyboard and mouse run |
| Complete journey on fresh TV, tablet and desktop installations, with build, OS, input, viewport and synthetic screenshots | Partly | Fresh emulator installs and the headless and private-window desktop runs above. Not run: physical devices, desktop video in a real window, movie and series playback on Android |

## Not run

- Physical TV, tablet, phone or desktop hardware.
- iPad, macOS and Windows.
- Android split screen, a hardware keyboard or mouse on the tablet,
  TalkBack, and system font scale above 1.0.
- Movie and series playback on Android: the smoke fixture is an M3U playlist.
- Native player surface and frame continuity on Android across rotation and
  Back.
- Video in the desktop real window.
