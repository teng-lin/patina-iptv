# Consolidated Review & Fix Plan (Packet-Only Pass)

> **Scope & Verification Boundary:** This review is based **solely on the supplied defect-report packet** (`0e197073a272eceb0631e4d53b08d2823950c416`, 2026-11-16). Independent source verification, file inspection, and command/test execution are **explicitly outside this packet-only pass**.

## Calibration Criteria

1. **Ordinary production correctness over exotic API inputs:** Real user/server/concurrency failures (`P0`/`P1`) take precedence over `i64::MIN`/`u32::MAX`/forged-newtype inputs that require caller misuse (`P3`).
2. **`Debug` exposure ≠ production log leak:** Missing `Debug` redaction violates the crate confidentiality boundary (`docs/security.md`), but without an observed production log sink it is defense-in-depth hardening (`P2`), not an active `P0`/`P1` outage.
3. **Reproduced vs. inferred:** Executable probes (`runtime-probe`, `app_image_reply.rs`, `stale_page.rs`, `net-slow-recording-probe.rs`, ctypes `libmpv` probe, SQLite WAL probe) are distinguished from source-traced or inferred effects (e.g., `iptv-player-mpv-003` deadlock, `iptv-runtime-002` inconclusive durable-state probe).
4. **Architectural contracts preserved:** Keep intentional `Rc`/local futures in `iptv-app`/`iptv-import`/`iptv-guide`, `wasm32-unknown-unknown` portability in `iptv-core` and leaf portable crates, persistent surface 0 / no hide-or-detach in `iptv-app`/`iptv-player-mpv`, and existing-harness test consolidation (`AGENTS.md`).

**Priority Scale:**
- **P0 (Critical):** Active security/parental authorization bypass or durable unauthorized state mutation.
- **P1 (High):** User-visible production bug, hang, data corruption, stale UI/guide state, or resource starvation under normal operation.
- **P2 (Medium):** Defense-in-depth (`Debug` redaction, TOCTOU on private media dir), untrusted-feed/server panic hardening, or experimental/non-shell API gaps.
- **P3 (Low):** Exotic public-API boundary inputs, internal schema-table tampering, or compile-time API hygiene.

---

## Finding-by-Finding Disposition

### `iptv-runtime`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-runtime-001** | **Correct** (fix scope) | **P0** | **Reproduced** (`runtime-probe`: locked adult channel returned by `inspect_channel` and `query_channels`). Fix must capture the owner's parental snapshot at acceptance and **integrate allowed unlocks/blocks/adult-membership *before* pagination and cursor generation**—not filter an already-paginated slice (which leaks page presence/counts and breaks cursor stability). Apply uniformly to channel, programme, candidate, audit, and repair reads. |
| **iptv-runtime-002** | **Correct** (evidence & fix) | **P0** | Source-confirmed interleaving; note that the supplemental `recording_admission.rs` probe was inconclusive on durable state because `update_recording` intentionally does not update title. **Fix challenge:** Must capture parental authority + source incarnation at acceptance and **recheck `native_allowed` + source/incarnation inside the SQLite writer transaction** (`schedule_recording`, `update_recording`, recurring first-job, re-enable). **Crucially, preserve existing accepted background jobs**—once durably accepted, later UI relock/expiry must not cancel or block them. Coordinate with `iptv-store-sqlite-003`. |
| **iptv-runtime-003** | **Merge** (with `iptv-guide-002`) | **P1** | Source-confirmed in `data/pages.rs::arrived`; portable collision **reproduced** in `stale_page.rs`. `iptv-runtime` must supply a monotonic `schedule_generation` and per-request `ticket` to `iptv-guide` so `Pages::insert` rejects replies from replaced schedules or prior evictions without adding ambient globals to `iptv-guide`. |
| **iptv-runtime-004** | **Accept** | **P1** | Source-confirmed (`data/pages.rs:67-72`): DB read errors are turned into `GuideStore::default()` and cached as loaded pages, permanently hiding guide data and skewing catch-up. Fix must carry `(generation, ticket)` into `complete_failure` so a stale error cannot clear a newer retry, while keeping empty-result caching distinct from read errors. |
| **iptv-runtime-005** | **Correct** (severity) | **P2** | Source-confirmed multi-snapshot read + `.expect("source checked")` in `inspection/probe.rs:591-638`. Inferred race under concurrent source edit/delete; worker catches panics so it fails the probe rather than crashing the process. Wrapping in one `guide_repair_snapshot` read transaction (and auditing `stored::guide_inputs`/library/org compound reads) is clean. |
| **iptv-runtime-006** | **Correct** (priority) | **P2** | Source-confirmed missing post-await grant check in `vod_play`, `recording_file`, `open_recording`. Downgraded from Must-fix because `iptv-app`'s `player::publish_source` already performs a synchronous final `source_allowed` check (tested by `tests/app/recordings_transport.rs:493`). Still worth fixing for direct/experimental `iptv-runtime` consumers, and must close any newly created recording relay on final gate failure. |
| **iptv-runtime-007** | **Accept** | **P2** | Source-confirmed (`inspection/probe.rs:511-517, 1092`): `validate_candidate` mints a fresh 60s budget instead of forwarding `worker_control.cancel` and `deadline`. Forward both and bound `current_configuration`'s queued analysis read. |

### `iptv-guide`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-guide-001** | **Correct** (priority) | **P2** | **Reproduced** (`guide_context_debug_leaks=true`), but `Debug` exposure is not proof of a production log leak. Batch manual `Debug` redaction for `StalkerLink` and `PlaybackContext` (`M3u` Referrer/headers) into the cross-crate secret-redaction package. |
| **iptv-guide-002** | **Accept** (merge with `iptv-runtime-003`) | **P1** | **Reproduced** (`stale_page.rs`). Add `generation` and `ticket` to `PageRequest` and verify both in `Pages::insert` before publishing. Keep `iptv-guide` portable (`wasm32`, no platform APIs or ambient globals); `iptv-runtime` owns the counters. |

### `iptv-store-sqlite`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-store-sqlite-001** | **Accept** | **P1** | **Reproduced** via SQLite WAL probe (`cached identity 'a'/'old-a' now reads [('New B',)] via reused id 1`). Pinning one deferred read snapshot via `is_autocommit().then(|| conn.unchecked_transaction())` across `load_epg`, `for_each_epg_title`, and `epg_guide` preserves nested snapshot callers and avoids blocking writers. |
| **iptv-store-sqlite-002** | **Correct** (priority) | **P3** | SQL probe confirmed `[0, 2]` passes `COUNT(*) == MAX(version)`. In ordinary production, `schema_migrations` is only populated internally from 1; nonpositive rows require external tampering or corrupt fixtures. Adding `count > 0 && min != 1` is trivial hardening. |
| **iptv-store-sqlite-003** | **Accept** | **P1** | Source-confirmed `Deferred` read-then-write transactions in `recordings.rs`, `overlay.rs`, `organise.rs` that fail immediately with `SQLITE_BUSY_SNAPSHOT` under WAL contention. Switch to `TransactionBehavior::Immediate`, move pre-transaction reads (e.g., `library_preferences::move_group_to`) inside the transaction, and keep read-only snapshots `Deferred`. |

### `iptv-player-mpv`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-player-mpv-001** | **Accept** | **P1** | Source-confirmed control-flow bug (`worker.rs:328, 355, 374`, `mod.rs:865`): pre-load guard refusals return `Note::TransportError` without decrementing `State.loads`, leaving the UI buffering. (Note: the 10 baseline player test failures are context, not proof this caused all 10.) Adding `Note::LoadRejected(&'static str)` that decrements `st.loads` and calls `fail` only on the newest load—including hook-substitution failure and guard retirement—preserves fail-closed behavior and normal `TransportError` evidence gathering. |
| **iptv-player-mpv-002** | **Accept** | **P1** | **Reproduced** with ctypes `libmpv.so.2` probe: consuming a wakeup leaves the next `mpv_wait_event(..., -1.0)` blocked even when command responses were already sent. Using `timeout = if sent { 0.0 } else { -1.0 }` drains pending events nonblockingly and notifies the shell immediately. |
| **iptv-player-mpv-003** | **Investigate** | **P2** | Source-confirmed unbounded `JoinHandle::join` in `Inner::drop`; deadlock is **inferred, not reproduced**. **Fix challenge:** UI teardown reordering **requires native API/lifetime proof** before implementation: (1) verify `libmpv` thread-safety for calling `mpv_render_context_free` on the UI thread while a worker thread is mid-`mpv_command` on the same `mpv_handle`; (2) prove what keeps `mpv_handle`, wakeup pointers, and render callbacks alive until *both* `free_render()` and the worker exit; (3) bound reaper threads under repeated multiview open/close; (4) preserve persistent surface 0. |

### `iptv-app`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-app-001** | **Accept** | **P1** | **Reproduced** (`app_image_reply.rs`: queued cache hit returns `Some(value)` without sending `wanted`, yielding `Canceled`). Sending on both paths in `content_image.rs` fixes row placeholder stalls while preserving `Rc`/local futures. |
| **iptv-app-002** | **Accept** | **P1** | Source-traced (`guide_assign_view.rs:159`): old assignment completion unconditionally calls `close(ui)` and can close a newly opened picker. Gating `close(ui)` on `next_request()` token match aligns save completion with the existing open-load check. |
| **iptv-app-003** | **Accept** | **P1** | Source-traced (`guide_repair_view.rs:277, 288`, `guide_repair_screen.rs:58`): `Screen::new` resets generation to 0, so a slow reply from a closed screen can match a new screen at the same generation. Check `control.cancel.is_cancelled()` (or a shell-owned monotonic instance ticket) across *all* repair callbacks, and ensure stale successful preview replies release their plan before dropping. |
| **iptv-app-004** | **Accept** | **P1** | Direct source proof (`menu_view.rs:485` renders hardcoded fixture text while `header.rs:192` reads `EpgSchedule`/`CombinedSchedule`). Extract and share `header.rs`'s lookup with `ui.guide_data()` invalidation. |
| **iptv-app-005** | **Accept** | **P1** | Source-traced (`player_keys.rs:171`, `chooser.rs:275`, `video.rs:169`): Stop/Long Back ignores VOD (`vod` is `Some` while `playing`/`recorded` are `None`), and sleep timer bypasses `stop_playback`, leaving VOD/recorded/early-start sources open. Route all active variants through `stop_playback` and clear `ui.player.early` in `player::stop`, preserving persistent surface 0. |

### `iptv-search`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-search-001** | **Accept** | **P1** | **Reproduced** in linked state probe and confirmed in `iptv-app/src/search_view.rs:283, 360`. Directly violates `AGENTS.md` ("focus is restored by content identity, never by list index"). Carry opaque `Schedule::channel_key` alongside transient `usize` indices in hits/`ResultId` and update `iptv-app` adapters together. |
| **iptv-search-002** | **Correct** (priority) | **P3** | **Reproduced** only with `start = i64::MIN` and `now = i64::MIN`. Exotic public-API boundary inputs that ordinary Unix timestamps do not reach. Harden with checked/saturating arithmetic at low priority. |

### `iptv-providers`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-providers-001** | **Correct** (priority) | **P2** | **Reproduced** with `timestamp_now = i64::MAX` (`server_clock_panicked=true`). Requires a malformed/malicious server response (`i64::MAX` or `UTC+2147483647:00`). Widen subtraction to `i128` and validate unsigned hour/minute bounds lazily before arithmetic. |
| **iptv-providers-002** | **Correct** (priority & fix) | **P2** | **Reproduced** (`custom_header_debug_leaks=true`, `portal_debug_leaks=true`), but `Debug` exposure is not a production log leak. **Fix challenge:** Redact all `HttpRequest` header values by default and implement manual redacting `Debug` on `StalkerPortal`—**do not** reject URL userinfo in `StalkerPortal::new` unless confirmed unsupported, as rejecting it could break existing user-configured portals. |
| **iptv-providers-003** | **Correct** (fix) | **P1** | **Reproduced** (`...play?id=7` + `?token=synthetic-new` → `...play?id=7?token=synthetic-new`). Real Stalker playback bug. **Fix challenge:** The report's proposed `url::Url::parse` + `query_pairs_mut().extend_pairs(...)` **re-encodes** existing query parameters, which can break provider tokens/signatures expecting raw encoding. Repair the query string while **preserving raw query encoding** (split off `#fragment`, strip leading `?` from reply, join with `&` or `?` based on existing query presence, and handle duplicate keys without re-encoding untouched pairs). |

### `iptv-epg`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-epg-001** | **Correct** (priority) | **P2** | **Reproduced** on `xmltv_ns` value `4294967295`. Untrusted external feed input; trivial 1-line fix (`checked_add(1)`). |
| **iptv-epg-002** | **Correct** (priority) | **P2** | **Reproduced** on synthetic `(u32::MAX-1, u32::MAX+2)`. Requires ~136-year compact slots, unlikely in normal EPGs but possible on untrusted feeds; 1-line fix (`i64::from(last.start) + i64::from(last.dur)`). |
| **iptv-epg-003** | **Correct** (priority) | **P3** | **Reproduced** on `20260301123099 +0000`. Only affects malformed seconds `61..=99`; trivial fix (check `second > 60` before `.min(59)`). |

### `iptv-import`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-import-001** | **Accept** | **P1** | **Reproduced** (`waiters_woken=32/33; first_waiter_woken=0`), plus dropped `Cancelled` futures leak registrations and evict live waiters even below 33 concurrent tasks. Give each `Cancelled` a registration ID and unregister in `Drop`, preserving portable local futures (no Tokio/`Send` bounds). |
| **iptv-import-002** | **Accept** | **P2** | Source-confirmed (`stalker.rs:395-396, 449`): `Crawled::warnings` accumulates unboundedly across up to 1000 pages (~1M strings) before `into_catalog` caps it. Enforce `MAX_STORED_WARNINGS` during page merge and count `dropped`. |

### `iptv-library`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-library-001** | **Correct** (priority) | **P2** | **Reproduced** (`library_edit_debug_leaks=true`), but `Debug` exposure is not a production log leak. Batch into the cross-crate `Debug` redaction package. |
| **iptv-library-002** | **Correct** (priority) | **P2** | **Reproduced** (`library_feed_1001_count=2` when syncing 1000 provider URLs after 2 user guides). Extreme playlist edge case, though `next_user_feed` overflow and silent ID collision are real bugs. Enforce `MAX_PROVIDER_FEEDS` (999) + checked `next_user_feed` returning a typed `Result`, preserving existing config atomically on error. |

### `iptv-net`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-net-001** | **Correct** (priority) | **P2** | Source-confirmed TOCTOU between `symlink_metadata` and `File::open` in `recording_file.rs:76, 80` (and missing `O_NONBLOCK` / descriptor type check in `recording.rs:142`). Requires local mutation of the private media directory; not dynamically reproduced. Reuse `src/file.rs` open discipline (`O_NOFOLLOW | O_NONBLOCK` on Unix, `meta.is_file()` on opened handle, platform private-file policy on Windows). |
| **iptv-net-002** | **Accept** | **P1** | **Reproduced** (`net-slow-recording-probe.rs`): 4 stalled capability-authorized clients exhaust all serving slots indefinitely. Use bounded chunk reads + 15-second idle write timeout per header/chunk write, matching live/playback relays without capping total recording duration. |

### `iptv-core`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-core-001** | **Correct** (priority) | **P2** | **Reproduced** (`core_play_source_debug_leaks=true`, `core_wizard_event_debug_leaks=true`), but `Debug` exposure is not a production log leak. Batch into the cross-crate `Debug` redaction package while preserving `wasm32` portability. |
| **iptv-core-002** | **Correct** (priority) | **P3** | **Reproduced** via `membership.rows.clear()`, which is caller misuse of public fields rather than normal app flow. Making `rows` and `query` private with slice/`&str` getters is clean API hygiene. |
| **iptv-core-003** | **Correct** (priority) | **P3** | **Reproduced** via `move_focus(0)` on a heading-only menu. Exotic API input; `if step == 0 { return; }` is a 1-line fix. |

### `iptv-settings`

| ID | Disposition | Priority | Reason & Fix Challenge |
|---|---|---|---|
| **iptv-settings-001** | **Correct** (priority) | **P3** | **Reproduced** via `step(i64::MAX)` and `clamp_to_grid(1, 0, 10, 0)`. Exotic public-API inputs; built-in catalogue grids have valid non-zero steps. Saturating `steps.saturating_mul(step)` and validating custom grid schemas is low-priority hardening. |
| **iptv-settings-002** | **Correct** (priority) | **P3** | Source-confirmed public `.0` on typed key newtypes. Exotic cross-crate misuse; making `.0` `pub(crate)` with `as_str()` is low-priority API hygiene. |

### `iptv-sqlcipher-init`
- **0 findings.** Clean review; retain the 5-second export polling loop and `sqlcipher-src` C symbol assumption as a dependency-upgrade watchpoint.

---

## Consolidated Fix-Plan Outline

> All regression tests below must be added to **existing** crate unit modules or existing integration harnesses (`tests/it/`, `tests/app/`) per `AGENTS.md` (no new top-level `tests/*.rs` executables).

### Package 1: Parental Authorization & Recording Transaction Integrity (**P0**)
- **Scope:** `iptv-runtime-001`, `iptv-runtime-002`, `iptv-store-sqlite-003` (recordings), plus `iptv-runtime-006` (experimental direct API consistency).
- **Changes:**
  1. In `iptv-runtime/src/inspection.rs`, capture the owner's parental snapshot at acceptance; enforce `native_allowed` (policy, adult membership, explicit blocks, active unlocks) **before pagination/cursor generation** across channel, programme, candidate, audit, and repair queries.
  2. In `iptv-runtime/src/recordings.rs` and `recordings/rules.rs`, capture parental authority + source incarnation at acceptance and recheck `native_allowed` + source/incarnation **inside the SQLite writer transaction** (`Immediate` per `iptv-store-sqlite-003`) for one-shot, update, recurring-first-job, and re-enable mutations. **Do not** revoke or block already-accepted background recording jobs on later UI relock.
  3. In `iptv-runtime/src/vod.rs` & `recordings.rs`, add final post-await grant/parent checks to `vod_play`, `recording_file`, and `open_recording`, closing any newly created relay on failure.
- **Tests:** Extend `tests/it/parental_access.rs` (paginated/direct/revoked-mid-flight inspection; writer-barrier relock during recording schedule/update/rule-enable while proving existing accepted jobs continue; direct `vod_play`/`open_recording` revocation) and `tests/it/concurrency.rs` (`Immediate` transaction contention).

### Package 2: SQLite Snapshot Consistency & Guide Paging Identity (**P1**)
- **Scope:** `iptv-store-sqlite-001`, `iptv-store-sqlite-003` (`overlay.rs`, `organise.rs`), `iptv-guide-002`, `iptv-runtime-003`, `iptv-runtime-004`.
- **Changes:**
  1. Pin one deferred read snapshot (`is_autocommit().then(|| conn.unchecked_transaction())`) across `load_epg`, `for_each_epg_title`, and `epg_guide`; switch read-then-write mutations in `overlay.rs` and `organise.rs` (including `move_group_to`'s pre-reads) to `Immediate`.
  2. Add `generation: u64` and `ticket: u64` to `iptv-guide`'s `PageRequest` and verify both in `Pages::insert` (supplied by `iptv-runtime`, keeping `iptv-guide` portable).
  3. In `iptv-runtime/src/data/pages.rs`, propagate DB read errors as typed `complete_failure(generation, ticket, error)` instead of caching `GuideStore::default()`.
- **Tests:** Extend `tests/it/concurrency.rs` (EPG replacement between binding read and programme lookup; `Immediate` overlay/organise contention) and `iptv-guide` / `iptv-runtime` / `iptv-app` guide-page tests (stale schedule reply, error-then-retry).

### Package 3: Player Worker Completion, Wakeup & Teardown Investigation (**P1** / **P2**)
- **Scope:** `iptv-player-mpv-001`, `iptv-player-mpv-002`, `iptv-player-mpv-003` (investigate first), `iptv-app-005`.
- **Dependencies:** Independent of Packages 1–2; `iptv-player-mpv-003` gated on native `libmpv` lifetime proof.
- **Changes:**
  1. Add `Note::LoadRejected(&'static str)` in `iptv-player-mpv` for pre-load guard refusals, hook-substitution failures, and guard retirement; decrement `st.loads` and emit `fail` only for the newest load.
  2. Set `mpv_wait_event` timeout to `0.0` when commands already produced response notes (`sent == true`) before calling `notify`.
  3. In `iptv-app`, route `on_demand::active` and `player::resume::playing` through `stop_playback`, clear `ui.player.early` in `player::stop`, and call `stop_playback(ui)` when the sleep timer fires (preserving persistent surface 0).
  4. **Gate for `iptv-player-mpv-003`:** Before changing `Inner::drop`, verify `libmpv` thread-safety of `mpv_render_context_free` concurrent with an in-flight `mpv_command`, prove callback/`mpv_handle` lifetimes across UI and worker threads, and bound reaper threads.
- **Tests:** Extend `iptv-player-mpv` worker/native unit tests (early guard refusal, quiet-handle command wakeup) and `iptv-app` playback/sleep scenarios (VOD/recorded/early-start stop & sleep).

### Package 4: UI Async Ownership, Artwork, Details & Search Identity (**P1**)
- **Scope:** `iptv-app-001`, `iptv-app-002`, `iptv-app-003`, `iptv-app-004`, `iptv-search-001`.
- **Changes:**
  1. `content_image.rs`: send `wanted` on queued cache hits.
  2. `guide_assign_view.rs`: check `REQUEST` token before `close(ui)` in `assign::choose` callback.
  3. `guide_repair_view.rs`: check `control.cancel.is_cancelled()` (or shell-owned monotonic instance ticket) across all repair callbacks; release stale preview plans before dropping.
  4. `menu_view.rs`: share `header.rs`'s `EpgSchedule`/`CombinedSchedule` description lookup with `ui.guide_data()` invalidation.
  5. `iptv-search` + `iptv-app/src/search_view.rs`: replace numeric channel positions in `ResultId` / hits with opaque `Schedule::channel_key` while keeping `usize` only as a same-generation lookup index.
- **Tests:** Extend `iptv-app` unit/scenario suites and `iptv-search` unit tests (concurrent artwork cache hit, busy-assign Back/reopen, repair screen incarnation race, programme details descriptions, catalogue reorder while search is open).

### Package 5: Ingestion, Stalker Query Repair & Recording Transport (**P1** / **P2**)
- **Scope:** `iptv-providers-003` (**P1**), `iptv-import-001` (**P1**), `iptv-net-002` (**P1**), `iptv-import-002` (**P2**), `iptv-net-001` (**P2**).
- **Changes:**
  1. `iptv-providers/src/stalker/parse.rs`: repair query-only replies **preserving raw query encoding** (no `Url::query_pairs_mut()` re-encoding; preserve `#fragment`, append with `&` if `?` already present).
  2. `iptv-import/src/cancel.rs`: give `Cancelled` per-waiter registration IDs and unregister in `Drop`, preserving local futures/`wasm32` compatibility.
  3. `iptv-import/src/stalker.rs`: enforce `MAX_STORED_WARNINGS` during page merge and track `dropped`.
  4. `iptv-net/src/recording_file.rs` & `recording.rs`: apply 15s idle write timeout per header/chunk write, and open with `O_NOFOLLOW | O_NONBLOCK` + post-open `meta.is_file()` check (and Windows private-file policy).
- **Tests:** Extend existing `iptv-providers` Stalker tests (raw percent-encoded/base64 tokens, existing query, fragment), `iptv-import` unit tests (33+ waiters, dropped waiters, multi-page warning cap), and `iptv-net` recording transport harness (4 stalled readers + 5th range request, symlink/FIFO swap).

### Package 6: Cross-Crate `Debug` Redaction & Untrusted-Input / API Hardening (**P2** / **P3**)
- **Scope:**
  - **P2 Redaction:** `iptv-core-001`, `iptv-guide-001`, `iptv-library-001`, `iptv-providers-002` (redact `HttpRequest` headers by default and `StalkerPortal` `Debug`, without breaking portals that legitimately have URL userinfo).
  - **P2 Untrusted-input & Snapshot Hardening:** `iptv-epg-001`, `iptv-epg-002`, `iptv-providers-001`, `iptv-library-002`, `iptv-runtime-005`, `iptv-runtime-007`.
  - **P3 Exotic-input & API Hygiene:** `iptv-epg-003`, `iptv-core-002`, `iptv-core-003`, `iptv-search-002`, `iptv-settings-001`, `iptv-settings-002`, `iptv-store-sqlite-002`.
- **Tests:** Add redaction marker tests, boundary-input unit tests, and compile-fail/visibility updates in each owning crate's existing test modules.

### Final Validation Gates (Implementation Phase)
1. `cargo test -p iptv-core` and `cargo build --locked -p iptv-core --target wasm32-unknown-unknown` (once `wasm32` target stdlib is available).
2. `cargo clippy --locked --workspace --all-targets --features iptv-runtime/test-support,iptv-app/test-support -- -D warnings`.
3. `cargo test --locked --workspace --features iptv-runtime/test-support,iptv-app/test-support --tests` and `--doc`.
4. `cargo build --locked -p iptv-app`, plus NSH pin reconciliation (`nsh.rev` vs `~/src/nsh`) so `tools/check.sh` runs end-to-end, and Android (`tab36`/`tv36`) / isolated timing gates for player & SQLite concurrency changes.
