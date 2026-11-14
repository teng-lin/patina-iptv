# Patina IPTV: consolidated audit (2026-11-13)

This report merges the three independent audits of revision `8c259ee` (v0.1.1) in this directory:

| Source | Auditor | Overall | Character |
|---|---|---:|---|
| [`2026-11-13-claude.md`](2026-11-13-claude.md) | Claude, seven reviewer agents | 7 / 10 | Broad; strongest on process, distribution, resilience and hygiene. Missed one security gap and overstated three numbers. |
| [`2026-11-13-codex.md`](2026-11-13-codex.md) | Codex, three reviewer agents | 7.4 / 10 | Conservative; every finding carries a confidence statement. Nothing in it was contradicted by the code. |
| [`2026-11-13-agy.md`](2026-11-13-agy.md) | agy (Gemini), five reviewer roles | 9.0 / 10 | Most line-level detail, and the highest error rate: several headline findings are unreachable or wrong. |

**Consolidated overall: 7.5 / 10.** This is a judgement, not an average (the consolidated column below averages 7.4). Patina is unusually well engineered for a v0.1: the portable core, the storage and network security model, the tests and the release tooling are real strengths. The open issues are one confirmed credential-confidentiality gap, one breach of the project's own data rule, licensing and trust gaps in distribution, and structural concentration in one crate and one person.

This is the second revision. The first was reviewed by a fresh Claude agent, the Codex CLI and the agy CLI; see [Review and revision](#review-and-revision).

## How this was consolidated

- Findings from all three reports were merged and de-duplicated. Each one names its sources.
- Each finding carries a status:
  - **Verified**: the cited code was read during consolidation or review, and it says what the finding claims.
  - **Plausible**: the mechanism is consistent with the code, but the effect was not reproduced.
  - **Reported**: taken from the source audit; not re-checked.
- "Verified" means static confirmation. Nothing was reproduced at runtime for this consolidation, and no audit ran anything on a device (`tab36`, `tv36`). Where a source audit observed something at runtime, the row says so.
- Claims that the code contradicts are listed in [Claims corrected or rejected](#claims-corrected-or-rejected).
- Finding IDs are stable across revisions. C09 was withdrawn after review.

## Consolidated scorecard

| Aspect | Claude | Codex | agy | **Consolidated** | Basis for the consolidated score |
|---|---:|---:|---:|---:|---|
| Architecture & boundaries | 7 | 8 | 9.0 | **7.5** | Portable core and sans-IO ports are excellent. `iptv-app` holds too much, and pure screen state depends on SQLite types (C16 to C18). |
| Portability & platform isolation | (9 sub-score) | 8 | n/a | **8.5** | Eight crates build for wasm in CI. No web adapter exists yet, so portability is checked at compile time only. |
| Code quality & correctness | 7.5 | 8 | 9.2 | **8** | Library crates are disciplined. One confirmed user-visible bug in the app (C07); the other reported bugs are latent or unreproduced (C03, C10, C11, C23). |
| Security & privacy | 9 | 7 | 9.1 | **8** | The design is strong, but custom credential headers leak across origins (C01), which two audits found and one missed. |
| Testing & CI | 8 | 8 | 9.0 | **8** | About 1,130 Rust tests that check behaviour, strict gates. Gaps: Android compile on Rust-only PRs, a flaky scenario, no coverage. |
| Performance & resilience | 8 | 8 | n/a | **8** | Streaming imports and atomic publishing. Shared guide budget without retry, one DB connection, schedule built on the UI thread. |
| Distribution & reproducibility | 7 | 7 | 9.0 | **7** | Locked, pinned and smoke-tested packages. Unsigned desktops, replaceable release assets, floating native inputs, no source offer. |
| Maintainability | 5 | 7 | 8.8 | **6** | Good tooling and regression protection. One author, bot-only review, about 63 globals, a fork outside the repo. |
| Documentation & onboarding | 6 | 6 | n/a | **6** | Good README and security/release docs. No architecture overview or community files; release doc has drifted. |
| UX, accessibility & localization | 6 | 7 | n/a | **6.5** | Identity-based focus and tested D-pad navigation. Translation coverage is partial; assistive technology untested. |
| **Overall** | **7** | **7.4** | **9.0** | **7.5** | Judgement; the three reviewers of this report scored the repository 7.5, 7.5 and 7.8. |

### Why the three scores differ

- **agy's 9.0 is not supported by its own findings.** Its summary says 25 findings; the report actually labels 32, a dozen of them Medium, and then deducts about one point per category. It also states that no real provider data exists in the repository or its history, which is wrong (C02). On checking, three of its headline Medium bugs are not reachable in the app (C03, C09, C11) and several details are false. Its scores are not used as an anchor. It deserves credit for findings that hold up: the header leak (also found by Codex), and three nobody else made: the untested MSRV, the Windows CI gaps, and the `!Send` schedule.
- **Claude's security 9 was too high.** That review found no Medium issue, but the cross-origin header leak (C01) is real. It also said the mpv hardening options "are verified"; in production code a failure only logs a warning (C32).
- **Claude's maintainability 5 folds in bus factor and the screenshot violation**, which Codex scored under documentation and agy did not consider. The consolidated 6 keeps the process risks and counts the screenshot once (C02).
- **Codex scored what it could establish.** It did not open the screenshot, could not run the wasm build (no target installed) and did not run clippy or `cargo deny`. Its numbers are closest to the consolidated ones.

## Strengths all three audits agree on

1. **A genuinely portable core.** `iptv-core` has no dependencies and forbids unsafe code. Providers, import, EPG, guide, search, settings and navigation are sans-IO with injected clocks, and `tools/check.sh` builds all eight for `wasm32-unknown-unknown`.
2. **Unsafe code is contained and documented.** Ten of eleven crates have `#![forbid(unsafe_code)]`. All unsafe code sits in the app's mpv and platform FFI, each block has a `SAFETY` comment, and the workspace denies undocumented unsafe blocks.
3. **Encryption at rest is done properly.** A random 256-bit SQLCipher key lives in the platform keystore (wrapped by the Android Keystore on Android), and the app refuses to open rather than fall back to plaintext.
4. **Untrusted network input is handled carefully.** rustls only, no disabled certificate checks, manual redirects that refuse HTTPS→HTTP, SSRF checks inside the DNS resolver, a loopback-only playback relay behind a 192-bit token, and bounded manifests, connections, record sizes and import sizes.
5. **Secrets are kept out of logs.** `Secret` and `SecretUrl` render as `***`, URL redaction also covers percent-encoded secrets, and error types name fields, never values.
6. **Imports and restores are atomic.** Catalog and guide data stream into a staging database and are published in one transaction. Migrations are forward-only and tested from every historical schema version. Restore validates the schema, integrity and foreign keys before activation.
7. **Tests check behaviour.** Property tests compare chunked and buffered parsing, memory tests assert heap bounds on 100k-channel imports, real-mpv tests sample pixels, and headless UI scenarios run in isolated processes.
8. **Release engineering is treated as engineering.** SHA-pinned actions, locked Cargo graphs including the NSH-generated workspace, checksum-verified downloads, signing on a separate runner, package install-and-launch checks, and a Windows libmpv built offline from pinned sources.
9. **The AGENTS.md invariants hold where checked.** Focus is restored by content identity, overlays that hold channel indices are closed or rebuilt when the schedule changes, and one persistent player surface changes layout only.

## Consolidated findings

Priorities: **P1** before the next stable release, **P2** in the next improvement cycle, **P3** later hardening. They order the work; they are not CVSS ratings. No audit established a critical or remotely exploitable-by-default issue.

### P1

| ID | Finding | Evidence | Sources | Status |
|---|---|---|---|---|
| C01 | **Custom credential headers survive cross-origin requests.** M3U playlists can set arbitrary headers (Kodi properties, pipe syntax). On a cross-origin redirect or media segment, only five headers are removed (`authorization`, `cookie`, `proxy-authorization`, `sn`, `x-user-agent`). A playlist-supplied `X-Api-Key`, `X-Auth-Token`, `Referer` or `Origin`, and the Stalker portal `Referer`, reach the other origin. | `iptv-net/src/playback.rs:31-37,417-421`, `iptv-net/src/redirect.rs:16-22,110-114`, `iptv-providers/src/m3u/mod.rs:252-261,460-481`, `stalker/session.rs:497` | Codex A01, agy SEC-2 | Verified |
| C02 | **A committed screenshot shows a real provider's playlist**: real channel names, broadcaster logos, a broadcast video frame and real programme listings. This contradicts AGENTS.md and is in the public repository's history. | `docs/screenshots/tablet-guide.png`, `README.md:24-26` | Claude 1, Codex A03 (caption only) | Verified (image opened) |
| C04 | **Rust-only changes can skip the Android build on pull requests.** The path filter lists manifests, host files and packaging inputs but no `crates/**/*.rs`, and no other PR workflow compiles for Android. Pushes to `master` do build. | `.github/workflows/android.yml:34-41,59` | Codex A02 | Verified |
| C05 | **All guide sources share one 300 s / 512 MiB budget, with no retry.** One import context covers every source, and its elapsed time and bytes are cumulative. When the budget runs out, the check at the top of the source loop ends the whole import, so feeds that already loaded are discarded too. There is no retry loop; the next attempt is an hour later. A large guide on a slow link will likely fail every time. | `iptv-import/src/epg.rs:164,184`, `ctx.rs:47-61,108-117`, `iptv-core/src/import_limits.rs:43-49`, `iptv-net/src/client.rs:50-53`, `iptv-app/src/data/stored.rs:373` | Claude 3 | Verified |
| C06 | **No source offer for the FFmpeg/libmpv bundled in the DMG and AppImage.** The scripts copy Homebrew and Ubuntu libraries with licence texts and an inventory, but no corresponding source and no written offer. Windows does this correctly. Whether those builds enable GPL components cannot be told from the repo, and source is owed under the LGPL either way. This is a compliance judgement that deserves a legal check; the shipped artifacts were not inspected. | `tools/bundle-macos.py:159-214`, `tools/ci/package-linux.sh:81-87`, `tools/ci/linux-native-notices.py`, `docs/release.md:81` | Claude 2 | Verified from the scripts |

### P2: correctness and performance

| ID | Finding | Evidence | Sources | Status |
|---|---|---|---|---|
| C07 | **Player-card logos are missing for combined schedules.** The helper downcasts to `EpgSchedule` and returns `None` for `CombinedSchedule`, though `Schedule::channel_logo` supports both. There are 16 `downcast_ref` sites in `iptv-app/src` worth the same review. | `iptv-app/src/player_panels/card.rs:31-36` | Codex A06, Claude 12 | Verified |
| C08 | **Schedule lookups are linear.** `find_channel` scans all channels and allocates a key for each; its callers run per event, not per frame. `group_of` asks every group for its channel list and scans it: with the cached lists the app normally installs, that is about one pass over the channels plus one small allocation per group. Without the cache (a failed store read, or tests) `group_channels` copies the whole playlist order per group, which is the O(groups × channels) path. | `iptv-core/src/guide.rs:185-187`, `iptv-core/src/lists.rs:106-119`, `iptv-guide/src/lib.rs:704-725`, `iptv-app/src/data/organization.rs:44-52` | Claude 4, agy ARCH-3 | Verified; impact is modest on the normal path |
| C12 | **One SQLite connection for everything.** UI reads queue behind long publish transactions although WAL mode would allow a separate reader. The delay was not measured. | `iptv-app/src/store_service.rs:1-2,113-123` | Claude 9, agy ARCH-5 | Verified (structure) |
| C13 | **Custom-group order reset runs one transaction per channel.** Each `move_in_custom_group` call opens its own transaction, reads every channel ID of the source and rewrites every member. It runs on the store worker. | `iptv-app/src/organization_view.rs:275-287`, `iptv-store-sqlite/src/organise.rs:374-390` | agy QUAL-3 | Verified |
| C14 | **Ad hoc concurrency.** Each Stalker link resolve spawns a thread and a tokio runtime with a throwaway `CancelToken`, so zapping cannot cancel it. Import results are polled every 100 ms. The image loader keeps four workers, each with its own runtime. A dead mpv worker thread is not noticed, because draining ignores a disconnected channel. | `iptv-app/src/player.rs:418-431`, `data.rs:73,645-658`, `content_image.rs:48-52`, `mpv/mod.rs:647-651` | Claude 9, agy QUAL-4 | Verified |
| C15 | **Synchronous file writes on the UI thread.** The last channel is written on every zap, each settings change does a file fsync and a directory fsync, and search history is a non-atomic plaintext write. | `iptv-app/src/player.rs:236,517-527`, `settings.rs:238-279,314-317`, `search_view.rs:238-256` | Claude 10 | Verified |
| C39 | **Resolved Stalker links are always treated as public-only.** `StalkerLink::source` hard-codes `public_only: true`, and public-only destinations must be public addresses. A Stalker portal on a private network whose resolved streams are also private would import and then fail to play. | `iptv-guide/src/playback.rs:198-209`, `iptv-net/src/redirect.rs:65-72` | agy SEC-3 (second half) | Plausible; not reproduced |

### P2: architecture and maintainability

| ID | Finding | Evidence | Sources | Status |
|---|---|---|---|---|
| C16 | **`iptv-app` holds too much** (about 39k non-test lines): UI, mpv FFI for four systems, X11, keyring, backup, import orchestration, the store worker and a thread pool. None of the non-UI parts can be tested without NSH. | `crates/iptv-app/src/{mpv,data,backup.rs,store_service.rs}` | Claude 6, Codex A07, agy MAINT-3 | Verified (sizes) |
| C17 | **Hidden global state.** 34 `thread_local!` blocks in 30 files, about 63 globals counting static `Mutex`/`OnceLock`/atomics, and a `Ui` struct with 46 public fields. The test harness needs one process per scenario because of it. | `iptv-app/src/lib.rs:168-254,392-394`, `tests/app/isolated.rs:3` | all three | Verified |
| C18 | **Pure screen state depends on adapter types.** Three state modules import `iptv_store_sqlite::SourceId`, so they cannot move into core. | `playlists_view/state.rs:9`, `epg_sources_view/state.rs:11`, `guide_assign_view/state.rs:8` | Claude 8, Codex A05 | Verified |
| C19 | **Bus factor of one and bot-only review.** One author, 400 commits between 2026-11-09 and 2026-11-13, pull requests reviewed by bots or by nobody. | `git shortlog -sn`, `gh pr list` | Claude 14 | Verified |
| C20 | **The NSH fork is a path dependency outside the repo.** `Cargo.lock` cannot record its commit, the README clones the floating `tv-keys` branch while CI pins a SHA in two places, and one more Windows-only patch lives in `tools/patches/`. | `Cargo.toml`, `.github/actions/setup/action.yml:16`, `README.md:84` | all three | Verified |
| C21 | **String errors and one silent failure.** Backup uses `Result<_, &'static str>` and discards the underlying `io::Error` in about 80 places; `ProviderError` and `ImportError` have no `source()`. In the guide refresh path, a failed `NetClient::new` is dropped without a log line or a recorded failure. | `iptv-app/src/backup.rs:53`, `data.rs:843-870` | Claude 12, agy MAINT-4 | Verified |
| C41 | **Adding a provider type touches many files.** `SourceKind` is a closed three-variant enum, and Stalker-specific branches are spread across the crates. Acceptable for three fixed protocols; there is no plug-in point. | `iptv-providers/src/model.rs:13-17` | Claude 13 | Enum verified; spread reported |

### P2: testing, release and documentation

| ID | Finding | Evidence | Sources | Status |
|---|---|---|---|---|
| C23 | **One flaky UI scenario, one possible error misreport.** A scenario presses `Appearance` straight after `Settings` with no wait; Claude's audit saw it fail once in two full runs. Separately, a late FFmpeg `HTTP error 415` log line can overwrite the relay's transport error in `Clues`, so the user would be shown an HTTP status where a more specific cause was known; agy reports seeing the matching test fail under load. | `tests/app/personalization_appearance.rs:9-10`, `iptv-app/src/mpv/map.rs:273-286,313-318` | Claude 5, agy TEST-1 | First: missing wait verified, failure source-reported. Second: plausible |
| C24 | **Public release assets can be replaced.** The publish step uploads with `--clobber` to an existing release and does not check whether it is already public. | `.github/workflows/release.yml:265-269` | Codex A04 | Verified |
| C25 | **No platform trust on the desktops.** The Windows installer is unsigned. macOS notarization is wired into `release.yml` but runs only when a Developer ID certificate is configured; the secret names visible through `gh` on 2026-11-13 were only `ANDROID_*`. The shipped DMG itself was not inspected. | `tools/ci/package-macos.sh:6-21`, `release.yml:206-211`, `docs/release.md:78-80` | Claude 16, Codex A08 | Conditional path verified; secret listing as observed |
| C26 | **Native package inputs float.** macOS and AppImage builds take whatever Homebrew and apt provide at release time. | `.github/actions/setup/action.yml:24-29` | all three | Verified |
| C27 | **Device lifecycle is untested in CI, and some suites run on one platform only.** There is no emulator test for the Android video-surface rules, and the host has three Kotlin tests. The real-mpv suite is compiled on Linux only. Windows CI runs the app tests but skips those of five library crates and has no `cargo deny` or Rust cache. Linux and macOS run the full check. | `android.yml:83`, `iptv-app/src/mpv/mod.rs:71-80`, `tools/build-windows.ps1:30-41`, `.github/actions/setup-windows/action.yml`, `tools/ci/audit-runners.sh:8` | all three; agy TEST-2, DIST-4 | Verified |
| C28 | **Other test gaps.** No coverage measurement. One fuzz target multiplexes seven parsers and runs nightly only; seven seeds are committed but no grown corpus is kept; the HLS and DASH rewriters are not fuzzed. Some wall-clock assertions run in debug builds. | `fuzz/fuzz_targets/imports.rs:9-16`, `import-stress.yml:3-5`, `iptv-epg/tests/it/parse.rs:284` | Claude 5, 21; agy SEC-7 | Verified |
| C29 | **No provenance or signed checksums.** `SHA256SUMS` is unsigned, and there are no build attestations, SPDX or CycloneDX SBOM, or Gradle dependency verification. | `release.yml:266` | Claude 18 | Verified |
| C30 | **Documentation gaps and drift.** No architecture overview, CONTRIBUTING, CHANGELOG or SECURITY policy; code comments cite an external reference checkout; `docs/release.md` still says RPM and pacman get metadata checks only, though container install checks exist. The declared `rust-version = "1.89"` is never built in CI, and `locked-build-env.sh` requires `rustup`. | `docs/release.md:40-44`, `tools/ci/check-linux-packages.sh:23`, `Cargo.toml:22`, `tools/ci/locked-build-env.sh:6` | Claude 19, Codex A10, agy DIST-1/2 | Verified |
| C31 | **Localization and accessibility are partial.** Long help, plurals, dates and some secondary screens stay English by the project's own account; some accessibility labels are untranslated literals; the bundled font has no CJK. Claude's "about a quarter of literals translated" was not confirmed, because many strings reach `tr()` indirectly. No audit exercised a screen reader. | `docs/localization.md:13-16`, `iptv-app/src/play_bar.rs:373` | Claude 20, Codex A10 | Partly verified |

### P3: hardening and latent issues

| ID | Finding | Evidence | Sources | Status |
|---|---|---|---|---|
| C03 | **`favorites::reorder` would not persist in "All playlists" mode, but nothing can call it there.** It writes only when a single source is shown. Its only callers sit behind a menu item that is disabled in combined mode and a check that a source is shown. A latent inconsistency with its sibling functions, not a user-visible bug. | `iptv-app/src/favorites.rs:461-475`, `organization_view.rs:195,224`, `menu_view.rs:90-101,192` | agy ARCH-2 | Code verified; not reachable at this revision |
| C10 | **Modal state is enumerated by hand.** `pointer_moved` checks twelve modal signals but not `custom_groups` or `channels_view`, and `toggle_panels` omits `custom_groups`. The play bar is deliberately layered above the description popup and below custom groups, so the only candidate for a visible glitch is the bar appearing over the channel list in fullscreen, which was not reproduced. The real cost is that each new overlay must be added in several places. | `iptv-app/src/lib.rs:736-747,757,791-804`, `play_bar.rs:75-91` | agy QUAL-2, MAINT-1 | Omissions verified; effect not reproduced |
| C11 | **Group vectors can differ in length on duplicate group IDs, but the app never passes duplicates.** `from_catalog` builds `group_keys` and `group_names` by different rules, so duplicate IDs would cause an out-of-bounds index. Every production caller reads groups back from SQLite, where `(source_id, id)` is the primary key and inserts use `INSERT OR IGNORE`. | `iptv-guide/src/lib.rs:103-117,157-161,299-303,375`, `iptv-app/src/data.rs:943-950`, `iptv-store-sqlite/src/catalog.rs:37-39` | agy QUAL-1 | Mechanism verified; not reachable at this revision |
| C22 | **Some dense one-line code passes `fmt --check`.** Across tracked Rust under `crates/` there are 43 lines over 200 characters, 8 over 300 and 1 over 400. rustfmt gives up on such chains without reporting. | `custom_groups.rs:622-640`, `organization_view.rs:429-470`, `library_preferences.rs:43-123` | Claude 11 | Verified (counts re-measured) |
| C32 | **mpv hardening options warn and continue.** If `config=no`, `load-scripts=no`, `ytdl=no` or `autoload-files=no` cannot be set, initialization logs a warning and proceeds. The unit test asserts `load-scripts` and `ytdl` only, not `config` or `autoload-files`. | `iptv-app/src/mpv/handle.rs:100-109,203-222` | Codex A10 | Verified |
| C33 | **Stalker credentials follow the host, not the origin.** `is_portal_owned` matches the host on any port, by documented design, so a stream on another port of the portal's host receives the portal's cookie and bearer token. | `iptv-providers/src/stalker/parse.rs:617-632` | agy SEC-3 (first half) | Verified; deliberate, low risk |
| C34 | **Android player has no relay-only guard.** Desktop mpv blocks any URL outside the relay prefix; ExoPlayer's `DefaultHttpDataSource` does not. This matters only if a manifest rewriter misses a URL, and no such miss was confirmed. | `hosts/android/.../PlayerComponent.kt:185-189` | agy SEC-1 | Verified as defence-in-depth |
| C35 | **Android release settings.** Release builds fall back to the debug key when no keystore is configured (intended for local installs), and `<profileable android:shell="true"/>` ships in release. | `hosts/android/app/build.gradle.kts:45-46`, `AndroidManifest.xml:26` | Claude | Verified |
| C36 | **The transport would bypass a proxy if one were ever configured.** `NetConfig::proxy` is ignored for public-only requests and for the playback relay. The app has no proxy setting today, so nothing is bypassed in practice. | `iptv-net/src/client.rs:121-130`, `playback.rs:229`, `iptv-app/src/settings.rs:358-369` | agy SEC-4 | Verified as an API limitation |
| C37 | **Portable backups are plaintext.** Documented and warned about in the UI. An optional passphrase would protect exported credentials. | `docs/security.md:48`, `iptv-app/src/backup.rs:709` | Codex A10, agy SEC-5 | Verified |
| C40 | **Schedules are built on the UI thread.** `EpgSchedule` holds an `Rc` and a non-`Send` closure, so `from_catalog` and the organisation sort cannot move to a worker. | `iptv-guide/src/lib.rs:98,121`, `iptv-app/src/data.rs:477` | agy ARCH-4 | Verified |
| C38 | **Smaller items.** `setlocale` is called on every `Mpv::new`. The image cache is FIFO, does not de-duplicate and panics on a poisoned lock. Win32 and JNI callbacks lack `catch_unwind` (reported). `Secret` is not zeroized. Backup restore accepts Windows reserved names (reported). The playback relay ignores extra root certificates, so a private-CA server can import but not play. A local playlist file is opened without `O_NONBLOCK`, so a FIFO swapped in after the first check would block the open. Three `getrandom` versions are in the lock file. The IP classifier omits `192.88.99.0/24` and a few IPv6 special ranges. | `mpv/handle.rs:21-25`, `content_image.rs:28-38,71-119`, `mpv/win.rs:76-89`, `android_transport.rs`, `iptv-providers/src/secret.rs:11`, `iptv-import/src/backup_manifest.rs:29-41`, `iptv-net/src/playback.rs:225`, `iptv-net/src/file.rs:32-45`, `iptv-app/Cargo.toml:35`, `content_url.rs:45-76` | agy QUAL-5/6, MAINT-5, SEC-6, SEC-8; Claude | Verified except where marked reported |

## Claims corrected or rejected

These statements appear in a source audit, or in the first revision of this report, and do not match the code at `8c259ee`.

| Claim | Source | What the code shows |
|---|---|---|
| "Zero real provider URLs, channel lists, or credentials anywhere in the repository or git history." | agy §5.1 | False. See C02. |
| Search results and the favourites editor keep stale channel indices after a schedule swap, and search has no effect watching `data_version`. | agy ARCH-1; first revision C09 | Rejected. `set_schedule` closes the editor (`iptv-app/src/lib.rs:424`), and search rebuilds its indexes and re-runs the query on every `data_version` change (`search_view/view.rs:35-42`). `ResultId` does hold indices, which is minor design debt. |
| Favourite reordering is silently lost in "All playlists" mode. | agy ARCH-2; first revision C03 at P1 | The persistence gap exists but is unreachable from the UI. Kept as latent (C03, P3). |
| A duplicate `GroupId` in the catalog causes an out-of-bounds panic. | agy QUAL-1 | The mechanism exists but no production path can supply duplicates (C11, P3). |
| DASH `xlink:href` is neither blocked nor rewritten. | agy SEC-1 | False. `href` and `xlink:href` are in the rewritten attribute list (`playback/dash.rs:73-79`). |
| Lowercase or single-quoted HLS `uri=` bypasses the relay on Android. | agy SEC-1 | The rewriter is case-sensitive (`playback.rs:796`), which is true. No evidence was given that any player fetches such an attribute; HLS attribute names are upper-case by specification. Not accepted as an exploit; kept only as C34. |
| `M3uParser::feed` has no line-length cap. | agy SEC-7 | False. `feed` delegates to `try_feed`, which checks the record-size limit before extending the buffer (`m3u/mod.rs:120-124,150-154`). |
| `backup.rs:369-380` calls `p.parse::<u16>().unwrap()` and panics on six-digit ports. | agy MAINT-4 | No such code. The only `parse::<u16>` in the workspace is `iptv-settings/src/validate.rs:138`, which uses `is_ok_and`. |
| `.nsh/rev` pins NSH for CI; an iOS Swift/ObjC host lives in `hosts/ios`. | agy header, ARCH-6 | Neither path exists. The NSH pin is in the composite actions; `hosts/` contains only `android`. |
| `public_ip` omits `192.0.0.0/24`. | agy SEC-6 | It is covered (`content_url.rs:56`). The other ranges it lists are genuinely missing (C38). |
| `release.yml` "already signs, notarizes, and staples macOS builds", so `docs/release.md` is wrong. | agy DIST-1 | The steps exist but are conditional on a certificate (C25). The doc is out of date about what the scripts can do, not shown to be wrong about what shipped. |
| The custom-group reset loop runs on the UI thread. | agy QUAL-3 | It runs inside a store-worker job (C13). |
| 1,241 automated tests; per-crate executed counts; 25 findings. | agy §1, §2 | The two audits that ran the suites agree on about 1,133 Rust tests (273 core, 593 other library crates, 209 app unit, 58 app scenarios). agy's core count matches; several other per-crate counts do not. Its report labels 32 findings. |
| 31 `thread_local!` blocks across 27 files. | agy MAINT-2 | 34 blocks in 30 files. |
| `organization_view.rs` is 1,286 lines and `search_view.rs` 867. | agy MAINT-3 | 470 and 715. The larger figures appear to add each module's submodule files to the parent. |
| The mpv restrictions "are verified". | Claude | Only two of the four, and only in a test (C32). |
| No Medium security findings. | Claude | C01 is at least Medium. |
| `group_of` costs about 10⁸ operations per focus change. | Claude 4; first revision C08 | Only on the fallback path without cached group lists (C08). |
| 21 `downcast_ref` calls in the app; about 35 lines over 400 characters. | Claude 12, 11 | 16 calls in `iptv-app/src`; one line over 400 characters, 43 over 200 (C07, C22). |
| A user who configures a proxy still connects directly for media. | first revision C36 | The app exposes no proxy setting (C36). |

## Validation results across the audits

| Check | Claude | Codex | agy |
|---|---|---|---|
| `cargo test -p iptv-core` | 271 + 2 pass | 271 + 2 pass | reported pass |
| wasm32 build of the portable crates | pass (rustup toolchain) | blocked: no wasm target on system Rust | not reported |
| Library crate tests | 866 pass including core | 593 pass excluding core | counts unreliable |
| `cargo clippy -D warnings` | 0 warnings | not run | reported clean |
| `cargo fmt --check`, `cargo deny check` | both clean | not run | not reported |
| `cargo build -p iptv-app` | pass | pass | not reported |
| `iptv-app` tests | 209 unit pass; scenarios 57/58 then 58/58 | 209 unit and 58 scenarios pass | reports an mpv test failing under load |
| Packaging helper tests (Python) | not run | 48 pass | reports 51 |
| Device runs (`tab36`, `tv36`), screen readers, fuzz campaigns | not run | not run | not run |

Codex notes that its app test run did not set `IPTV_REQUIRE_PLAYER_TESTS=1`, so a pass does not prove every native player path ran. CI does set it (`ci.yml:28`).

## Recommended sequence

1. **Before the next release (P1).**
   - Scope playlist-supplied and portal headers to their original origin, with an explicit cross-origin allowlist and two-origin tests for redirects, HLS and DASH (C01).
   - Replace the tablet screenshot with a synthetic capture and decide whether to rewrite history (C02).
   - Add a cheap Android `cargo check` for Rust changes on pull requests (C04).
   - Give each guide source its own budget, switch to an idle timeout, keep feeds that already loaded, and retry transient failures with backoff (C05).
   - Publish corresponding source or a written offer for the DMG and AppImage media libraries, after a legal check (C06).
2. **Next cycle: correctness and responsiveness.**
   - Use the `Schedule` trait instead of downcasts, starting with the logo helper (C07).
   - Add a read-only WAL connection and batch the custom-group reset into one transaction (C12, C13).
   - Move file writes off the UI thread and make them atomic (C15).
   - Share one runtime and real cancellation for link resolves, and treat a disconnected mpv channel as a player failure (C14).
   - Decide the policy for resolved Stalker links on private networks and test it (C39).
   - Index key→channel and channel→group once per schedule if profiling on a large playlist shows a need (C08).
3. **Next cycle: release trust and tests.**
   - Refuse to replace assets on a public release (C24).
   - Configure notarization and Windows signing, or label those artifacts as previews (C25).
   - Pin or self-build the macOS and Linux media libraries, which also settles C06 (C26).
   - Add the missing wait, and keep the relay's transport error when a later log line arrives (C23).
   - Add an Android emulator smoke test, run the full crate set and `cargo deny` on Windows, measure coverage, and fuzz the manifest rewriters (C27, C28).
   - Attest builds and sign the checksum file (C29).
4. **Structural work, incrementally.**
   - Split mpv and orchestration out of `iptv-app`, move globals into an owned context, and introduce core ID types so screen state can move into `iptv-core` (C16 to C18).
   - Keep one NSH revision file read by CI, `check.sh` and the README, and upstream the fork (C20).
   - Write `docs/architecture.md`, add CONTRIBUTING, CHANGELOG and SECURITY, and bring `docs/release.md` up to date (C30).
   - Get a human reviewer on core and app changes (C19).
5. **Hardening (P3).**
   - Fail initialization when an mpv restriction cannot be set, and assert all four in the test (C32).
   - Add an Android relay-only data-source guard (C34).
   - Remove the debug-key fallback and `profileable` from release builds (C35).
   - Replace the hand-enumerated modal checks with one "top modal" query (C10).
   - Make the latent cases impossible by construction: route `favorites::reorder` through the owner lookup or assert a source, and build group keys and names in one pass (C03, C11).
   - Offer passphrase-protected backups (C37).

## Finding map

| Consolidated | Claude | Codex | agy |
|---|---|---|---|
| C01 | n/a | A01 | SEC-2 |
| C02 | 1 | A03 | (denied) |
| C03 | n/a | n/a | ARCH-2 |
| C04 | n/a | A02 | n/a |
| C05 | 3 | n/a | n/a |
| C06 | 2 | n/a | n/a |
| C07 | 12 | A06 | n/a |
| C08 | 4 | n/a | ARCH-3 |
| C09 (withdrawn) | n/a | n/a | ARCH-1 |
| C10 | n/a | n/a | QUAL-2, MAINT-1 |
| C11 | n/a | n/a | QUAL-1 |
| C12 to C15 | 9, 10 | n/a | ARCH-5, QUAL-3, QUAL-4 |
| C16 to C18 | 6, 7, 8 | A05, A07 | MAINT-1/2/3 |
| C19 to C22 | 11, 12, 14, 15 | A09 | ARCH-6, MAINT-4 |
| C23 | 5 | n/a | TEST-1 |
| C24 | n/a | A04 | n/a |
| C25, C26 | 16, 17 | A08, A09 | DIST-3 |
| C27, C28 | 5, 21 | A08 | TEST-2, TEST-3, DIST-4, SEC-7 (fuzzing half) |
| C29 to C31 | 18, 19, 20 | A10 | DIST-1, DIST-2 |
| C32 to C38 | low-severity notes | A10 | SEC-1, SEC-3 (first half), SEC-4, SEC-5, SEC-6, SEC-8, QUAL-5, QUAL-6, MAINT-5 |
| C39 | n/a | n/a | SEC-3 (second half) |
| C40 | n/a | n/a | ARCH-4 |
| C41 | 13 | n/a | n/a |

Not carried over: agy QUAL-2's note that overlay z-order and key routing disagree for the remote editor and organisation screens (the order does differ, `lib.rs:746-748` against `input.rs:43-48`, but both map to the same input context, and no effect was shown); agy ARCH-1's note that the channel list overlay closes on every data refresh; agy SEC-8's mention of the documented `RUSTSEC-2026-0192` ignore, which is a tracked decision, not a defect.

## Review and revision

The first revision of this report was reviewed for accuracy by three reviewers, each read-only and each asked to check it against the three sources and the code.

| Reviewer | Result |
|---|---|
| Fresh Claude agent | 15 issues. It also re-checked 17 findings that were marked unverified or plausible. Scored the repository 7.5. |
| Codex CLI | 17 issues. Scored the repository 7.5. |
| agy CLI | 11 issues, written against the first revision. Scored the repository 7.8 and called 7.5 fair. |

Every review point was checked against the code before it was applied. The Codex and Claude reviews held, with one exception: Codex could not confirm the signing-secret state offline, so C25 now states how it was observed. The agy review arrived after those changes and was written against the first revision. Its numbered issues were accurate and mostly repeated the other two; it added the corrected line citations in C18 and C38, the `MAINT-3` line-count row, and the routing-order note above. Its closing list of "claims checked and found correct" described findings that are not in this report (for example, an Android workflow triggering on the wrong branch) and was disregarded. The main changes in this revision:

- **Withdrawn:** C09 (stale indices). Both reviewers showed the editor is closed and the search re-run when the schedule changes.
- **Downgraded to latent (P3):** C03 (favourite reorder), C10 (modal checks) and C11 (duplicate group IDs). Each mechanism is real, but no path in the app reaches it, or the visible effect is unconfirmed.
- **Narrowed:** C08 (the heavy cost is a fallback path), C36 (the app has no proxy setting), C27 (desktop runtime coverage exists) and C32 (the test covers two of four options).
- **Upgraded from unverified to verified:** C05, C06, C12 to C15, C19, C28 and C29.
- **Added:** C39 (private-network Stalker links), C40 (schedule built on the UI thread), C41 (closed provider enum), plus private-CA playback, the file-open window and the Windows CI gaps.
- **Corrected counts and attributions:** downcast sites, long lines, agy's finding count, and three rows of the corrections table that misquoted or misattributed a source.
- **Scores:** code quality went from 7.5 to 8, because fewer of the reported bugs are real. The overall score stays at 7.5 and is now stated as a judgement.
