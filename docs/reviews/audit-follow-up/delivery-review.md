# Security, recovery and delivery peer review

Delivery agent review completed at 2026-11-17 04:19 UTC. Reviewed the root
agent's uncommitted security, process-death and search changes on top of
`82a3d6d`. Application/runtime code was reviewed as workspace implementation,
with Rust edition 2024 and declared Rust 1.99.1. Review used the Rust review and
best-practices skill checklists. No remaining blocking findings were identified
in this scope. Cargo execution was coordinated through the root agent; this
reviewer did not independently rerun its native tests or Clippy.

## Independently inspected boundaries

- `content_image.rs` inspects encoded dimensions before passing downloaded bytes
  to NSH. The pinned NSH `Natural::of` rejects zero dimensions and values that do
  not fit `u32`; the application applies both side and pixel-product limits using
  `u64` multiplication. Unknown headers fail closed. SVG sizing uses the existing
  NSH parser. The documentation accurately limits this claim to one raster canvas,
  without claiming that the new header check bounds every animation frame.
- Xtream playback now retains public-address enforcement for public accounts.
  A selected private account exempts only its own URL origin. The existing relay
  and redirect transport independently re-enable public-only checks for a changed
  origin (`iptv-net/src/playback.rs` and `redirect.rs`). Tests exercise public and
  loopback accounts, another port, another private host and a public CDN.
- Production credential tests use the actual Secret Service implementation and
  encrypted catalog. Wrong, malformed, missing and locked keys must fail without
  changing the encrypted database bytes. The unavailable-broker case starts a
  bounded child process with its own invalid bus address. The separate delivery
  runner removes desktop/session variables, creates private HOME/XDG directories
  and owns the test daemon; no login keyring is used.
- Catalog replacement exits a child process before and after transaction commit,
  then reopens the encrypted database and checks the expected complete catalog.
  Restore exits after validation, each rename and directory sync; repeated actual
  startup recovery must retain the restored source and remove temporary siblings.
  Both suites reuse the existing executable and enforce bounded child lifetime.
  These tests cover process death, not sudden power loss or filesystem faults.
- Search state now groups indexes, loaded history and generation in one private
  module-owned structure. Defaults, ticket comparison, history invalidation and
  index lifecycle match the previous separate fields. The move introduces no
  new asynchronous borrow lifetime or cross-thread state access.
- The development playlist hook evaluates to `None` outside debug builds.
  Security documentation separates encrypted metadata from plaintext exports
  and recorded media, and preserves the unresolved native locale/thread-order
  limitation instead of claiming that `Once` proves it safe.

## Delivery validation performed by this reviewer

The delivery implementation itself received the separate root-agent review in
[integration-review.md](integration-review.md). Local helper validation covered
all 34 `tools/ci/test-*.py` scripts: 359 tests, zero failures, with explicit
tool-dependent skips retained. Ruff over `tools/`, ShellCheck for the changed
shell entrypoints with their sourced build-environment helper, workflow YAML
parsing and `git diff --check` passed. The helper tests cover plan selection,
workspace coverage inventory, required/local test-list parity, signing policy,
changelog requirements, provenance refusal before release mutation, dependency
candidate failure propagation, isolated broker environment and disposable AVD
ownership/backing-file confinement.

The root agent separately reported a successful production broker run including
the locked collection case, workspace rustdoc without warnings and catalog
process-death tests. These are attributed observations, not independent executions
by this reviewer; final native validation belongs in the implementation tracker.

## Evidence limits

No release was published, no signing operation or hosted configuration mutation
was performed, and no new hosted iOS or dependency-candidate workflow execution
is claimed here. The read-only public-release recheck at 04:02 UTC still showed
the documented v0.1.4 provenance exception; implementation does not retroactively
upgrade that historical artifact evidence. Android execution belongs to the root
agent and is emulator-only with `-no-audio`. This review does not claim physical
Android TV, spoken-screen-reader or manual visual acceptance testing.
