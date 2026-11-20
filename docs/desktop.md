# Desktop viewing

How Patina behaves with a mouse, a trackpad and a keyboard in resizable
windows on Linux, macOS and Windows (#89). It implements the shared
[interaction contract](interaction.md) and adds what only the desktops have:
the wheel, keyboard shortcuts, several windows and several displays.

Each rule is marked **Now** (implemented) or **Planned**, as in the contract.
[Validation](validation/issue-89-desktop-journey.md) records which host each
rule was checked on.

## Pointer and wheel

| Where | Vertical wheel or two-finger swipe | Horizontal swipe, or Shift + wheel | Right-click | Visible way in |
|---|---|---|---|---|
| Guide | Moves through the channels | Moves through programme time | Programme and channel menu | ⋯ after the programme's title |
| Guide's groups column | Scrolls the groups | Nothing | Group options | ⋯ on the shown group's row |
| Channel list over the video | Scrolls the channels | Nothing | Channel menu | ⋯ at the end of each row |
| Channel list's groups column | Scrolls the groups | Nothing | Group options | ⋯ on the shown group's row |
| Movies and Shows posters | Scrolls the posters | Nothing | Nothing (no menu yet; #90) | — |
| Recordings | Scrolls the recordings | Nothing | The recording's actions | ⋯ at the end of each row |

- **Now:** all rows above (`crates/iptv-app/src/grid/wheel.rs`,
  `channels_view.rs`, `panels.rs`, `recordings_view/rows.rs`). A wheel, a
  right-click or a ⋯ never plays, tunes or stops anything.
- **Now:** hovering never changes the channel or the focus that playback
  follows. Moving the mouse over full-screen video shows the player's
  controls for a while.
- **Now:** the ⋯ controls show while touch or a pointer drives the app and
  hide for keys, which have Menu (M) and Shift+F10. A groups column shows
  its ⋯ only on the shown group, so a narrow column keeps its names: a
  click shows another group, whose row then has it.

## Keyboard

One compact set, the same on every desktop; `Mod` is Ctrl on Linux and
Windows and Command on macOS.

| Keys | Does |
|---|---|
| Arrows, Enter | Move and choose; Enter on a channel plays it full screen |
| Escape or Backspace | One step back ([Getting back](interaction.md#getting-back)) |
| / or Mod+F | Search |
| M, Shift+F10 | Menu for the focused programme, channel or group |
| Space | Play or pause |
| = and - | Channel up and down (a page in the guide) |
| P | Previous channel |
| N | Back to live |
| [ and ] | Rewind and fast-forward |
| I | Information |
| G | Back to the guide |
| Home | The programmes on now |
| Shift+← and Shift+→, Mod+← and Mod+→ | An hour, a day through the guide |
| 0–9 | Channel number |
| Ctrl+M | Mute or unmute |
| Ctrl+↑ and Ctrl+↓ (Cmd on macOS) | Volume |
| F | Full screen in Patina's window, and back (main window) |
| F11 (Linux, Windows), Ctrl+Cmd+F (macOS) | The operating system's full screen for the window |
| Mod+, (macOS) | Settings |

- **Now:** the set above (`crates/iptv-app/src/input/keys.rs`,
  `input.rs`). macOS also has its menu bar (`desktop.rs`); Linux and
  Windows draw none.
- **Now: a shortcut belongs to the window that has the keyboard focus.**
  Keys pressed in a second player window control that player only, and the
  main window's keys its own. The view keys F and O exist in the main
  window only.
- **Now: text fields keep their keys.** While a field edits (search, a
  playlist address, a PIN), letters, digits, Space, /, =, -, [, ], the
  arrows, Home, Backspace, selection and the system's copy, cut and paste
  go to the field and no shortcut fires; Ctrl+↑/↓ stay with the field too.
  A Ctrl shortcut the field does not use, such as Ctrl+M, still acts on the
  focused window's player. Typing never zaps, closes or pauses anything.

## Full screen

- **Now:** clicking a channel plays it full screen *inside* Patina's window
  (unless **Keep the guide after choosing** is on). It never resizes the
  operating system's window.
- **Now:** a double click on the video toggles the operating system's full
  screen for that window, and plays nothing again. Escape first leaves the
  operating system's full screen, then the player's panels, then Patina's
  full screen.
- **Now:** resizing, a narrow side-by-side window, the operating system's
  full screen, a change of display scale and larger text change the layout
  only: the same player, the same stream, the same channel, and Back
  returns to it. The guide's header and Movies and Shows collapse by
  [class](interaction.md#layout-classes); collapsing the rail and groups
  column by class is still Planned there.

## Several players

A guide channel's menu has **Watch in new window** (Linux and macOS): a
separate player window for that channel, beside the main window's player.

- **Now: one window has the sound.** A new player window opens muted, and
  its title says so ("Music 05 · muted"); the title is the channel's name
  alone in the window that has the sound. A compact window keeps the title
  "Picture-in-Picture" (window managers' rules match it), so there its Mute
  control shows the sound. The main player shows its sound in
  its play bar's Mute control. Only an explicit sound control moves the
  sound: a window's Mute or volume, or the main player's Mute, Ctrl+M or
  volume, which takes the sound back from the other windows. Clicking into
  a window or moving the keyboard focus never moves the sound. Closing the
  window that has the sound gives it back to the main player, unless the
  main player is muted.
- **Now:** each window shows its own channel and keeps its own pause,
  volume, audio delay and channel list. A window's compact
  picture-in-picture is that window made small and floating; its player and
  stream stay the same.

## Closing

One contract, with or without recordings. #60's player-window rules are
unchanged; closing the main window now also closes every player window when
recordings keep the app running, as it already did when quitting.

| Action | Result |
|---|---|
| A player window's ✕, Stop, G or its "TV guide" | That window closes and its stream stops; the other players play on |
| Escape in a player window | Leaves compact, else closes its dialog, else leaves the operating system's full screen, else closes the window |
| A compact window's ⇱, Escape or double click | Back to its normal window, still playing |
| A compact window's ✕ | That window closes and its stream stops |
| The main window's picture-in-picture: ⇱, Escape or double click | The video comes back to the main window, full screen |
| The main window's picture-in-picture: ✕ | Playback stops |
| Minimizing the main window | Everything keeps playing |
| Closing the main window, no recordings scheduled or running | Patina quits: every window closes |
| Closing the main window while recordings are scheduled or running | The main window hides and **every** player stops: main, multiview, picture-in-picture and player windows. The recorder runs on; opening Patina again shows the main window |
| Quit (the macOS menu's Quit, or the system ending the session) | Recordings stop with the recorder; scheduled ones stay scheduled for the next start |

- **Now:** all rows (`crates/iptv-app/src/desktop.rs`,
  `desktop/player_windows.rs`, `desktop/pip_window.rs`).

## Displays and hosts

- **Now (macOS, Windows):** when a display is removed, displays are
  arranged again, a window first shows on a saved place whose display is no
  longer there, or a window comes back from minimized or full screen, a
  Patina window
  whose title bar can no longer be grabbed moves onto the nearest remaining
  display, at its size or smaller
  (`iptv_core::playback_windows::reachable`). A window the viewer can
  still grab is never moved. Moving a window plays and stops nothing.

| Host | Window positions | Compact window above others | After a display goes away | Fallback |
|---|---|---|---|---|
| macOS | Read and set | Floating level | Patina moves unreachable windows (above) | — |
| Windows | Read and set (main window; player windows are not offered) | — | Patina moves the main window | Player windows are Linux and macOS only |
| Linux, X11 | Not available: GTK 4 neither reports nor sets positions | Left to the window manager | The window manager keeps windows on screen | The compact window is titled "Picture-in-Picture", so window-manager rules written for browsers' picture-in-picture apply |
| Linux, Wayland (Hyprland) | Not through GTK; Patina places a window only when it first becomes compact, through Hyprland's IPC, and never moves one the viewer arranged | Hyprland rules that match the title | The compositor moves windows off removed outputs | As X11 |
| Linux, other Wayland compositors | Not available | Compositor policy | The compositor | As X11 |

- **Planned:** reporting these per-host differences as capabilities (#70).

## Verification

- Headless app scenarios `desktop_workflows` (wheel, right-click, ⋯, typing,
  window focus and sound, full screen and resize, closing during
  recordings), `guide_player_windows` (#60's player windows),
  `pip_control`, `pane_collapse`, `guide_nav` (the wheel through time) and
  `pointer`.
- Core unit tests in `iptv_core::playback_windows` (display recovery,
  session and sound state).
- The real-window journey: [issue #89 desktop journey](validation/issue-89-desktop-journey.md).
