# Desktop player windows prototype

Related: [issue #60](https://github.com/teng-lin/patina-iptv/issues/60).
This is an experimental Linux and macOS implementation. Windows remains
disabled; general Wayland placement remains unverified. This is not official
desktop support.

Right-click a live channel or live programme in the existing guide and choose
**Watch in new window**. The main player continues. Each auxiliary window uses
the existing player views and control bar, with its own pause, volume, tracks,
channel navigation and full-screen target. Closing it stops only that session.
The same auxiliary channel does not open twice. Up to three auxiliary sessions
are available, subject to known account limits; multiview and auxiliary windows
are mutually exclusive in this prototype. Changing playlist closes the windows.
Recording controls are disabled in auxiliary windows pending DVR integration.
The primary player's compact PiP remains supported; auxiliary windows do not
yet have their own compact/always-on-top PiP toggle.

## Reproduce on Linux or macOS

Use the toolchain, NSH revision and native dependencies in CONTRIBUTING.md.
Fetch the pinned runtime for your platform as described in README.md. The
launcher uses `target/mpv-runtime-linux/lib` or `target/mpv-runtime-macos/lib`,
or an explicitly supplied `IPTV_RS_MPV_LIB_DIR`. Generate the HLS fixtures
with `bash tools/make-hls-fixtures.sh` if needed. Then run:

```sh
python3 tools/preview-guide-windows.py --walkthrough
```

This launches the actual App with an isolated temporary profile and a local
synthetic HLS server on port 18091. The timed walkthrough invokes the same menu
action to open two channels; omit `--walkthrough` to use the guide manually.
The demo is silent. On macOS the launcher builds a private
`target/guide-window-preview.app` and executes it with the temporary HOME and
selected runtime environment; it never launches an installed Patina profile.
`--no-build` reuses the example already built in the selected Cargo target.

Mac auxiliary windows open with separated title bars on the main window's
display, constrained by its usable area. This uses NSH's supported initial
position API, with its native cascade as fallback. Later controls, resizes and
fullscreen transitions do not reset a user's placement. There is no
compositor-specific placement adapter for Linux.

## Mac video and lifecycle

Each `tv.player` supplies its logical `popupSurface` binding to the Mac
backend. Each auxiliary `tv.pip` host requests one explicit `surface`; the
ordinary single-player PiP host follows the existing `pip.window` selection.
An auxiliary host with no matching player waits without borrowing PiP's video.
Hosts can mount before or after their players. A newly constructed host waits
for its props before adopting the ordinary PiP default.

The backend moves the existing layer-hosting NSView into the selected host.
Its CGL context and mpv render context remain the player's, with no new player
or load solely to present a window. Closing a host returns only its borrowed
view. Player teardown removes its surface binding by player identity before
freeing the render context; a late teardown cannot erase a replacement player
on the same surface. Host references in the registry are weak. Only ordinary
PiP locks the content aspect ratio; auxiliary windows resize freely with the
original player layout and controls. No NSH checkout changes are required.

The Android persistent-video rules are unchanged: no Android player is hidden,
detached or reparented by this desktop implementation.

## Mac test handoff

Record the PR commit, macOS version, architecture, NSH revision and mpv runtime.
Use generated playlists and local mock streams only. Never capture an existing
personal profile or provider information.

1. Build with `cargo build -p iptv-app` and run the existing synthetic-data app
   checks from README.md/CONTRIBUTING.md. Verify normal guide playback, the
   existing single PiP, return/close and full-screen behavior.
2. Run `cargo test -p iptv-player-mpv` for identity-aware Mac routing and
   teardown tests, and
   `cargo test -p iptv-app --features test-support --test app guide_player_windows::`
   for the guide's original controls, per-surface props, fullscreen target,
   channel changes, close/reopen and return-to-guide disposal. Run the existing
   `pip_control::` and `cov_mpv_pip_window::` scenario filters as well. These
   headless tests do not verify AppKit rendering.
3. Run the native synthetic demo above. Confirm that the channel and live
   programme context menus offer **Watch in new window**, and that the main
   preview continues while auxiliary windows show their own test patterns.
4. Test main A plus separate B and C in one process;
   visible video in both, movable/resizable windows, correct channel identity,
   local pause/tracks/volume/full screen, closing B leaving A/C playing, reopening
   without duplicate sessions, changing channels and playlists, main minimize,
   app quit, mixed-DPI displays and window placement. Include native recordings
   only with synthetic content. Record failures and actual observed results.

Record each actual native observation separately from a headless pass. Mixed-DPI
displays, minimize/quit, audio hardware, accessibility/device checks and
X11/other Wayland behavior still require their respective environments. Windows
needs a backend port before its entry point can be enabled.
