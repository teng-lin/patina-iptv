# Issue #138 iPad portrait and multitasking validation

Evidence for #138: the iPad app runs in portrait and in resizable windows
(Split View, Slide Over and Stage Manager in earlier iPadOS, windowed apps
in iPadOS 26 and later), keeping the channel, playback and selection by
identity, with no extra stream and the one player view never hidden,
detached or re-parented. Run on 20 November 2026.

All data is synthetic: the 300-channel generated playlist and XMLTV from
`tools/make-iptv-fixtures.py --channels 300 --host 127.0.0.1:8138`, generated
HLS test streams from `tools/make-hls-fixtures.sh --android-smoke`, served by
`tools/mock-server.py 8138` on the Mac (every request logged: the stream
counts below are its `index.m3u8` requests), and the generated playlists,
guides and on-demand catalog of the headless scenarios. No provider data
was used.

**Physical iPad: none.** Everything below is the iPad simulator or
headless; a physical iPad run is still to be recorded.

## Configuration

`crates/iptv-app/nsh.json` now lists all four orientations for
`UISupportedInterfaceOrientations` and `UISupportedInterfaceOrientations~ipad`
and no longer sets `UIRequiresFullScreen`. NSH copies `ios.infoPlist` over its
own defaults (`nsh-cli` `package.rs`), so every key needed here is
expressible. The packaged `Info.plist` was checked with `plutil -p`: the
four orientations under both keys, no `UIRequiresFullScreen`, a
`UILaunchScreen` entry, `UIApplicationSupportsMultipleScenes` false (NSH's
reserved key: one window, which is all multitasking beside another app
needs). NSH was not changed.

## Build and hosts

| Item | Value |
|---|---|
| Branch | `feat/issue-138-ipad-portrait-split` (the simulator app was built from commit `cc7acc6c`, before the review fixes 5–7 below, which are checked headless only) |
| Simulator app | `nsh package . --target ios-simulator` (nsh at the pinned rev, built on the Mac), `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true`, development playlist `http://127.0.0.1:8138/iptv/playlist.m3u`, Keychain entitlements embedded at link time (`-sectcreate __TEXT __entitlements`, the #136 workaround) |
| Mac | macOS 26.6.2, Xcode 27.2, iOS simulator runtime 27.2, Rust 1.99.1 |
| Simulator | Disposable "Patina 138 Synthetic", iPad Air 11-inch (M4), 820 × 1180 points; deleted after the run |
| Input | `idb ui tap`, `idb ui swipe` (window corner drags), `idb ui rotate`, `idb ui button HOME`, `xcrun simctl ui content_size`, Settings changed by taps |
| Headless | `cargo test -p iptv-app --features test-support --test app ipad_windows`: environment `Idiom::Pad`, `Os::Ios`, Linux |

## Criteria and evidence

| #138 criterion | Headless (`tests/app/ipad_windows.rs`) | iPad simulator |
|---|---|---|
| Portrait and landscape keep the channel, playback and selection by identity | `ipad_guide_keeps_the_focused_channel_and_the_preview_in_every_window`: 1180×820, 820×1180, 678×820, 507×820, 375×820, 320×1180, 700×560 at text 1.0, 1.35 (xxxLarge) and 2.35 (AX3): the focused channel (C) stays focused, the previewed channel (B) plays on, no open, the video under the preview by `GuideHeader`'s policy | Met. Launched in portrait ([guide](../screenshots/issue-138/ipad-portrait-guide.png)); played channel 1 full screen ([portrait](../screenshots/issue-138/ipad-portrait-full-screen.png)); rotated to landscape left ([landscape](../screenshots/issue-138/ipad-landscape-full-screen.png)), portrait upside down and back: same process, still channel 1 full screen, **1 stream request before and after** |
| 1/3, 1/2 and 2/3 Split View keep the channel, playback and selection | Same scenario (375, 507 and 678 points wide, 320 for the narrowest window); `ipad_full_screen_and_details_keep_progress_and_episode_in_every_window`: a movie at 37 s of 2 min stays full screen with its progress, a show's details keep the selected episode, at every window and text size | Met with iPadOS 26 windows (Settings › Multitasking & Gestures › Windowed Apps; the system has no separate Split View there): the window dragged from 820×1180 to 390×844 kept the guide on channel 1 playing in the preview ([390-point guide](../screenshots/issue-138/ipad-window-390-guide.png)); widened to 820×1057 and narrowed to 390×1057, **no stream request** from any resize |
| Primary controls reachable at large text | Every window above at text 1.35 and 2.35: the play bar's ‹ and play/pause and **every** bar button inside the window; the picture size chooser at 375 points (text 1.0 and 2.35) inside the window and above the wrapped bar; the guide header's title and times whole inside their column (the header grows for them); the guide's ‹ clear of a 65-point leading inset (an iPad window's controls); Close (or the category heading that brings it back) inside; the empty Movies message and Add playlist inside; Search's field inside with the query kept; the header's shortcuts row inside, scrolling sideways | Met at AX3 (Accessibility Extra Large) in portrait: the bar's buttons and times ([play bar](../screenshots/issue-138/ipad-portrait-ax3-play-bar.png)), the guide header with "15 min left" wrapped onto its own line ([guide](../screenshots/issue-138/ipad-portrait-ax3-guide.png)). At 390 points the bar wraps its right-hand buttons under the transport ([390-point bar](../screenshots/issue-138/ipad-window-390-play-bar.png)); at 820 it is one row ([820-point bar](../screenshots/issue-138/ipad-window-820-play-bar.png)) |
| No extra stream on rotation or resize; the player view never hidden, detached or re-parented | Every step of all four scenarios: one `tv.player` mounted once, never removed, inserted once, never hidden; the stream sources unchanged, and through the full-screen resizes the player's open count and the source's token too (a reopen of the same movie would change them) | Met for streams: 0 requests from 3 rotations, 3 window resizes and 2 text-size changes (mock log). The view's identity is not observable from the simulator; the headless record covers it |
| The multitasking lifecycle follows the documented background policy | `ipad_another_app_beside_changes_nothing_and_a_reauthorized_return_reopens_once`: the scene inactive (another app focused beside) at 375 and 820 points wide: not paused, no open, same stream. The app's side of the return only: the test injects the native player's `reauthorize` after foregrounding and sees exactly one reopen of the same channel on the same player. No native player runs headless, so the stop in the background and the request on return are verified on the simulator (the mock server's counts), not here | Met. Home: same process returned (pid unchanged), **1 new stream request** (the reopen after fresh admission). Settings over the whole screen backgrounded Patina; Settings made small beside it: Patina visible again with **1 reopen**, then 10 s with Settings focused and Patina inactive: **no request**, video shown ([beside Settings](../screenshots/issue-138/ipad-inactive-beside-settings.png)) |
| Evidence on the iPad simulator in a validation doc; a physical iPad separately | This document | Simulator: this document. Physical iPad: **not run** |

Stream counts in order (mock server `index.m3u8`): channel 1 opened (1);
rotations and AX3 (1); Home and return (2); a tap on channel 2 in the
390-point window, a deliberate choice (3); Settings covering then beside
Patina (4); 10 s inactive beside Settings (4).

## Fixed in this change

1. **The play bar ran past the window's edge in narrow windows.** Its right
   group reserved room for every button on one row, so in a Split View or
   Slide Over window the last buttons were cut off (#88's observation 2). A
   window narrower than the bar's one-row width now wraps that group onto
   the rows under the transport (`play_bar.rs`). Headless without the fix:
   `play-bar-guide` at x 662–710 in a 678-point window.
2. **The guide header's times and minutes left ran past the window** at
   large text in a narrow window (375 points at AX3: "min left" ending at
   398). Each part now wraps within the column (`header.rs`).
3. **Dynamic Type did not count toward the layout classes.** The classes
   and the guide header measured the app's own text size only; iOS Dynamic
   Type and Android's font scale enlarge every text, so a portrait iPad at
   AX3 stayed Medium. They now use the app's size times the system's
   (`settings::text_factor`).
4. **A headless iPad laid the guide out at the desktop's shrinking scale.**
   The scale now also keys on the reported idiom, so a tablet keeps the full
   sizes headless as it does on the device (`scale.rs`; release builds are
   unchanged: Android and iOS never scaled).
5. **Choosers over the video ignored the wrapped play bar.** They kept
   clear of a one-row bar's height, so in a narrow window a tall chooser
   covered the wrapped buttons (375 points at AX3: the picture size chooser
   ended at 692, the bar began at 594), and a 380-point chooser was wider
   than a 375-point window. The bar now reports its laid-out height
   (`Ui::play_bar_height`), and the chooser is never wider or taller than
   its room, its list scrolling (`chooser.rs`).
6. **The header clipped the program's title and times** in a short
   window at large text (700×560 at AX3: 64 points for the title and two
   lines of times). The header now budgets the title and the times, wrapped
   as the column's width makes them, before the description
   (`iptv_core::layout::details_height`), growing up to 55% of the window as
   before, or 70% at the accessibility text sizes.
7. **iPadOS 26 window controls covered the guide preview's ‹** in a window
   ([390-point guide](../screenshots/issue-138/ipad-window-390-guide.png),
   before this fix). It now moves by the part of the leading safe-area
   inset that overlaps the preview, never past the preview's right edge
   (`header.rs`; no ancestor pads the header's sides). Checked headless
   with a 65-point inset and by unit tests; not re-run on the simulator.
8. **Android tablets in split screen (#163).** The same play bar wrap fixes
   the physical Galaxy Tab A9+ report (half of an 800 × 1280 dp screen at
   font scale 1.15: the last buttons ran off the window).
   `tablet_journey::a_split_screen_tablet_keeps_every_play_bar_button_inside_the_window`
   checks every bar button inside 400 × 1280, 800 × 640 and 640 × 800 dp
   windows at 1.15, headless; the physical tablet is not re-run here.

## Observations, not fixed here

1. **At AX3 the guide's rows truncate hard** (channel names "Sy…", short
   programs "…"); the times under the bar's progress wrap one character
   per line in portrait ("12:4 / 0 / PM"). Readable but cramped.
2. **Widening Movies after a compact window keeps the categories closed**:
   the one-pane empty catalog closes them, and `iptv_core::vod` keeps them
   closed in a wider window with the action focused (a tested remote rule).
   Touch then reaches Close through the category heading, one tap away.
3. **The keyboard focus cannot reach the preview on iPad** (Up from the top
   row stays there): the preview takes the focus on TVs and desktops only,
   by design (`lists::navigate`).
4. The development build shows the key-timing line in the header
   (`keys 0 · …`), as debug builds do everywhere.

## Could and could not drive

- **Driven:** rotation to all four orientations (`idb ui rotate`), Dynamic
  Type (`simctl ui content_size`), Home and return, iPadOS 26 windowed
  apps (enabled in Settings by taps), window resizing by dragging its
  corner with `idb ui swipe`, a second app's window over and beside Patina.
- **Not driven:** iPadOS 26 has no Split View or Slide Over mode separate
  from windowed apps, so the 1/3, 1/2 and 2/3 Split View widths are covered
  by windows dragged to similar widths (390 and 820 points) and by the
  headless widths; Stage Manager was not enabled; window tiling by the
  window menu was not tried; a hardware keyboard and the audio session
  (calls, route changes: #88 delivered Android's only, iPad's remains
  Planned in `docs/interaction.md`) were not exercised.
- The app's own log lines (`iptv mpv 0: background, the stream stops`) do
  not reach the simulator's unified log; the stream stop and reopen are
  shown by the mock server's requests and the unchanged process id.
