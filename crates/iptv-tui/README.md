# Patina TUI (`patina-tui`)

Optional terminal host for Patina IPTV built on `nsh-platform-tui`.
Built and linted in CI as a workspace member, but not packaged or distributed in release workflows.

## Prerequisites

- Sibling `../nsh-patina` checkout pinned to `nsh.rev`
- `ffmpeg` on `PATH` for `--video=kitty` and `--video=inline` (video-only; no audio playback)
- `mpv` on `PATH` for `--video=popup` (audio + hardware-accelerated video in a separate desktop window)

## Build locally

```sh
cargo build --release -p iptv-tui
```

## Usage

```sh
patina-tui [--video=kitty|inline|popup|none] [--headless] [--demo]
```

- `--video=kitty`: Fullscreen video via the Kitty Graphics Protocol (auto-selected on Ghostty, Kitty, and WezTerm; video-only). Uses a private `0700` `/dev/shm` directory locally (`1280x720@30`) and falls back to zlib-compressed inline chunked streaming over SSH (`1280x720@20`) or non-Linux systems.
- `--video=inline`: Fullscreen true-color half-block (`▀`) rendering inside any 24-bit terminal (video-only).
- `--video=popup`: Pops up a separate native `mpv` window on the local display (stays alive when switching back to the guide).
- `--video=none`: Headless guide and DVR browsing with no video output.
- `--headless`: Renders a 120×40 synthetic snapshot to stdout and exits (always implies `--demo`).
- `--demo`: Seeds an auto-cleaned temporary `HOME` with synthetic channels.

Controls: Arrows to navigate, `Enter` to select, `Esc` / `Backspace` for Back/Guide, `f` to toggle fullscreen, `m` for menu, `F10` / `Ctrl+Q` / `Ctrl+C` to quit.

## Limitations

- `TuiPlayer` is a lightweight subprocess player prototype (`ffmpeg`/`mpv`) and does not yet implement the full bidirectional `iptv-player-mpv` event contract (`state`, `position`, `tracks`, `headers` forwarding).
- Video in `kitty` and `inline` modes is fullscreen-only (no preview player in Guide mode) and video-only (no audio).
- Stream URLs are passed to `mpv`/`ffmpeg` via `argv` (only `http://`/`https://` schemes are allowed).
