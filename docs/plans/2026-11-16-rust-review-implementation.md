# Rust workspace review implementation

Branch: `fix/rust-workspace-review`. Isolated worktree:
`/home/blackmyth/src/patina-rust-review/patina-iptv`.
User authorized fixes, native/Claude/agy review, commits, one PR, CI/review/conflict
fixes and merge when ready. The [reviewed fix plan](2026-11-16-rust-review-consolidated-fix-plan.md)
is the contract. Historical review-only records remain distinct from this implementation.

Six source commits now sit above merged PR #95, including independently merged
PR #97 from master. Rebase completed without conflicts. No implementation PR
has been opened yet. The exact-tree full check is running; no full pass claimed.

## Finding tracking

Every accepted source correction is implemented and source-reviewed. The shared
final validation and affected platform gates remain open for the batch.
`iptv-sqlcipher-init` had no confirmed defect; `iptv-nav` was independently owned.

| ID | State | Implementation record |
|---|---|---|
| iptv-core-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-core-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-core-003 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-settings-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-settings-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-providers-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-providers-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-providers-003 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-epg-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-epg-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-epg-003 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-library-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/guide-identities.md) |
| iptv-library-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/guide-identities.md) |
| iptv-guide-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/pages-search-review.md) |
| iptv-guide-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/pages-search-review.md) |
| iptv-search-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/search-identities.md) |
| iptv-search-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/search-identities.md) |
| iptv-import-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-import-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/portable-first-pass.md) |
| iptv-net-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/net-fixes.md) |
| iptv-net-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/net-fixes.md) |
| iptv-store-sqlite-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/store-first-pass.md) |
| iptv-store-sqlite-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/store-first-pass.md) |
| iptv-store-sqlite-003 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/store-first-pass.md) |
| iptv-player-mpv-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/native-lifecycle.md) |
| iptv-player-mpv-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/native-lifecycle.md) |
| iptv-player-mpv-003 | investigated and tested; lifecycle order retained | [Scoped record](../reviews/rust/implementation/native-lifecycle.md) |
| iptv-runtime-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/inspection-access.md) |
| iptv-runtime-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/inspection-access.md) |
| iptv-runtime-003 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/inspection-access.md) |
| iptv-runtime-004 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/inspection-access.md) |
| iptv-runtime-005 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/inspection-access.md) |
| iptv-runtime-006 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/inspection-access.md) |
| iptv-runtime-007 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/inspection-access.md) |
| iptv-app-001 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/app-fixes.md) |
| iptv-app-002 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/app-fixes.md) |
| iptv-app-003 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/app-fixes.md) |
| iptv-app-004 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/app-fixes.md) |
| iptv-app-005 | implemented; combined validation pending | [Scoped record](../reviews/rust/implementation/app-fixes.md) |

## Validation and review

- [x] Fresh [portable](../reviews/rust/implementation/final-portable-review.md)
  and [native/security](../reviews/rust/implementation/final-native-security-review.md)
  source reviews, including post-Claude correction recheck: no blockers remain.
- [x] [Claude source review](../reviews/rust/implementation/claude-implementation-review.md)
  completed with supported Sonnet after the default model hit its usage limit.
  Concrete findings corrected; remaining suggestions verified or dispositioned.
- [x] Six substantive [agy diff packet reviews](../reviews/rust/implementation/agy-implementation-review.md)
  completed; identified stale-search and guide-seed cases corrected.
- [x] Guarded Linux player suite: 66 passed, including actual decoder pause/range
  recovery and bounded pending-control removal child; native teardown unchanged.
- [x] Recording suite: 62 passed after capture-authority and acknowledgement fixes.
- [x] Parental suite: 10 passed, including semantically equal cache refresh.
- [x] Foundation integration: 39 passed, one isolated budget case excluded.
- [x] Workspace formatting, line checks, dependency audit and Clippy passed in
  the first full attempt; final exact-tree shared run remains required.
- [x] Rebase onto merged #95/#97.
- [ ] Exact-tree full `tools/check.sh`: tests, isolated budgets, portable wasm,
  separate doctests, private-display GTK scenario and production app build.
- [ ] Affected fuzz/platform CI, final Android builds and device/accessibility gates.
- [ ] One PR, final-head CI/review/conflict resolution and merge.

The initial portable batch passed 898 tests. Earlier store/network suites passed
175/123 tests. These are intermediate evidence, not substitutes for the final
feature-unified shared run. The first full attempt reached app scenarios with
169 passed and four failures: two catalogue lint failures, an older test expecting
the removed description placeholder, and a tablet sleep test using a real clock.
All four were corrected without weakening lints or behavior assertions.

Use pinned Rust 1.99.1, the exact adjacent NSH worktree and guarded libmpv.
The coordinated runner, commands and logs are in
`/tmp/patina-rust-fixes-20261116/`; record summaries here so evidence is not
dependent solely on temporary files.

## Android gates and validation incident

See the [Android record](../reviews/rust/implementation/android-validation.md).
Preliminary aarch64 typecheck, 25 Java unit tests and fresh x86_64/arm64 builds
passed. Fully isolated tab36/tv36 API 36 lifecycle smokes passed. Final committed
APK and physical-device/accessibility checks remain separate gates. The physical
tablet's closed hardware cover currently prevents playback validation; user input
has been requested.

An incorrect emulator setup command removed the original tab36 userdata, cache
and encryption-key images; no recovery is established. This was disclosed to the
user. Physical application data was untouched. The original emulator profile is
no longer modified; subsequent validation uses complete isolated AVD copies.
