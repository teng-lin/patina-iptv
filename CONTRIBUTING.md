# Contributing to Patina IPTV

Thank you for helping. This page covers setup, the checks to run, and the
rules a change has to meet. [AGENTS.md](AGENTS.md) states the project rules in
short form, and [docs/architecture.md](docs/architecture.md) explains how the
code is laid out and why.

## Set up

You need:

- Rust, the version pinned in `rust-toolchain.toml` (rustup installs it
  automatically), with the `wasm32-unknown-unknown` target.
- The NSH fork checked out **next to** this repository, at the exact commit
  recorded in [`nsh.rev`](nsh.rev):

  ```sh
  git clone https://github.com/teng-lin/nsh.git ../nsh
  git -C ../nsh checkout --detach "$(cat nsh.rev)"
  (cd ../nsh && cargo build --release -p nsh-cli)
  ```

  NSH is a path dependency, so `Cargo.lock` cannot pin it. When `nsh.rev`
  changes, fetch and check out the new commit. `tools/check.sh` stops at once
  if `../nsh` is at another commit.
- The platform libraries listed under
  [Build from source](README.md#build-from-source) in the README.
- `cargo-deny` 0.20.2 for the dependency audit in `tools/check.sh`:
  `cargo install cargo-deny --version 0.20.2 --locked`.
- `python3` (3.11 or newer) and `ffmpeg` for the scripts and the synthetic
  test streams.

Run the app against the synthetic fixtures as the README's
[Try it with synthetic data](README.md#try-it-with-synthetic-data) shows.

## Check before you send a change

```sh
tools/check.sh
```

It runs the NSH commit check, formatting, Clippy with warnings as
errors, `cargo deny`, the tests of every library crate, the
`wasm32-unknown-unknown` builds of the portable crates, the app build, the
app's unit and headless scenario tests, and the unit tests of the build and
release scripts. CI runs the same script on Linux and macOS, and
`tools/build-windows.ps1 -Check` runs the same crate set on Windows.

One scenario: `cargo test -p iptv-app --features test-support <name>`.

Two extra checks are worth running when they apply:

- `tools/ci/check-android.sh` type-checks the app for Android (needs the NDK
  and `rustup target add aarch64-linux-android`). CI runs it for every pull
  request that changes Rust sources.
- `tools/ci/coverage.sh` measures test coverage of the library crates.

### When a device run is needed

A change to the Android host (`hosts/android`), to the video layer
(`crates/iptv-app/src/video.rs`, `player.rs`, the `tv.player` contract), to
input handling on Android, or to threading and storage that Android exercises
differently needs a run on a real device before it merges: an Android tablet,
an Android TV box, or both when the change affects both form factors. CI
launches the app only on emulators (`tools/ci/android-smoke.sh`, which you
can also run against your own emulator or device), not on real hardware.
The smoke script reinstalls the app or clears its data, so it erases the
playlists and settings on that device: run it on a disposable emulator or
device, or back the app up first. Say in the pull
request which devices you ran and what you checked, using the synthetic
playlist (`10.0.2.2` instead of `127.0.0.1` from an emulator).

## Rules for every change

**Synthetic data only.** Fixtures, tests, documentation, screenshots, logs you
paste and commit messages use generated playlists and guides and the local
mock servers in `tools/`. Never commit a real provider's address, name,
channel list, logo or credentials. If you test against a real provider, read
its credentials at run time from outside the repository and leave no trace in
files, logs or screenshots. This applies to issues and pull request
descriptions too.

**Behaviour, not artwork.** Patina follows the behaviour of a reference TV
guide experience, but shares none of its code, artwork, icons, fonts, layouts
or wording. Do not copy any of those. The palette in
`crates/iptv-app/src/theme.rs` is Patina's own.

**The core stays portable.** `iptv-core` and the other portable crates must
not depend on NSH or platform APIs, must not block, and must build for
`wasm32-unknown-unknown`. Put I/O behind a port and implement it in an
adapter crate.

**State lives in the core.** Views render state and send actions. Restore
focus by content identity, never by list index.

**One persistent player surface.** Modes change its layout only. Never hide,
detach or re-parent the player view.

**Keep NSH patches small**, with tests. They go upstream later. Developing the
fork locally? Set `IPTV_ALLOW_NSH_DRIFT=1`, and update `nsh.rev` in the same
pull request that needs the new commit.

**Pin what CI downloads.** A third-party GitHub Action is pinned to a full
commit SHA with a version comment; a tool is installed at an exact version
with `--locked`; a download is checked against a SHA-256.

## Commits and pull requests

- Write the subject as one imperative sentence that says what the commit
  does, without a type prefix and without a trailing period: "Keep translated
  labels reactive", not "fix: labels".
- Use the body to say what changed and why, including what you ruled out. A
  reviewer should not need the diff to understand the intent.
- One logical change per commit. Keep moves and refactors apart from
  behaviour changes.
- Add or update tests with the change. A bug fix comes with a test that fails
  without it.
- Add a line to the Unreleased section of [CHANGELOG.md](CHANGELOG.md) when
  users or packagers would notice the change.
- Dependency changes update `Cargo.lock` on purpose, and the runner lock with
  it; see [Locked package builds](docs/release.md#locked-package-builds).

By contributing you agree that your contribution is licensed under
[AGPL-3.0-only](LICENSE), the licence of the project.

## Reporting security problems

Do not open a public issue. See [SECURITY.md](SECURITY.md).

## Screen readers and large text

For changes to controls, focus, notices or typography, run this checklist on
both `tab36` and `tv36`, using generated playlists and EPG only. Record the
emulator profile, APK commit, Android font scale, TalkBack version, observed
results and screenshot paths in the PR. An unchecked item is pending, not a
pass. Restore the device's accessibility settings afterwards.

- Enable TalkBack in Android Accessibility settings. Navigate with touch on
  the tablet and D-pad on the TV. Confirm each actionable control has a
  meaningful name and role; decorative images must not become extra stops.
- Open the guide, channel/group lists, settings and choice editors. Confirm
  the currently selected item is announced, toggles announce checked state,
  and state changes are heard without leaving and revisiting the control.
- Trigger a synthetic import failure and an ordinary settings notice. Verify
  the notice is spoken once and focus stays at the initiating control.
- Open and close a chooser and a modal; verify focus returns to the same
  content item, including after a list reorder.
- Set Patina's Appearance → Font size to Extra large. Also try Android's
  largest system font size and record whether it affects the app (currently
  Patina uses its own text-size preference). Verify settings,
  backup/restore, organisation, custom groups and guide assignment remain
  operable, labels do not overlap, and long guide titles ellipsize while
  retaining the full accessible text.
- For each of `zh`, `zh-Hant`, `ja` and `ko`, select the interface language,
  open the guide and settings, and save a screenshot using synthetic CJK
  channel and programme names. Check for missing-glyph boxes and clipping.
- Return the interface language and Android font/accessibility settings to
  their initial values. Include any unresolved failures in the PR.

`tools/ci/android-cjk-screenshots.py emulator-5554 --out target/cjk-tab36`
captures the four CJK startup/guide scenarios from a debug APK built with the
synthetic development playlist. Start the mock server first as described in
`tools/ci/android-smoke.sh`. The script clears app data on its dedicated
emulator. Repeat on `tv36`; inspect every capture for missing glyphs and
clipping, then use the checklist above for settings and TalkBack.
