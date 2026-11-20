# Patina TUI (`patina-tui`)

Optional local terminal host for Patina IPTV built on `nsh-platform-tui`.
Excluded from the root workspace so it is not built or distributed in CI.

## Build locally

```sh
cargo build --release --manifest-path crates/iptv-tui/Cargo.toml
```

## Usage

```sh
patina-tui [--video=kitty|inline|popup|none] [--headless] [--demo]
```

- `--video=kitty`: Fullscreen pixel video via the Kitty Graphics Protocol (auto-selected on Ghostty, Kitty, and WezTerm). Uses `/dev/shm` locally (`1280x720@30`) and automatically switches to zlib-compressed inline chunked streaming over SSH (`1280x720@20`).
- `--video=inline`: True-color half-block (`▀`) rendering inside any 24-bit terminal.
- `--video=popup`: Pops up a separate native `mpv` window on the local display.
- `--video=none`: Headless guide and DVR browsing with no video output.
- `--headless`: Renders a 120×40 snapshot to stdout and exits.
- `--demo`: Seeds an isolated temporary `HOME` with synthetic channels.

Controls: Arrows to navigate, `Enter` to select, `Esc` / `Backspace` for Back/Guide, `f` to toggle fullscreen, `m` for menu, `F10` / `Ctrl+Q` / `Ctrl+C` to quit.
