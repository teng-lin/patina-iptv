# Issue #89 desktop journey validation

Evidence for #89: the everyday desktop workflow with a mouse, a trackpad and
a keyboard in resizable windows, checked against
[desktop viewing](../desktop.md) and the
[interaction contract](../interaction.md). Run on 20 November 2026.

The journey: a seeded source, search, watch (choosing goes full screen),
browse, a second player and its compact picture-in-picture, return, in a
normal and a narrow side-by-side window; then recordings, closing and
display recovery.

All data is synthetic: the generated playlist of the
`guide_window_preview` example (three "Synthetic" channels and twelve
generated channels in each of three groups), the generated HLS test streams
of `tools/make-hls-fixtures.sh` served from `127.0.0.1:18091`, and the
generated playlists, catalogues and recordings of the headless scenarios.
No provider data was used.

**Physical hardware: one Linux desktop, used only to host the runs below.**
No native desktop session, real display, real multi-monitor arrangement or
physical input device of the host took part; the real window ran in a
private GTK Broadway display. The macOS results come from a separate
native run on a Mac with two displays ([macOS run](#macos-run)); nothing
here stands for Windows.

## Build

| Item | Value |
|---|---|
| Base commit | `e83c27e8` (master) plus this change on `feat/issue-89-desktop-journey` |
| Rust | 1.99.1 (the pinned toolchain) |
| Desktop app | `cargo build -p iptv-app`; the real window ran the `guide_window_preview` example (`--features test-support`), debug build |
| Headless | `cargo test -p iptv-app --features test-support --test app` |

## Hosts

| Host | OS and stack | Window manager and display | Input | Window sizes | Text and scale |
|---|---|---|---|---|---|
| Linux, real window | Linux 7.2 (Arch), GTK 4.24.1 | Private GTK Broadway display `:17` (`gtk4-broadwayd`), shown in headless Chromium 155 and driven over the DevTools protocol; no window manager, no OpenGL, one virtual screen | Synthetic mouse moves, clicks, double clicks, right-clicks, vertical wheel, Shift + wheel and keys | 1024×713 and maximized 1280×763 (points), and 488×713 after dragging the window's edge | Medium, scale 1, English |
| Linux, headless | The app's headless engine (`nsh-testing`), Os Linux, idiom Desktop | Headless window model: main window plus player windows, one 1920×1080 display | Pointer, wheel, keyboard and window-focus events the harness injects | 1280×720, 1920×1080, 480×800, 400×600 | Pixel ratio 1, 1.25 and 2; font scale 1 and 1.6 |
| macOS (native, commit `98a17b9f`) | macOS 26.6.2 (25G83), Xcode 27.2 (27B52), Rust 1.99.1, Apple silicon (host `skywalker`) | Native AppKit; two displays: primary 3008×1692 points at 2×, usable y 90–1662; portrait secondary 1692×3008 points at 2× at (3008, −309) | Headless scenarios' injected input; the real window was moved by System Events only (no pointer or keyboard input: TCC blocks it over SSH) | Real window 1280×882 at (864, 203) | Scale 2 |
| Windows | Not run: no Windows host here | | | | |

Broadway limits what the real window shows: it has no OpenGL, so no video
("The current backend does not support OpenGL") and the player never
reports a frame. Its stall recovery therefore reopens the stream about every
15 seconds; every later `player 0 open` line in the log follows a
`retry N` line, and none followed a resize, a wheel, a menu or Escape.
Broadway sends only up/down wheel steps, has no window manager (the
operating system's full screen is requested and logged but not shown, and a
window cannot be dragged by its title bar), and turns a press in a
frameless compact window into a window drag. Those checks rest on the
headless scenarios below.

## Journey results

Pass means the step behaved as [desktop viewing](../desktop.md) says. The macOS
column was run natively at `98a17b9f`, before the review fixes listed under
[Review fixes](#review-fixes); the Linux columns cover them.

| Step | Linux real window | Linux headless | macOS (`98a17b9f`) |
|---|---|---|---|
| Vertical wheel over the guide moves through the channels; time stays | Pass: [desktop-wheel-scrolls-channels.png](../screenshots/issue-89/desktop-wheel-scrolls-channels.png) | Tunes nothing (`desktop_workflows::the_wheel_right_click_and_hover_never_tune_and_every_menu_has_a_visible_way_in`); the screen model does not place the rows' `sticky` list, so it cannot show the move | Headless scenario passed; no real pointer input |
| Shift + wheel or a sideways swipe moves through time; channels stay | Pass (Shift + wheel): [desktop-shift-wheel-pans-time.png](../screenshots/issue-89/desktop-shift-wheel-pans-time.png), Now appears | `guide_nav` (sideways scroll through listings) | Headless scenarios passed; no real pointer input |
| Hover changes no channel | Pass: no `player 0 open` while moving over rows | Pass (`the_wheel_right_click_and_hover_…`) | Headless scenario passed |
| Right-click opens the channel menu without tuning; Escape closes it | Pass | Pass (Back; the screen model's Escape needs a navigation stack) | Headless scenario passed |
| ⋯ on the shown group in the guide's groups column; group options; Escape | Pass: [desktop-group-more.png](../screenshots/issue-89/desktop-group-more.png) | Pass, and keys hide it | Headless scenario passed |
| ⋯ and right-click on the channel list's groups column and rows | Not run | Pass (`window_full_screen_resizing_and_display_scale_keep_the_stream_and_the_channel`) | Headless scenario passed |
| Recordings: ⋯ on each row, right-click, wheel | Pass (⋯, right-click and Escape on one generated partial recording): [desktop-recording-more.png](../screenshots/issue-89/desktop-recording-more.png) | Pass with 36 generated recordings, including the wheel | Headless scenario passed |
| Posters: wheel scrolls, right-click plays nothing | Not run (the example has no movies) | Pass (`the_wheel_scrolls_posters_and_a_right_click_on_one_plays_nothing`) | Headless scenario passed |
| Ctrl+F, then typing `m`, Space, `f`, `=`, `-`, `[`, `]`, Backspace and arrows edits the query; nothing zaps | Pass: [desktop-search-typed-shortcut-characters.png](../screenshots/issue-89/desktop-search-typed-shortcut-characters.png); the only key action logged was `OpenSearch` | Pass, with paste (`typing_shortcut_characters_in_a_field_edits_it_and_controls_no_player`) | Headless scenario passed |
| Clicking a search result plays it full screen in the window, not the OS window | Pass | Pass | Headless scenario passed |
| Double click on the video: the OS full screen, no second play | Requested and logged (`iptv window: full screen true`), not shown (no window manager); no extra open | Pass: main window FullScreen, opens unchanged | Headless scenario passed |
| Escape steps back: OS full screen, panels, then the guide on the same channel | Pass (guide on Music 03 ▸) | Pass | Headless scenario passed |
| Narrow side-by-side window keeps the channel and focus | Pass: [desktop-narrow-guide-488.png](../screenshots/issue-89/desktop-narrow-guide-488.png) | Pass at 480×800 and 400×600, full screen (`window_full_screen_…`) and in the guide (`pane_collapse`) | Headless scenario passed |
| Display scale and larger text keep the stream | Not run | Pass (pixel ratio 2, 1.25, 1; font scale 1.6) | Headless scenario passed |
| Watch in new window: the second player opens muted and says so | Pass: title "Music 05 · muted", [desktop-second-player-muted-title.png](../screenshots/issue-89/desktop-second-player-muted-title.png) | Pass (`keys_follow_the_focused_window_and_focus_never_moves_the_sound`) | Headless scenario passed |
| Keys follow the focused window; focus never moves the sound | Pass: Space in the second window logged `ctx=Player` for its own input (n=1) | Pass: Space, F and O in each window; sound unchanged by focus | Headless scenario passed |
| The second window's Mute takes the sound; its title drops "muted" | Pass: [desktop-second-player-has-sound.png](../screenshots/issue-89/desktop-second-player-has-sound.png) | Pass; the main player is muted | Headless scenario passed |
| Compact picture-in-picture and back, same session | Pass by Escape (`iptv pip: BackToApp`); the compact ⇱ press became a window drag in Broadway | `guide_player_windows` | `guide_player_windows` passed |
| Closing a player window stops only its stream | Pass: the main guide kept Music 03 ▸ | `guide_player_windows` | `guide_player_windows` passed |
| Closing the main window during recordings closes every player window; the recorder runs on | Not run (the one real recording had ended) | Pass (`closing_the_main_window_during_recordings_ends_every_viewing_window`; fails with the change reverted) | Headless scenario passed |
| A window off a removed display comes back onto a remaining one | Not run (one virtual screen; GTK reports no positions) | Core policy: `iptv_core::playback_windows` tests; the headless model has one display | Partly: `reachable` core tests passed; the real window made no move at launch and stayed where System Events put it (primary, the portrait secondary at (3100, −400), and back). Display removal not run |

The second player in the real window showed "Can't play Music 05 · No
provider connection available": the example's playlist allows one
connection and the main player held it. That is admission working, not a
fault; the headless scenarios raise the limit and play both.

## Acceptance criteria

| #89 criterion | Linux | macOS | Windows |
|---|---|---|---|
| Setup → search → watch → browse → compact/PiP → return with mouse and keyboard, including a narrow side-by-side window and large text | Partly: real window for search, watch, browse, a second player, compact and return, narrow 488 points; large text headless only | Partly: headless journey scenarios passed natively; the real window started and showed the guide (first frame +621 ms). No real pointer or keyboard input, no screenshots | Not run |
| Resize, DPI and full-screen transitions keep source, session, browse anchor and progress; presentation opens no stream | Met headless (same player node, source and opens through OS full screen, four sizes, three scales, large text; `pane_collapse` for the guide and Movies and Shows); real window shows no opens except stall retries | Met headless (205 app scenarios passed natively, `desktop_workflows` included) at scale 2; no video in the real window | Not run |
| Wheel, trackpad and right-click consistent over the guide, channel list, VOD cards and recordings; every secondary action has a visible entry point | Met: real window for the guide, groups and recordings; headless for all four. Horizontal swipes in a real window: Shift + wheel only (Broadway) | Met headless; no real pointer or trackpad input | Not run |
| Typing shortcut characters, editing fields and switching focused windows cannot zap, close or control the wrong player | Met: real window and headless | Met headless; no real keyboard input | Not run |
| Two players show channel and audio ownership; return, close, stop and main-window close during a recording follow one documented contract | Met: titles in the real window; [closing contract](../desktop.md#closing); main close during recordings headless only | Met headless | Not run |
| Monitor removal and reconnection leave windows recoverable; host limits have fallbacks | Partly: core policy and [host table](../desktop.md#displays-and-hosts); on Linux the window manager or compositor keeps windows reachable (GTK 4 has no position API). No real multi-monitor run | Partly: `reachable` tests passed; the real window was not moved at launch and stayed where it was put on either display. Display removal and a missing saved display not run | Not run |
| Native Linux and macOS journey evidence; Windows where changed | Linux: Broadway real window and headless, above | Partly: native build and headless scenarios; real window launch and placement only | Not run: Windows is changed only by the main window's display recovery |

## macOS run

Host `skywalker`, commit `98a17b9f` (before the review fixes):

- `cargo test -p iptv-core`: 408 passed, the four `reachable` tests among them.
- `cargo build -p iptv-app`: passed, no warnings.
- `cargo test -p iptv-app --features test-support --test app`: 205 passed,
  0 failed, 4 ignored; all six `desktop_workflows` scenarios passed.
- `cargo test -p iptv-app --lib`: 136 passed.
- Real window: the `guide_window_preview` example with its synthetic seed
  (39 channels) and `tools/mock-server.py`. It started cleanly, mpv drew
  through `CAOpenGLLayer`, and the first guide frame came at +621 ms. The
  window opened at (864, 203), 1280×882, inside the primary's usable area;
  `keep_reachable` moved nothing at launch. System Events moved it to
  (200, 150), onto the portrait secondary at (3100, −400) and back; it stayed
  where it was put each time, with no move by the app.

## Review fixes

Made after the first review of #133; Linux headless and core tests cover
them, the macOS run above predates them.

- `reachable` could panic in `f32::clamp` when a window as large as the
  remaining display had a fractional origin (scaled displays); it now keeps
  the bounds ordered, and returns nothing for a window with no size
  (`fractional_origins_and_oversized_windows_never_panic`).
- Display recovery now also runs when a window first reports its state and
  when it comes back from minimized or full screen, not only when displays
  change (`desktop::tests::only_a_plain_window_off_every_display_moves`;
  the trigger itself needs a host that reports positions, macOS or Windows).
- Closing the main window during recordings now stops playback as the
  viewer's Stop does, so choosing the same channel after reopening plays it
  (`closing_the_main_window_during_recordings_ends_every_viewing_window`
  reopens and chooses it, with and without the main window's
  picture-in-picture; it fails with the old stop).
- Moving the mouse after keys now switches to Pointer everywhere, not only
  over full-screen video, so the ⋯ controls come back; a sideways wheel in
  the guide does too. Focus and playback are untouched.
- The player-window scenarios are compiled for Linux and macOS only, where
  "Watch in new window" exists; the others check a menu item every desktop
  has.
- A groups column keeps no room for a ⋯ that multiview picking hides.

## Bugs and observations

1. **Observation:** in a 488-point window, Back from the guide to the groups
   column and rail shows both at full width, leaving the guide rows a few
   letters wide until focus returns to the guide. Collapsing the rail and
   groups column by class is still Planned in the
   [interaction contract](../interaction.md#layout-classes) (#86
   observation 2).
2. **Observation:** the headless screen model goes back on Escape only where
   a navigation stack or modal is mounted, so the scenarios close menus with
   the platform's Back. Escape closed every menu in the real window.

## Not run

- macOS (run separately on host `skywalker` at `98a17b9f`): real pointer
  and keyboard input and screenshots (TCC blocks them over SSH); removing
  or rearranging a real display (it would disturb that desktop); a launch
  restored onto a missing display (no saved main-window bounds to plant);
  video in the real window; the review fixes below.
- Windows: no Windows host.
- Real multiple displays: removal, reconnection and mixed scale.
- A native Linux desktop session (X11 or a Wayland compositor such as
  Hyprland): the run used a private Broadway display so the host's own
  desktop was never touched, which means no window manager, no OpenGL video,
  no horizontal wheel and no operating-system full screen.
- Physical trackpad gestures and a screen reader.
