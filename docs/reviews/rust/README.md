# Rust workspace defect review

The sections below preserve the initial **review-only** run. Subsequent user
authorization started implementation in an isolated worktree; see the
[implementation tracker](../../plans/2026-11-16-rust-review-implementation.md),
[final portable review](implementation/final-portable-review.md),
[final native review](implementation/final-native-security-review.md), and
[Claude](implementation/claude-implementation-review.md)/[agy](implementation/agy-implementation-review.md)
implementation reviews for current dispositions and validation limits.

Review baseline: `0e197073a272eceb0631e4d53b08d2823950c416`, 2026-11-16.
Scope: all fourteen workspace crates except independently owned `iptv-nav`.
The [workspace plan](../../plans/2026-11-16-rust-workspace-review.md) supplies
review coverage and implementation gates. This run produces reports and a
reviewed consolidated fix plan. Source code, tests, manifests and commits are
outside the authorized changes for this run.

## Crate reports

- [iptv-core](iptv-core.md)
- [iptv-settings](iptv-settings.md)
- [iptv-providers](iptv-providers.md)
- [iptv-epg](iptv-epg.md)
- [iptv-library](iptv-library.md)
- [iptv-guide](iptv-guide.md)
- [iptv-search](iptv-search.md)
- [iptv-import](iptv-import.md)
- [iptv-net](iptv-net.md)
- [iptv-sqlcipher-init](iptv-sqlcipher-init.md)
- [iptv-store-sqlite](iptv-store-sqlite.md)
- [iptv-player-mpv](iptv-player-mpv.md)
- [iptv-runtime](iptv-runtime.md)
- [iptv-app](iptv-app.md)

Each report separates confirmed source paths, executable synthetic probes and
unverified risks. Proposed patches are report excerpts, not applied changes.
File inventories distinguish production review from scenario/test inspection;
not every test body received an exhaustive audit. Platform code inspected on
Linux does not establish device behavior on other hosts.

## Coordinated validation

Host: Linux x86_64, kernel 7.2.15-1-omarchy. Cargo and rustc 1.99.1 from Arch;
workspace edition 2024, declared Rust version 1.99.1. Initial tracked tree was
clean; `docs/plans/` was already untracked. A SHA-256 baseline of all 991 tracked
files is retained outside the repo and was rechecked unchanged at completion.
The application HEAD is unchanged; new artifacts are uncommitted documents only.

| Check | Result | Evidence |
|---|---|---|
| `bash tools/check.sh` | Stops at NSH revision mismatch before the suite | `check-baseline.log` |
| `python3 tools/ci/check-plan.py format` | Passed for workspace packages | `format.log` |
| Workspace Clippy, all targets, runtime/app test-support, `-D warnings` | Passed | `clippy.log` |
| Workspace ordinary tests with runtime/app test-support | App unit tests 129 passed; app scenarios 173 passed and 4 ignored; later player unit target failed with 50 passed and 10 failed | `workspace-tests.log` |
| Ordinary tests excluding app and player, runtime test-support | 1483 passed, 0 failed, 7 ignored across 23 harness results | `non-player-tests.log` |
| Workspace doctests with runtime/app test-support | 13 passed, 0 failed, 2 ignored | `doctests.log` |
| `cargo build --locked -p iptv-app` | Passed | `app-build.log` |
| `cargo build --locked -p iptv-core --target wasm32-unknown-unknown` | Failed: target standard library absent; rustup unavailable on this host | `wasm-core.log` |

Logs and temporary reproducer sources are under
`/tmp/patina-rust-review-20261116/` on the review host. Portable reviewer probes
also reside under `/tmp/patina-portable-review/`. Reports record relevant output
so their conclusions do not depend solely on temporary files surviving.

The NSH checkout is `tv-keys` at
`32769658ad34ad364b960a134cc73c754d50f2d9`; `nsh.rev` specifies
`9ed782cbf362ab63ba18c47dc657fe944c405e50`. A diff between those revisions is
empty, but the exact revision gate still fails. Neither checkout nor the pin
was changed. The player test binary links `/usr/lib/libmpv.so.2`. Ready/ended
and native worker timeout failures are baseline evidence, not proof that every
proposed player defect caused every failure. The unmodified player target ran
484.11 seconds and exited with failure; no test process was abandoned.

The full host suite is not green. Strict isolated timing gates, fuzzing, Miri,
Android smoke and Apple/Windows device validation remain implementation gates;
they were not implied by the passing checks above.

Exact additional Cargo commands used:

```sh
cargo clippy --locked --workspace --all-targets \
  --features iptv-runtime/test-support,iptv-app/test-support -- -D warnings
cargo test --locked --workspace \
  --features iptv-runtime/test-support,iptv-app/test-support \
  --tests -- --format=pretty
cargo test --locked --workspace --exclude iptv-app --exclude iptv-player-mpv \
  --features iptv-runtime/test-support --tests -- --format=pretty
cargo test --locked --workspace \
  --features iptv-runtime/test-support,iptv-app/test-support \
  --doc -- --format=pretty
```

## Review and consolidation status

All fourteen reports are complete with 39 finding IDs. The
[consolidated fix plan](../../plans/2026-11-16-rust-review-consolidated-fix-plan.md)
maps each ID to a disposition, owner, work package and regression gates; guide
request identity and its runtime consequence share one implementation package.
Player teardown remains an investigation before lifecycle changes.

A fresh three-agent team challenged the source evidence and approved the
corrected plan. Claude CLI supplied an independent source review; agy CLI
supplied a report-packet review. Both CLIs subsequently approved final plan
packets with no blocking omissions. The
[consolidation decisions](peer-review/consolidation-decisions.md) explain material
disagreements and scope limitations; the plan links every review artifact.

The review task ends here. No implementation finding is fixed, no failing or
unavailable platform gate is marked passed, and no code change or commit was made.
