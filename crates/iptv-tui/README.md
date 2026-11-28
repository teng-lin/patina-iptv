# Patina TUI (`patina-tui`)

Optional terminal host for Patina IPTV built on `nsh-platform-tui`.
Built and linted in CI as a workspace member, but not packaged or distributed in release workflows.

## Prerequisites

- Sibling `../nsh-patina` checkout pinned to `nsh.rev`
- Patina's guarded `libmpv` on Linux and macOS ([setup](../../README.md#setup))
- `ffmpeg` on `PATH` for `--video=kitty` and `--video=inline` (video-only; no audio playback)
- `mpv` on `PATH` for `--video=popup` (audio + hardware-accelerated video in a separate desktop window)

## Build locally

```sh
cargo build --release -p iptv-tui
```

## Usage

```sh
./target/release/patina-tui [--video=kitty|inline|popup|none] [--headless] [--demo]
# or build and run directly:
cargo run --release -p iptv-tui -- [--video=kitty|inline|popup|none] [--headless] [--demo]
```

- `--video=kitty`: Fullscreen video via the Kitty Graphics Protocol (the default on Ghostty, Kitty, and WezTerm; video-only). Renders at 1280x720, 30 fps locally and 20 fps over SSH. Local sessions use a private `0700` directory in `/dev/shm`; over SSH, or without `/dev/shm`, frames are sent inline as zlib-compressed chunks.
- `--video=inline`: Fullscreen true-color half-block (`▀`) rendering inside any 24-bit terminal (video-only; the default elsewhere).
- `--video=popup`: Pops up a separate native `mpv` window on the local display (stays alive when switching back to the guide).
- `--video=none`: Headless guide and DVR browsing with no video output.
- `--headless`: Renders a 120×40 synthetic snapshot to stdout and exits (always implies `--demo`).
- `--demo`: Seeds an auto-cleaned temporary `HOME` with synthetic channels.

Controls: Arrows to navigate, `Enter` to select, `Esc` / `Backspace` for Back/Guide, `f` to toggle fullscreen, `m` for menu, `F10` / `Ctrl+Q` / `Ctrl+C` to quit.

## Limitations

- `TuiPlayer` is a lightweight subprocess player prototype (`ffmpeg`/`mpv`) routed through `iptv_net::playback::PlaybackRelay` (public-only destination policy, origin-scoped headers, credentials kept out of `argv`), and does not yet emit the full bidirectional `iptv-player-mpv` event contract (`state`, `position`, `tracks`).
- Video in `kitty` and `inline` modes is fullscreen-only (no preview player in Guide mode) and video-only (no audio).
