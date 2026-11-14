# Patina IPTV: consolidated audit, second round (2026-11-14)

This report merges the two independent audits of revision `5fa7a53` in this directory. That revision is 93 commits (PRs #28 to #36) after `8c259ee`, which the [first consolidated audit](2026-11-13-consolidated.md) covered.

| Source | Auditor | Overall | Character |
|---|---|---:|---|
| [`2026-11-14-claude.md`](2026-11-14-claude.md) | Claude Fable 5.2 as lead, nine reviewer agents (Fable, Opus, Sonnet, Haiku) | 7.5 / 10 | Broad and deep on security, concurrency and architecture. Read CI history, GitHub settings and release metadata. Missed two defects that Codex reproduced, and overstated four findings (see [Claims corrected](#claims-corrected)). |
| [`2026-11-14-codex.md`](2026-11-14-codex.md) | Codex, one reviewer, no sub-agents | 8.3 / 10 | Conservative and precise; two defects reproduced with synthetic inputs. Nothing in it was contradicted by the code. No network, so it did not see CI runs, repository settings, releases or pull requests. Did not find the relay gap or the unfinished runtime migration. |

**Consolidated overall: 7.7 / 10.** This is the mean of the consolidated column below. Yesterday's column averaged 7.4 and was stated as 7.5 by judgement, so like for like the repository moved from 7.4 to 7.7. The two reviewers of this report scored it 7.5 and 7.8.

The fixes since yesterday are real and well tested. Of the first round's 41 findings, 15 are fixed, 4 mostly fixed and 15 partly fixed, and neither audit found a regression. What remains:
- one relay gap and one header gap in transport;
- a release path that has not yet run for real;
- an application runtime whose migration is unfinished;
- partial localization;
- the process risks of one person merging very large changes with bot-only review.

This is the second revision. The first was reviewed by a fresh Claude agent and the Codex CLI; see [Review and revision](#review-and-revision).

## How this was consolidated

- Findings from both reports were merged and de-duplicated. Each one names its sources.
- New findings have IDs D01 to D25. The first round's IDs C01 to C41 are kept for follow-up status only.
- Each finding carries a status:
  - **Verified**: the cited code or setting was read during consolidation or review, and it says what the finding claims.
  - **Reproduced**: also observed at runtime; the row says by whom and how.
  - **Plausible**: the mechanism was read and is consistent with the code, but the effect was not observed or measured.
  - **Reported**: taken from the source audit; not re-checked.
- Nothing ran on a device or emulator in either audit. Both audits built and tested on Linux only.
- Statements in a source that the code contradicts are listed in [Claims corrected](#claims-corrected).
- Work that started after `5fa7a53` is not scored. See [In progress since this revision](#in-progress-since-this-revision).

## Consolidated scorecard

| Aspect | Claude | Codex | **Consolidated** | Yesterday | Basis for the consolidated score |
|---|---:|---:|---:|---:|---|
| Architecture & boundaries | 7.5 | 8.5 | **8** | 7.5 | The player and library model are real crates, no view names the SQLite adapter, and the boundaries have tests. Import orchestration still lives in the shell, and the runtime is one global with open fields (D08 to D10). |
| Portability & platform isolation | (9 sub-score) | 9 | **9** | 8.5 | Nine crates build for wasm in both audits, including the new `iptv-library`. No AGENTS.md violation was found. |
| Code quality & correctness | 8 | 8.5 | **8** | 8 | Library crates are excellent, and backup errors are typed. Two reproduced edge-case defects (D02, D03) and a few swallowed failures on data paths (D07). |
| Security & privacy | 8 | 8 | **8** | 8 | Arbitrary playlist headers no longer cross origins, Android has a relay-only guard, and mpv restrictions fail closed. Each audit found one transport gap the other missed (D01, D02). |
| Testing & CI | 8 | 8.5 | **8** | 8 | About 1,200 tests pass in both audits; coverage, manifest fuzzing and an emulator smoke test are new. Six scenario failures on Windows and macOS runners in one day, each since fixed; Windows is not a required check (D12, D13). |
| Performance & resilience | 7.5 | 8.5 | **8** | 8 | Per-source guide budgets with retries, a read-only connection, an atomic file writer. Duplicate guide-check timers in All-playlists mode, and the write connection still in front of import-driven display (D04 to D06). |
| Distribution & reproducibility | 7.5 | 8 | **7.5** | 7 | LGPL-only pinned libmpv on all desktops, attested draft-then-publish releases, an SBOM. None of it has run on a real tag, and desktops are unsigned (D03, D15, D16). |
| Maintainability | 5.5 | 8 | **6.5** | 6 | Ownership is clearer and architecture tests guard it. Still one author and bot-only review, with a 12.8k-line PR merged in 90 minutes (D17). |
| Documentation & onboarding | 7.5 | 8 | **7.5** | 6 | Architecture, CONTRIBUTING, CHANGELOG and SECURITY exist and are accurate. The README and changelog lag the refactors (D18). |
| UX, accessibility & localization | 6 | 7.5 | **6.5** | 6.5 | Identity-based focus and tested contrast. 59% of user-facing text bypasses the catalogue by Claude's count, with no plural or date rules; assistive technology is untested (D19 to D21). |
| **Overall** | **7.5** | **8.3** | **7.7** | 7.5 | Mean of the consolidated column (yesterday's column averaged 7.4). |

### Why the two scores differ

- **Codex had no network access.** It could not see CI history, branch protection, published releases, repository secrets or pull requests, and says so. Its testing and distribution scores therefore leave out the runner failures (D12) and the release path that has never run on a tag (D03). The consolidated scores for those two aspects follow Claude.
- **Claude folds process risk into maintainability; Codex scores the code.** Codex could not see the review statistics (D17). As yesterday, the consolidated 6.5 keeps the process risks and credits the clearer ownership.
- **Architecture sits between the two, at 8.** Both audits agree the runtime's fields are open and reached through a thread-local handle. Claude's architecture review went further and showed that the import lifecycle never moved out of the shell (D08), which keeps the score below Codex's 8.5. It is above Claude's 7.5 for two reasons: Claude's report overstated the player-contract finding (D11), and yesterday's 7.5 was for a tree where the player, the library model and the runtime all sat in one crate with no boundary tests.
- **Codex's performance 8.5 reflects what it measured.** It ran the two latency tests and did not trace the timer, write-lane and organisation paths that Claude's reviewer read (D04 to D06). Those mechanisms are verified; their effect is not measured, so the consolidated score is 8, not 7.5.
- **Each audit missed one transport defect.** Codex found that `Origin` values cross origins unchanged (D02). Claude found that unquoted HLS `URI=` attributes bypass the relay (D01). Both hold, and both audits scored security 8 without the other's finding.
- **Scope notes, not disagreements.** Codex says it found "no new P1 issue in the sampled code"; D01 was outside its sample. Codex did not open the screenshots; Claude's reviewer opened all four and scanned the fixtures, and found only synthetic data.

## Strengths both audits agree on

1. **The first round's main defects are fixed with tests.** Cross-origin requests keep seven transport headers and a `Referer` cut to its origin (one of the seven, `Origin`, is D02). Guide sources have their own budgets and retries. Rust-only changes now compile for Android on pull requests. The combined-schedule logo bug and the UI-thread file writes are gone.
2. **Portable state moved to where it belongs.** Playlist, guide-source and guide-assignment state live in the wasm-compatible `iptv-library`; `iptv-core` still has no dependencies.
3. **The store service is carefully built.** Separate write and read lanes, work scoped to the runtime that accepted it, and a tested guarantee that a busy writer does not block a read on the read lane.
4. **Player hardening fails closed.** mpv refuses to start if a required restriction cannot be set, and the Android player can only open relay URLs.
5. **Release engineering improved in kind, not just in amount.** The publisher refuses to touch a public release, every release file is attested, the NSH pin is one file read everywhere, and the bundled media runtime is built from pinned sources on all three desktops.
6. **Architecture rules are tests.** `tests/app/architecture.rs` pins the shell's statics, the store boundary and the runtime's ban on NSH.
7. **All local gates pass in both audits.** Clippy with warnings denied, formatting, the wasm builds, and identical test counts.

## Follow-up on the first round

Of the 41 findings in the first consolidated audit:

| Status | Count | Findings |
|---|---:|---|
| Fixed | 15 | C03, C04, C05, C07, C08, C11, C13, C14, C15, C18, C22, C32, C34, C35, C39 |
| Mostly fixed | 4 | C01, C27, C30, C38 |
| Partly fixed | 15 | C02, C06, C10, C12, C16, C17, C20, C21, C23, C24, C26, C28, C29, C31, C40 |
| Open | 5 | C19, C25 (mitigated), C33 (by design), C36 (documented), C37 |
| Withdrawn or dropped | 2 | C09, C41 |

This table follows Claude's follow-up, with one change: C01 is "mostly fixed", not "fixed", because the first-round finding named `Origin` and that part remains (D02). Codex's dispositions of its own ten findings are milder in wording for C17, C20, C24 and C25 ("substantially addressed", "addressed for ordinary reruns", "improved, not fully closed"); the facts behind them are the same.

Notes on four of them:
- **C02:** all screenshots and fixtures are synthetic now, but the old screenshot is still reachable in git history at `3a9c504`. No rewrite was done, by decision.
- **C06:** new builds bundle an LGPL-only libmpv with sources retained, but the published v0.1.0 and v0.1.1 AppImage and DMG still carry no source offer (see D03).
- **C25:** desktop builds are still unsigned, but the first round offered "or label those artifacts as previews" as a remedy, and new builds are labelled (see D15).
- **C26:** libmpv and FFmpeg are pinned by hash. The AppImage's GTK and GStreamer libraries still come from whatever the runner has, with their exact sources archived.

The residual part of every partly fixed finding is in the [Claude report](2026-11-14-claude.md#what-changed-since-the-first-audit); the ones that matter appear again below as D-findings.

## Consolidated findings

Priorities: **P1** before the next stable release, **P2** in the next improvement cycle, **P3** later hardening. They order the work; they are not CVSS ratings. Neither audit found a critical issue or a credential leak in default use.

### P1

| ID | Finding | Evidence | Sources | Status |
|---|---|---|---|---|
| D01 | **Unquoted HLS `URI=` attributes bypass the relay on desktop and iOS.** The rewriter matches only the literal `URI="`. FFmpeg also accepts `#EXT-X-KEY:METHOD=AES-128,URI=http://192.168.1.1/…` without quotes, so mpv fetches that address itself, outside the relay's public-address, downgrade and TLS checks. A hostile stream server gets a blind GET to the viewer's LAN or loopback. No credentials leak. Android has its own guard: `RelayGuard` refuses any address that is not the relay. mpv has no second guard, because its load hook covers top-level loads only, and the fuzz oracle makes the same assumption as the rewriter. | `iptv-net/src/playback.rs:798-816`; `iptv-player-mpv/src/mpv/worker.rs:130,236-275`; `fuzz/fuzz_targets/manifests.rs:59` | Claude N01 | Reproduced by the lead on loopback with the system mpv 0.41. Not run against the pinned FFmpeg 8.1 runtime or on iOS. |
| D02 | **A playlist-supplied `Origin` value crosses origins unchanged.** The allowlist keeps `Origin` because an origin "by definition carries no path or query", but nothing enforces that for values a playlist sets through the pipe syntax. A value such as `https://portal.example.test/watch?token=…` keeps its path and token on a cross-origin redirect or sub-resource. It needs a playlist that puts sensitive data in `Origin`; well-formed values are unaffected. Codex rated this P2. It is P1 here because it is what is left of the first round's P1 finding C01, and the fix is a few lines beside D01's. | `iptv-net/src/cross_origin.rs:26-37`; `iptv-providers/src/m3u/mod.rs:460-476`; callers at `redirect.rs:104`, `playback.rs:418` | Codex B01 | Reproduced by Codex at function level with synthetic values; code read during consolidation. |
| D03 | **The new release path has not run on a real tag, and a retried draft keeps stale notes.** v0.1.1 was published before the attestation, publisher and pinned-runtime work merged, and a manual dispatch never reaches the publish job. First use carries risks no test covers: asset size limits, attesting a dozen files, and the source-archive fallbacks. Separately, `publish` uses the generated notes only when it creates the release: a retried draft is published with its old body, so a preview notice can be missing or out of date. Codex rated the stale notes P2; they are listed here because the next tag is the first real use of this code. Meanwhile v0.1.1 is marked "Latest" with no source offer, no SBOM and no preview label on its unsigned DMG and installer. | `tools/ci/publish-release.py:142-158`; `release.yml:400,438`; release metadata for v0.1.1 | Claude N05, Codex B02 | Verified (publisher code; v0.1.1 was published at 02:02Z, before #29 and #32 merged). The stale notes were reproduced by Codex with the repository's CLI stub. |

### P2: correctness and performance

| ID | Finding | Evidence | Sources | Status |
|---|---|---|---|---|
| D04 | **Guide-check timers multiply in All-playlists mode.** Every refresh ends by scheduling the next guide check. In combined mode a refresh keeps the generation, so the earlier timer survives and another is added. Nothing keeps one timer per source: the `running` check guards `refresh`, not the timer's check. The timer's download uses a throwaway cancel token. When two timers for a source fire together, both can decide to download the same guide; a later one may instead see a fresh guide and skip. | `data.rs:214-219,313-317,477-490`; `data/library.rs:266-275`; `runtime/imports.rs:187-192` | Claude N02 | Plausible: both reviewers tried and failed to find a guard. Concurrent downloads were not observed. |
| D05 | **The write connection still sits in front of import-driven display.** When an import, refresh or cold start delivers a catalogue or guide, the handler first awaits `refresh_cache`, a job on the write lane. Cold start also reads source metadata on the write lane before the first guide data appears. Workers read the full catalogue and guide window through the write lane too. So switching playlists, or the first guide at start-up, can wait behind another source's guide publish. Not affected: the empty shell and early playback at start-up, and guide results from the timer check. Codex's latency tests show that a read on the read lane does not wait; these paths do not use it. | `data.rs:249-250` → `data/library.rs:46` → `runtime/library.rs:250-252`; `runtime/opening.rs:145`; `runtime/stored.rs:41-100`; `runtime/store_service.rs:355,381,406-412` | Claude N03 | Plausible: mechanism verified, delay not measured. |
| D06 | **Organisation edits do heavy work on the UI thread, and custom groups have two quadratic paths.** Each hide, rename or move re-sorts every channel on the UI thread. For 100k channels the project's own comment gives 126 to 173 ms on the Android TV emulator and 50 to 65 ms on a desktop. Two quick "Later" presses are reported to move a channel once, because both use the same snapshot. `reset_custom_group_order` is O(n²) inside one write transaction, and `Membership::reload` compares every selected id with every row on the UI thread. | `data/organization.rs:15-19`; `runtime_events.rs:34-44`; `runtime/commands.rs:213-227`; `iptv-store-sqlite/src/organise.rs:357-375`, `order.rs:9-30`; `iptv-core/src/membership.rs:75-81` | Claude N09 | Verified (the quadratic paths and the UI-thread install). The lost press is reported. |
| D07 | **A few failures are swallowed on data paths, and three functions on critical paths are very long.** If renaming an unreadable settings file fails, the error is dropped and the next save overwrites the file the log says was kept aside. A failed organisation edit is logged but the user sees nothing. A failed custom-group reload is dropped silently; a failed custom-group edit does show a message. `pipeline` is 295 lines, backup `export` 168 and `start_import_with` about 147. Four `unreachable!()` calls stand in for types. | `runtime/settings.rs:94-99`; `organization_view.rs:167-171`; `custom_groups.rs:173-188`; `runtime/imports.rs:266-560`; `runtime/backup.rs:277-444`; `data.rs:202-348`; `runtime/favorites.rs:403,421` | Claude N11 | Verified |

### P2: security

| ID | Finding | Evidence | Sources | Status |
|---|---|---|---|---|
| D25 | **The DASH rewriter ignores the declared XML encoding.** It reads bytes and passes text and CDATA through, so a manifest declared as ISO-2022-JP can hide a `SegmentURL` inside what the rewriter sees as CDATA. This is the same boundary as D01, but it is P2 because no shipped build is known to be exposed: Android's `RelayGuard` refuses the address, and the pinned Linux, macOS and Windows runtimes are built without a DASH demuxer. The exposed builds would be iOS, which links a third-party libmpv and is not part of a tagged release, and developer builds against a system libmpv. Which of those actually parse such a manifest should be checked before an iOS release. | `iptv-net/src/playback/dash.rs:10-139`; `tools/mpv-runtime-unix/linux/build.sh:99-102` | Claude low-severity note | Verified for the rewriter by Claude's reviewer, who also confirmed libxml2's reading with `xmllint`. Player behaviour is not tested. |

### P2: architecture and maintainability

| ID | Finding | Evidence | Sources | Status |
|---|---|---|---|---|
| D08 | **The runtime migration is unfinished.** Import lifecycle, combined mode and guide re-checks still live in the shell's `data/` modules (2.1k lines, NSH-dependent), which write the runtime's fields directly. At least 12 of the 24 `RuntimeEvent` variants are constructed only by the shell, to itself. One path loops: the runtime emits `RefreshDue`, the shell turns it into `data::refresh`, which calls back into the runtime. Six glob re-exports and eight duplicated module names hide where code lives. | `data.rs:202-349`; `runtime_events.rs:6`; `runtime/events.rs:6-40`; `data/stored.rs:43,72`; `favorites.rs:6`; `backup.rs:3` | Claude N06; Codex B04 cites one shell-side state update | Verified. Codex's review counts 14 shell-only variants when inline tests are excluded. |
| D09 | **The runtime is a global with open fields.** One `thread_local! RUNTIME` serves about 230 `crate::runtime()` call sites, and 35 of its 39 fields are `pub(crate)`. Invariants are kept by convention at each call site. It is better than the roughly 40 statics it replaced: one owner, deterministic teardown, two runtimes in one test. | `lib.rs:394-401`; `runtime/mod.rs:42-82` | Claude N07, Codex B04 | Verified |
| D10 | **Store types still reach the views, and the boundary tests are text searches.** `runtime/model.rs` re-exports six `iptv_store_sqlite` types, and eight files outside the runtime use them. The tests look for forbidden strings, so they miss the re-export and are not a compiler-enforced boundary. The runtime still compiles inside the UI crate, so its tests link NSH. | `runtime/model.rs:5-7`; `tests/app/architecture.rs:95-135` | Claude N08, Codex B04 | Verified |
| D11 | **The player contract is only partly enforced.** One macro declares the property set once and generates both a serializable `PlayerProps` and the `PlayerBindings` the shell builds, so property types and the set of fields are shared. The names on the wire are not: the shell, mpv and Kotlin each keep a hand-written list of strings, and `PlayerProps`, which carries the serialized names, is built only in a test. Player events have no shared type. Both players accept a `userAgent` property the app never sends. | `iptv-player-mpv/src/props.rs:25-53`; `video.rs:194-239`; `mpv/mod.rs:373-415`; `PlayerComponent.kt:412-433` | Claude N10; Codex credits the contract | Verified |

### P2: testing, release and process

| ID | Finding | Evidence | Sources | Status |
|---|---|---|---|---|
| D12 | **Six scenario failures on Windows and macOS runners in one day, and Windows is not a required check.** Among the last 30 runs of each workflow, Windows failed five times and Tests once, in its macOS job. Each was a scenario that waited for a screen or label and timed out; two were the same scenario. None fails on Linux, where both audits ran the suite clean. Every one of the six predates a targeted fix now on `master` (the off-screen wait helper in #33, #35, and commit `2124b5f`), and the run at `5fa7a53` is green. There have been too few runs since to know whether this kind of failure is gone. Branch protection requires only the Ubuntu and macOS tests and `android`, so the two Windows failures on `master` blocked nothing. | Runs 43569126169, 43571084274, 43573538295, 43571288495, 43577242589 (Windows), 43577242573 (Tests); branch protection | Claude N04 | Verified (run IDs, dates and branch protection read with `gh`). |
| D13 | **The Android smoke test does not cover process death, and it skips most crates.** It covers first import, playback, full screen, multiview, and background and resume, and it requires the process to stay alive in the background. It says nothing about recovery after process death, activity recreation, an interrupted restore, or a refresh during playback, which are what the new runtime and queued writes changed. Its pull-request path filter lists two of the thirteen crates, `iptv-app` and `iptv-net`. Changes elsewhere wait for a nightly schedule that has not yet run, because the workflow merged on the day of the audit. It is not a required check. Codex also asks for a passing tablet and TV run against each release candidate. | `tools/ci/android-smoke.sh:163-180`; `.github/workflows/android-smoke.yml:3-23,34` | Codex B03 | Verified |
| D14 | **Coverage is a measurement, not a gate, and fuzzing is narrow.** Library crates measure 95.2% of lines on CI, but there is no threshold and the app crate, including the 8.8k-line runtime, is excluded. Nine `runtime/` files have no unit test of their own; scenarios exercise them. Two fuzz targets and 12 seeds; no grown corpus is kept. | `coverage.yml:24-26`; `tools/ci/coverage.sh:4`; `fuzz/` | Claude N18, Codex B04 | Verified |
| D15 | **Desktop builds are still unsigned.** The signing and notarization jobs exist and fail closed, but the only signing secrets configured are the four Android ones. New builds are labelled previews, which both audits call honest. Byte-for-byte reproducibility of the packages was not measured by either audit. | `release.yml:87-148,299`; `README.md:88-92`; `gh secret list --env release` | Claude N17, Codex B05 | Verified |
| D16 | **Provenance stops at the Rust graph, and repository safeguards are off.** The SBOM leaves out libmpv, FFmpeg and the Android Gradle dependencies, and Gradle has no dependency verification or lock. GitHub immutable releases, secret scanning and push protection are disabled. Runtime-publish workflows can be dispatched from any branch, and the tag ruleset covers `v*` only. | `tools/ci/sbom.py:12-14`; `hosts/android/settings.gradle.kts`; `mpv-runtime.yml:3-9`; repository settings | Claude N16 | Verified (settings and ruleset read with `gh`). The SBOM and Gradle points are reported. |
| D17 | **Review is bot-only and not enforced.** All 14 PRs from #23 to #36 were self-merged. Seven got no CodeRabbit review (rate limit, or more than 100 files). #36 changed 12.8k lines and merged in 90 minutes; #34 touched 104 files and merged in 34. Branch protection requires no approving review. One author, 493 commits. This is C19, still open. | `gh pr view`; branch protection | Claude N15 | Verified |
| D18 | **README and changelog lag the code; follow-ups are tracked nowhere.** The README's crate table omits `iptv-library` and `iptv-player-mpv`, and its sentence calling every crate outside three "sans-IO" is no longer accurate, since the player crate is a native libmpv adapter on desktops. The changelog's Unreleased section misses most merged work. There are no issues, no TODOs, no issue or PR templates and about 40 remote branches. The audit documents themselves are not committed. | `README.md:266-279`; `CHANGELOG.md:8-42` | Claude N19 | Verified (README). The rest is reported. |

### P2: localization and accessibility

| ID | Finding | Evidence | Sources | Status |
|---|---|---|---|---|
| D19 | **Most text is English only, with no plural, date or word-order handling.** By Claude's count, 563 of about 960 user-facing literals bypass the catalogue (41% translated). Backup and restore confirmations, import failures, organisation, custom groups and guide assignment are English in every locale. Plurals are hand-written `if n == 1`, day and month names are English, the clock is 24-hour only, and `tr` has no placeholders. The project's own localization document lists these gaps; nothing enforces coverage. | `backup.rs:47-141`; `wizard_view/words.rs:117-127`; `docs/localization.md:13-16` | Claude N12, Codex B06 | Verified (plural helper, the document). The 41% figure is one reviewer's script and was not rerun. |
| D20 | **No CJK glyphs are bundled.** Roboto is subset to Latin, Greek and Cyrillic, so four of the eleven locales depend on system fonts, which no test covers. | `tools/fetch-fonts.sh:25-29`; `fonts.rs:1-8` | Claude N13 | Verified (subset ranges). The effect on real systems is untested. |
| D21 | **Assistive technology is unverified, and explicit accessibility semantics are sparse.** The app sets no roles and no selected or checked state, and toasts are not announced. Icon-only controls do have translated labels. A static count finds 62 `.on_press(` call sites and 12 `.label(` call sites, but shared helpers mean that ratio does not measure how many controls lack a name. No audit or test exercised TalkBack or VoiceOver, or large-text clipping. Guide cells clip text without an ellipsis. | `ui_kit.rs:30-45`; `grid/cell.rs:169`; `tests/app/interface_languages.rs` | Claude N14, Codex B06 | Verified (counts). The effect for screen-reader users is untested. |

### P3: hardening and smaller items

| ID | Finding | Sources | Status |
|---|---|---|---|
| D22 | **Security notes.** Any local process can hold all 12 relay connection slots for 15 s at a time without the token, and the token comparison is not constant-time (`playback.rs:32,284,389,398`). A user-entered origin is trusted by name, so a provider can later point its DNS into the LAN (`client.rs:136-138`, `redirect.rs:58-70`). A restored backup is trusted as user input, and archives are still plaintext (C37). A plaintext database found at startup is adopted without restore-time schema validation (`iptv-store-sqlite/src/encryption.rs:78-88`). The relay honours extra root certificates but no setting fills the list. Android allows cleartext for every host although the player only talks to `127.0.0.1`. | Claude low-severity notes; Codex notes plaintext backups | Reported |
| D23 | **Performance and reliability notes.** In All-playlists mode the last channel is saved through SQLite, which Android's `onStop` does not wait for (`player.rs:194-204`, `TvActivity.kt:89-95`). The WAL is never truncated after a large guide publish. Every Stalker zap builds a new HTTP client (`player.rs:414-416`). A guide sent with gzip transfer encoding is held to a lower size limit than the same guide as a `.gz` file. An organisation edit in All-playlists mode changes a child schedule in place (`data/combined.rs:375-391`). | Claude low-severity notes | Reported |
| D24 | **Smaller items.** Key routing, `go_back` and `bind_back` still list overlays by hand in different orders (rest of C10). `ProviderError` and `ImportError` have no `source()`, and `backup::lock()` returns `&'static str` (rest of C21). The NSH pin check compares the commit only, so it misses the Windows patch applied to the sibling checkout (rest of C20). 13 over-long lines are still exempt from the line-length check. `iptv-library` lacks `#![forbid(unsafe_code)]`. No shellcheck, ruff or ktlint step. Three `getrandom` versions. The image cache is insert-ordered. iOS has no tests. | Claude | Reported |

## Claims corrected

These statements appear in a source audit and do not match the code or the CI record at `5fa7a53`.

| Claim | Source | What was found |
|---|---|---|
| The player contract is "typed only on paper"; `PlayerProps` is never built outside its own test. | Claude N10 | The second half is true, but the same macro also generates `PlayerBindings`, which the shell builds in production (`video.rs:194`). Types and the field set are shared; only the wire names and events are not. Codex's credit for the contract was fair. See D11. |
| "Six of the last 30 Windows and macOS runs failed because a test waited for a toast or a 'Refreshing…' label that had already gone. A different scenario failed each time." | Claude N04 | Five Windows runs and one Tests run failed, out of the last 30 of each workflow. Two were the same scenario. "Playlist added" is a screen title, not a toast (`wizard_view.rs:550`). All six predate fixes that are on `master`. See D12. |
| The README "wrongly says `iptv-player-mpv` builds for wasm". | Claude N19 | It builds: `cargo build -p iptv-player-mpv --target wasm32-unknown-unknown` succeeded during this revision, because the libmpv adapter is compiled only on native targets. What is stale in that sentence is "sans-IO". See D18. |
| "Failed custom-group and organisation edits show the user nothing." | Claude N11 | A failed custom-group edit shows "The change could not be saved…" (`custom_groups.rs:190-197`). The silent cases are a failed custom-group reload and a failed organisation edit. See D07. |
| "Every `show`, including the cold-start first screen", waits on the write lane. | Claude N03 | Import-driven display does. Guide results from the timer check call `show` directly, and the empty shell and early playback at start-up do not wait. See D05. |
| C01 is fixed. | Claude | The first-round finding named `Origin`, which still crosses origins unchanged. Mostly fixed. See D02. |
| Security has "one Medium gap". | Claude | Two transport gaps, counting D02. The score stays 8 because both are narrow and neither leaks credentials in default use. |
| 73 press handlers against 13 explicit labels. | Claude N14 | 62 `.on_press(` call sites and 12 `.label(` call sites; the larger numbers count parameter names and a doc comment. See D21. |
| #36 changed 12.7k lines. | Claude N15 | +7,267 / −5,498, which is 12.8k. |
| 20 `uses:` lines are not pinned to a commit. | Claude's metrics census | All 20 are local actions or reusable workflows of this repository. Every third-party action is pinned. The Claude report already says so. |

Nothing in the Codex report was contradicted. Its rows in the first revision of this table were scope notes, and are now under [Why the two scores differ](#why-the-two-scores-differ).

## Validation results across the audits

| Check | Claude | Codex |
|---|---|---|
| `cargo fmt --check`, line-length check | clean | clean |
| `cargo clippy --workspace --all-targets -D warnings` | 0 warnings | passed |
| `cargo deny check` | not run (tool not installed) | passed; duplicate-version warnings only |
| `cargo test -p iptv-core` | 271 pass | 271 + 2 pass |
| Tests of the 12 non-app crates | 966 pass, 6 ignored | 966 pass, 6 ignored |
| wasm32 build of the nine portable crates | 9 of 9 | 9 of 9 |
| `cargo build -p iptv-app` | pass | pass |
| App unit tests / scenarios | 173 / 65, scenarios run three times with no flake | 173 / 65, run once |
| Native-player tests with `IPTV_REQUIRE_PLAYER_TESTS=1` | not run separately | 39 pass |
| The two ignored `store_latency` tests | both pass | both pass: 34.5 ms against the 50 ms budget; search in 286 ms while the writer was held for 6 s |
| Python tool tests | "17 of 18 pass" | 156 tests pass |
| Windows patch helper test | fails: needs `powershell.exe` | fails for the same reason |
| Line coverage | 95.2% over the library crates, read from CI | not measured |
| CI history, branch protection, releases, PR statistics | read with `gh` | not available (no network) |
| Reproductions | D01 with the system mpv on loopback | D02 at function level; the stale notes in D03 with a CLI stub |
| Device runs, screen readers, fuzz campaigns, Windows/macOS/iOS builds | not run | not run |

Codex's `tools/check.sh` run stopped at the first wasm build, because the system Rust that was first on its `PATH` has no wasm target. It then ran the remaining steps separately with the rustup toolchain, and they passed. That is an environment matter, not a portability failure.

One check was added during this revision: the wasm build of `iptv-player-mpv`, which passed (see Claims corrected).

## In progress since this revision

Neither audit covers work after `5fa7a53`, and none of it is scored here.

PR #37, "Extract native application services into iptv-runtime", was open with CI running when this report was revised. By its description it moves the runtime and its unit tests into a new crate that builds without NSH, injects platform access through ports, and adds the crate to the coverage list and the Android smoke paths. If it merges as described, it would address:
- the part of D10 about the runtime compiling inside the UI crate;
- the part of D14 about the runtime being outside coverage;
- possibly part of D13's path filter.

It would not, by itself, address the main point of D08: on that branch the shell's `data/` modules are still about 2,200 lines, and the `RUNTIME` thread-local is still in `lib.rs` (D09). These should be re-checked against the merged code, not taken from this note.

## Recommended sequence

1. **Before the next release (P1).**
   - Parse the HLS attribute list instead of searching for `URI="`, rewrite the value quoted or not, and refuse a tag line whose `URI` cannot be parsed. Extend the fuzz oracle to the unquoted form (D01).
   - Give mpv a second guard, as `RelayGuard` is for Media3: for example make the relay mpv's HTTP proxy and refuse anything outside the capability. That would also cover D25 (D01).
   - Reduce `Origin` to a serialized origin before it crosses origins, and define what happens to `null`, userinfo, non-HTTP schemes and malformed values (D02).
   - Rehearse the publish path against a throwaway draft, and update the notes on every draft retry (D03).
   - Add a written source offer to the v0.1.0 and v0.1.1 notes, or supersede those releases (D03).
   - Turn on immutable releases, so that the "never replace a public release" rule no longer rests on one script. Turn on secret scanning and push protection at the same time (D16).
2. **Next cycle: correctness and responsiveness.**
   - Keep one guide-check timer per source, with a cancel token that a refresh, switch, delete or app close cancels (D04).
   - Make `refresh_cache` a read, add a blocking reader for workers, and read opening metadata on the read lane (D05).
   - Prepare organisation state off the UI thread, make the two custom-group paths linear, and send moves as relative steps (D06).
   - Stop dropping the settings rename error, and show the user a failed organisation edit (D07).
   - Refuse DASH manifests that declare a non-UTF-8 encoding, and check which iOS and system-libmpv builds parse them (D25).
3. **Next cycle: tests and release trust.**
   - Watch the Windows and macOS scenario runs for a week; if they stay green, require `windows` and `android-check` (D12).
   - Add kill-and-reopen, activity recreation, refresh-while-playing and interrupted-restore cases to the smoke test, widen its path filter, and keep a passing tablet and TV run for each release candidate (D13).
   - Measure the runtime's coverage and set a floor for the library crates (D14).
   - Add Gradle dependency verification and an SBOM for the native runtime; restrict runtime publishing to `master` (D16).
   - Verify signing on a real release candidate before dropping the preview label (D15).
4. **Structural work, in small PRs.**
   - Finish the runtime migration before building on it: move the import loop, combined mode and paging out of the shell, delete the events only the shell emits, make the fields private, and move the remaining store-owned value types into `iptv-library` (D08 to D10).
   - Derive the wire names from the one property declaration, add a typed event, and have the Kotlin test drive `setProp` (D11).
   - Require one approving review, treat "review skipped" as a blocker, keep PRs under about 1.5k lines, file the open items as issues, and commit these audit documents (D17, D18).
5. **Localization and accessibility.**
   - Add a placeholder-capable `tr` with plural categories and locale-aware dates, route error, backup and organisation text through it, and add a test that fails on untranslated literals. Until then, say in the README that translation is partial (D19).
   - Bundle or document CJK fonts (D20).
   - Add roles and state to controls, announce notices, and run a repeatable screen-reader and large-text checklist on a device (D21).
6. **Hardening (P3).** Shorten the relay's pre-token deadline, refuse a plaintext database once the installation has been encrypted, flush the combined last channel on stop, cap the WAL, and offer passphrase-protected backups (D22, D23).

## Finding map

| Consolidated | Claude | Codex |
|---|---|---|
| D01 | N01 | n/a |
| D02 | n/a | B01 |
| D03 | N05 | B02 |
| D04 | N02 | n/a |
| D05 | N03 | n/a |
| D06 | N09 | n/a |
| D07 | N11 | n/a |
| D08 | N06 | B04 (part) |
| D09 | N07 | B04 |
| D10 | N08 | B04 |
| D11 | N10 | (credited as a strength) |
| D12 | N04 | n/a |
| D13 | n/a | B03 |
| D14 | N18 | B04 (coverage note) |
| D15 | N17 | B05 |
| D16 | N16 | n/a |
| D17 | N15 | n/a |
| D18 | N19 | n/a |
| D19 | N12 | B06 |
| D20 | N13 | n/a |
| D21 | N14 | B06 |
| D22 to D24 | low-severity notes | plaintext-backup note |
| D25 | low-severity note (DASH encoding) | n/a |

## Review and revision

The first revision of this report was reviewed for accuracy by two reviewers, each read-only and each asked to check it against the two sources and the code, and to try to refute six of the findings.

| Reviewer | Result |
|---|---|
| Fresh Claude agent (Opus 5.5), with read-only `gh` | 19 issues. Could not refute D01, D02, D04, D05, D08 or D11's facts. Confirmed the run IDs, release dates, repository settings and PR numbers. Scored the repository 7.5. |
| Codex CLI, no network | 13 issues, five marked "must fix". Scored the repository 7.8. |

Every point was checked against the code or the CI record before it was applied. Every factual point held, with one refinement: Codex counted 14 shell-only event variants where a count that includes inline tests gives 12, so D08 now says "at least 12". One suggestion was not taken: the Claude reviewer proposed lowering architecture to 7.5; the score stays at 8 and the reason is now stated.

What changed:

- **Corrected:**
  - D11: the contract is partly enforced, not "typed only on paper".
  - D12: the denominators, the repeated scenario, and the fact that all six failures predate fixes now on `master`.
  - D18: the player crate does build for wasm.
  - D07: a failed custom-group edit does show a message.
  - D05: which display paths wait on the write lane.
- **Reclassified:** C01 from fixed to mostly fixed, because the first round named `Origin`. The follow-up counts are now 15, 4, 15, 5 and 2.
- **Split out:** D25 (DASH encoding) from the P3 notes to P2, with the reason it ranks below D01.
- **Stated:** why D02 and D03 are P1 although Codex rated them P2; that the overall score is a mean while yesterday's was a judgement; why architecture is 8.
- **Moved:** turning on immutable releases, from the next cycle to before the next release.
- **Status labels:** D02 and D03 are marked as reproduced by Codex. D12, D15, D16 and D17 went from reported to verified after the reviewer read the GitHub state.
- **Carried over** details that the first revision dropped: the Gradle lock, `backup::lock()`, C26's AppImage residue, first-use risks of the release path, the release-candidate device run, and unmeasured byte-for-byte reproducibility.
- **Removed:** two rows that cited reviewer notes not published in either source report.
- **Added:** the note on PR #37.

The scores did not change. The corrections to D11 and D12 make two findings milder, and the reclassification of C01 makes one follow-up stricter.
