# Agent rules for Patina IPTV

- **Synthetic data only.** Fixtures, tests, docs, screenshots and commit text
  use generated playlists/EPG and local mock servers. Never commit real
  provider URLs, names, channel lists or credentials. Real-provider testing
  reads credentials at runtime from outside the repo and leaves no trace in
  logs, screenshots or files.
- **Behavior, not artwork.** The UX reference is `~/src/tivimate/docs`. Never
  copy files, code, artwork, icons, fonts, layouts or wording from the
  reference APK. `crates/iptv-app/src/theme.rs` holds the original Patina palette;
  preserve its independent design.
- **Domain and presentation stay portable.** `iptv-core`, `iptv-presentation` and the other portable crates must not
  depend on NSH or platform APIs, must not block, and must build for
  `wasm32-unknown-unknown`.
- **State ownership.** The runtime owns content and operations. Feature
  holders in `iptv-presentation` track focus by content identity. Views
  render state and send actions.
- **NSH pin.** Use the Patina NSH fork (`teng-lin/nsh`, branch `patina`) at
  `../nsh-patina` and the exact revision in `nsh.rev`. Patina consumes TV input through
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
- **Draft pull requests.** Open pull requests as drafts while pushing fixups
  (`gh pr create --draft`). Mark them ready (`gh pr ready`) once the change is
  complete; that starts the complete validation run. A draft run always fails
  its gate on purpose, listing the lanes it deferred.
- **Video layer.** One persistent player surface under the UI; modes change
  layout only. Never hide, detach or re-parent the player view: on Android
  hiding or detaching it destroys its video surface.
  Multiview adds one
  `tv.player` per further tile while it is open (`video.rs`). The
  desktops' picture-in-picture window shows surface 0's player through
  its `tv.pip` view without a second player or stream (`mpv/pip.rs`).
- **Check before finishing:** `cargo test -p iptv-core -p iptv-presentation`, the wasm
  builds of both crates, `cargo build -p iptv-app`, and for Android changes a run on
  `tab36` and/or `tv36`.
