# Desktop player windows prototype

Related: [issue #60](https://github.com/teng-lin/patina-iptv/issues/60).
This is an experimental Linux implementation. macOS, Windows and general
Wayland placement remain unverified; this is not official desktop support.

Right-click a live channel or live programme in the existing guide and choose
**Watch in new window**. The main player continues. Each auxiliary window uses
the existing player views and control bar, with its own pause, volume, tracks,
channel navigation and full-screen target. Closing it stops only that session.
The same auxiliary channel does not open twice. Up to three auxiliary sessions
are available, subject to known account limits; multiview and auxiliary windows
are mutually exclusive in this prototype. Changing playlist closes the windows.
Recording controls are disabled in auxiliary windows pending DVR integration.

## Reproduce on Linux

Use the toolchain, NSH revision and native dependencies in CONTRIBUTING.md.
Fetch the pinned Linux mpv runtime as described in README.md and generate the
HLS fixtures with `tools/make-hls-fixtures.sh` if needed. Then run:

```sh
python3 tools/preview-guide-windows.py --walkthrough
```

This launches the actual App with an isolated temporary profile and a local
synthetic HLS server on port 18091. The timed walkthrough invokes the same menu
action to open two channels; omit `--walkthrough` to use the guide manually.
The demo is silent. Window arrangement remains a UX/host capability follow-up;
no compositor-specific placement adapter is included in this prototype.

## Mac test handoff

Record the PR commit, macOS version, architecture, NSH revision and mpv runtime.
Use generated playlists and local mock streams only. Never capture an existing
personal profile or provider information.

1. Build with `cargo build -p iptv-app` and run the existing synthetic-data app
   checks from README.md/CONTRIBUTING.md. Verify normal guide playback, the
   existing single PiP, return/close and full-screen behavior.
2. Confirm that **Watch in new window** is currently absent: the new entry point,
   mounting and native surface-routing metadata are Linux gated. The preview
   example also prints that it is Linux-only. A successful Mac build alone does
   not verify auxiliary video playback.
3. Inspect `mpv/mac_pip.rs`: it still has a singleton `HOST`. Multiple independent
   windows need per-host lifecycle and explicit surface-to-player routing, like
   the GTK prototype's `HOSTS` and `SURFACES`, plus Mac entry-point enablement.
   Simply removing the Linux gates is not a supported Mac implementation.
4. Once Mac routing exists, test main A plus separate B and C in one process;
   visible video in both, movable/resizable windows, correct channel identity,
   local pause/tracks/volume/full screen, closing B leaving A/C playing, reopening
   without duplicate sessions, changing channels and playlists, main minimize,
   app quit, mixed-DPI displays and window placement. Include native recordings
   only with synthetic content. Record failures and actual observed results.

Mac native playback, Windows, X11/other Wayland behavior, Android video-layer
regressions and accessibility/device checks remain pre-merge verification work.
