# Agent rules for Patina IPTV

- **Synthetic data only.** Fixtures, tests, docs, screenshots and commit text
  use generated playlists/EPG and local mock servers. Never commit real
  provider URLs, names, channel lists or credentials. Real-provider testing
  reads credentials at runtime from outside the repo and leaves no trace in
  logs, screenshots or files.
- **Behaviour, not artwork.** The UX reference is `~/src/tivimate/docs`. Never
  copy files, code, artwork, icons, fonts, layouts or wording from the
  reference APK. `crates/iptv-app/src/theme.rs` holds the original Patina palette;
  preserve its independent design.
- **Core stays portable.** `crates/iptv-core` (and future core crates) must not
  depend on NSH or platform APIs, must not block, and must build for
  `wasm32-unknown-unknown`.
- **State lives in the core.** Views render state and send actions; focus is
  restored by content identity, never by list index.
- **NSH pin.** Use the upstream NSH checkout at `../nsh-patina` and the
  exact revision in `nsh.rev`. Patina consumes TV input through
  `vendor/nsh-tv` and NSH's public keyboard API.
- **Minimize test executables.** Each test target costs a separate link. Add
  integration scenarios as modules in an existing harness (`tests/it/` or
  `tests/app/`), rather than new top-level `tests/*.rs` targets. Prefer one
  integration executable per crate; allow at most two when allocator or native
  display isolation requires it. Use child processes of the same executable for
  fresh process state. Keep private unit tests in their owning crate and disable
  empty binary test harnesses. CI checks examples with Clippy's `--all-targets`,
  runs ordinary tests with `--tests`, and runs doctests separately. Run strict
  timing gates alone through the shared CI plan, reusing the same harness. The
  target budget is enforced by `tools/ci/test-check-plan.py`.
- **Video layer.** One persistent player surface under the UI; modes change
  layout only. Never hide, detach or re-parent the player view: on Android
  hiding or detaching it destroys its video surface.
  Multiview adds one
  `tv.player` per further tile while it is open (`video.rs`). The
  desktops' picture-in-picture window shows surface 0's player through
  its `tv.pip` view without a second player or stream (`mpv/pip.rs`).
- **Check before finishing:** `cargo test -p iptv-core`, the wasm build of
  `iptv-core`, `cargo build -p iptv-app`, and for Android changes a run on
  `tab36` and/or `tv36`.
