# Patina IPTV Rust review: independent adversarial challenge

**Baseline:** `0e197073` (tree clean except the untracked `docs/plans/` and `docs/reviews/`). This was a read-only review. I modified nothing, ran no builds or tests, spawned no agents and wrote no files. Every disposition below comes from reading the source.

## 0. Headline

- **Most findings hold, but several priorities are wrong in both directions.**
  - **Raise:** `iptv-runtime-001` is reachable from the shipped app's own guide inspection and repair screens, not only from an independent native consumer.
  - **Raise:** `iptv-player-001` leaks the load counter for the rest of that player handle's life.
  - **Raise:** `iptv-search-001` can **play the wrong channel**, not just keep focus on the wrong row.
  - **Lower:** the five Debug-redaction findings. None is formatted by any `log::` call; they are latent diagnostics exposure.
  - **Lower:** `iptv-core-002`, `iptv-core-003` and `iptv-settings-001`. They are reachable only with public-API values that no in-repo caller passes.
- **One rejection.** `iptv-store-sqlite-003` (BUSY_SNAPSHOT) cannot happen in production. Each process has exactly one writer connection, the reader and analysis connections are `query_only` (`iptv-store-sqlite/src/lib.rs:218`), and a process-wide storage lock excludes other processes (`recordings/host.rs:3`).
- **One proposed patch would introduce a new defect.** Using `parental::native_allowed` as the admission fence for inspection or recordings would reject even unprotected content whenever the parental epoch changes. The epoch changes on every channel change: `app/src/lib.rs:354` calls `parental_content_left`, which increments it (`parental.rs:403`), and `native_allowed` returns `false` on any epoch change before checking protection (`parental.rs:543`).

## 1. Baseline limitations versus actual defects

| Observation | Classification |
|---|---|
| `tools/check.sh` stops at the NSH pin (`3276965` checked out vs `9ed782c` pinned, identical trees) | Environment/config gate. Not a crate defect. Fix the pin or check out the pinned commit before any batch closes. |
| `wasm32-unknown-unknown` standard library absent; core wasm build cannot run | Host-toolchain limitation. The portability gate is still **unverified** for every portable fix. |
| Player library tests: 50 passed, 10 failed (ready/ended/worker timeouts) against `/usr/lib/libmpv.so.2` | A stock libmpv lacks the `patina-relay-guard-v1` protocol, so `worker.rs:349-357` refuses every HTTP load. These failures are evidence of the environment, not decoder defects. However, because of `iptv-player-001`, that refusal shows up as an endless *buffering timeout* instead of an explicit error. After the 001 fix, the same tests on this host should fail fast with `ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED`; use that as a validation signal. |
| App scenarios: 173 passed, 4 ignored; non-player tests: 1483 passed | Supporting evidence only. None of these covers the races below. |
| Release profile has no `overflow-checks` (`Cargo.toml:77-86`) | Arithmetic findings **panic in debug and silently wrap in release**. Priorities below are based on the release behaviour. |

## 2. Disposition of every finding ID

Priority tiers:
- **P1:** a reachable security or contract breach, or wrong data/channel shown in normal use.
- **P2:** reachable correctness problem with limited blast radius.
- **P3:** malformed provider input, or diagnostics/robustness hardening.
- **P4:** public-API values no caller passes, or defensive hygiene.

| ID | Disposition | Priority | Source-backed reason |
|---|---|---|---|
| core-001 | Accept with correction | P3 | `PlaySource` (`playback.rs:25`) and `WizardEvent` (`wizard.rs:539`) derive Debug. No `log::` call formats them. The recording `PlaySource` also carries the loopback capability URL, which `security.md:95` says must never be logged. This is latent exposure, not an observed leak. Keep header names in the output and redact only the values. |
| core-002 | Accept with correction | P4 (was Must) | No in-repo caller mutates `Membership.rows` or `.query`. The only user is `custom_groups.rs`, which goes through `new`/`reload`/`filter`. This is public-API hardening. |
| core-003 | Accept with correction | P4 (was Must) | Callers pass ±1 only (`input.rs:350,353`). With step 0, the loop only spins when the current row is not focusable. Take the trivial guard and check the sibling `move_focus` loops for the same shape. |
| settings-001 | Accept with correction | P4 | Only `Screen::step(i64::MAX)` or custom grids trigger it. The backup restore path is safe because `clamp` runs before the subtraction (`validate.rs:85-86`) and `check_grid` returns before the modulo (`validate.rs:68`). Use `saturating_mul`. Don't add a fallible API without a caller that needs it. |
| settings-002 | Accept with correction | P3 | The rationale is stronger than reported. About 16 inline `BoolKey("…")`/`EnumKey("…")` constructions in `iptv-runtime` (e.g. `parental.rs:83-95`, `imports.rs`) and `iptv-app` are **not** covered by the constants-vs-schema test. The proposed `pub(crate)` change does not compile without first adding constants for all of them. |
| providers-001 | Accept with correction | P3 | This is malformed provider input, and the release behaviour is **worse** than stated. With `epoch≈i64::MAX`, `round() as i32` saturates to `i32::MIN`, `*15` wraps to `i32::MIN`, and `abs()` in release returns `i32::MIN`, so the `≤ 840` check passes and the account gets `derived_offset_minutes = Some(i32::MIN)` (catch-up times shifted by about 68 years). `parse_fixed_offset` only panics in debug; in release the wrapped value is rejected by `h <= 14`. Fix: check the range on the `f64`/`i128` value **before** narrowing. |
| providers-002 | Accept with correction | P3 | The Debug gap is real (`http.rs:151-167` only masks an allowlist). **Reject the proposed behaviour change** of refusing userinfo in Stalker endpoints; use a redacting Debug instead. Redact all header values except a small allowlist; `Referer` is *not* safe to show. |
| providers-003 | Accept | P3 | `parse.rs:640-646` does plain string concatenation. Decide the duplicate-key rule (reply wins) and keep the portal-relative branch. |
| epg-001 | Accept | P3 | Malformed feed. `xmltv.rs:63` `n + 1` wraps to 0 in release. Use `checked_add`. |
| epg-002 | Accept | P3 | `settled_source.rs:92` adds two u32s that are each < 2³², so the sum can overflow. In release this gives a wrong overlap check, not a panic. Widen before adding. |
| epg-003 | Investigate before implementation | P4 | `time.rs:53` clamps seconds before validating. Rejecting seconds 61–99 would drop programmes from sloppy feeds that display correctly today. Decide strict versus tolerant handling first; possibly reject only in the strict parser. |
| library-001 | Accept with correction | P3 | Latent Debug exposure. Merge into the redaction package. |
| library-002 | Investigate before implementation | P3 | Confirmed: provider feed IDs `2+n` (`guides.rs:74`) collide with user IDs ≥1001 once a provider lists ≥999 EPG URLs. Investigate three things before choosing a policy. (a) The report's "reject the whole update" would fail an otherwise valid playlist because of its header; capping at ingestion with an import warning is probably better. (b) `iptv-import/src/epg.rs:111` (`from_m3u_header`) builds feeds from the same unbounded list and would fetch 1000+ URLs. (c) The guide-repair allocator in `store/guide_repair.rs` must not overlap the provider range. |
| guide-001 | Merge into redaction package | P3 | Same latent-exposure class as core-001. |
| guide-002 | Merge with runtime-003 | P2 (was Must) | Confirmed (`pages.rs:155-182`). Page stores are keyed by `ChannelKey` (`epg.rs:635-646`), so a stale reply brings **old programmes for the right channel**, not cross-channel data. The fix needs no runtime-supplied generation: `Pages` can create its own instance ID (a static atomic counter is wasm-safe) plus a per-entry ticket. |
| search-001 | Accept with correction | **P1/P2** | Worse than reported. Result rows render names from the *current* schedule using stale indices (`search_view/view.rs:299-321`), and `SearchEffect::Play(Channel(i))` → `Outcome::Live(i)` (`search_view.rs:128,459`) plays whatever channel index `i` now is. Parental checks still apply to the channel actually played, so this is wrong-channel playback, not a bypass. Store the key per channel in the programme index (not per hit) to keep memory bounded. |
| search-002 | Accept with correction | P4 | Exotic values only (`now == i64::MIN`, spans of ≥136 years). Use saturating windows and skip unrepresentable rows rather than adding a new `BuildError` API. |
| import-001 | Accept with correction | P2 | The contract violation is real (`cancel.rs:72-78`), but impact is overstated. `Services.closed` is one long-lived token shared by every inspection call (`store_service.rs:300`, `inspection.rs:49`) and by recording tasks. Completed futures never unregister, so after 32 operations evicting live waiters is routine. However, every current caller also races a deadline, timer or data future (`inspection.rs:62`, `write.rs:67`, `recordings.rs:1048-1051`). The result is late cancellation/owner-close observation (up to 3–60 s) plus stale waker buildup, not a permanent hang. |
| import-002 | Accept | P3 | `stalker.rs:395-401` and `449-454` accumulate warnings without a cap. Malformed provider input. |
| net-001 | Accept with correction | P4 | The check→open race is real (`recording_file.rs:76-80`), but exploiting it requires write access to the private media directory. Reuse `file.rs::open_regular` (`O_NONBLOCK` plus a check on the opened file) and add `O_NOFOLLOW`. For the resume writer (`recording.rs:126-149`), add `O_NONBLOCK` and an `fstat` check. |
| net-002 | Accept with correction | P3 | Confirmed: there is no write deadline, and `tasks.len() >= 4` silently drops new connections (`recording_file.rs:32`). Triggering it needs 4 stalled holders of the random capability, and mpv normally holds one. **A 15 s write deadline will close the connection of a recording paused for more than 15 s.** Either verify that mpv/ffmpeg reconnects with a range request on resume (the existing playback relay already does this, `playback.rs:817`), or evict the oldest stalled connection when the slots are full. |
| sqlcipher-init | No IDs; agree | — | Nothing to dispose. |
| store-001 | Accept | **P1** | Confirmed. The reader lane runs no transaction (`store_service.rs:561-573`), `load_epg` reads bindings, feeds and programmes as separate statements (`epg.rs:592-647`), and `epg_channels.id` is a reusable rowid (`epg.rs:252,452`). `read_programmes` labels the builder with the old binding's `epg_channel` and then reads programmes by the reused numeric ID (`epg_window.rs:73-74`), so programmes from another channel are attached. Same pattern in `for_each_epg_title` and `epg_guide`. |
| store-002 | Accept | P4 | Only reachable with a corrupt database. The fix is a trivial predicate change. |
| store-003 | **Reject** (as a defect) | — | There is a single writer connection, readers are `query_only`, and a process lock excludes others (see headline). `move_group_to`'s read outside the transaction is also serialized on the writer lane. Keep it only as optional consistency hygiene, and reopen it if a second writer is ever introduced. |
| player-001 | Accept | **P1** | Worse than reported. `State.loads` only resets when the control thread dies (`mod.rs:854`). After one refused load, every later `Loading` decrement leaves `loads ≥ 1` (`mod.rs:868-871`), so `expected` is never set and that player handle shows every later stream as buffering forever. Refusals come from `!bridge.guarded`, a missing guard protocol, `native_guard` failure, or a failed hook substitution (which disables the guard permanently, `worker.rs:267`). |
| player-002 | Accept with correction | P2 (was Must) | Confirmed: the wakeup is consumed and the next wait is `-1` (`worker.rs:241-245`). Replies are **delayed, not lost**: the next `send` wakes the wait, which flushes the previous batch. `Position`/`Timeline` ticks are therefore up to about 1 s late; one-shot replies (a refusal note, `Format`) on a quiet handle wait for the next command. The one-line fix (`timeout = if sent { 0.0 } else { -1.0 }`) is correct. |
| player-003 | Investigate before implementation | P3 | `Worker::drop` joins the thread without a deadline (`worker.rs:209-216`); confirmed. But `execute` does no blocking network I/O (relay start only binds loopback, `playback.rs:220-235`), and there is no evidence of a render/control deadlock. Surface 0 is persistent, so this affects multiview tile removal and shutdown only. A native reproduction is required before any lifecycle redesign. |
| runtime-001 | Accept with correction | **P1** | Confirmed against the v1 contract ("cannot be reached by bypassing search with a known ID", `runtime-api-v1.md:47-48`). The store only checks the legacy `channel_overlays.blocked` (`store/inspection.rs:281,344,401`). The **app reaches it too**: `guide_inspect_view.rs:124,153,163` and `guide_repair_view.rs:123`, so a locked user can browse blocked channel names and programme titles. It is also **over-restrictive**: legacy-blocked channels are denied even when `parental.live_enabled=false` or after a PIN grant, which the runtime policy would allow (`parental.rs:268,280`). |
| runtime-002 | Accept with correction | P1/P2 | Confirmed: `schedule_recording` checks parental access, then awaits a ready-wait of up to 2 s, a duplicate read, `refresh_accounts` and `retire_unused`, then commits (`recordings.rs:526-594`). `update_recording` has the same shape. The final gate must run inside the writer closure (see §4 for the epoch trap). Accepted jobs and the background materialization of recurring rules must remain ungated by later locks. |
| runtime-003 | Merge with guide-002 | P2 | `arrived` looks up the schedule by source only (`data/pages.rs:85-92`). |
| runtime-004 | Accept with correction | P2 | Confirmed (`data/pages.rs:68-72`). The proposal leaves out a backoff: `Pages::with` re-requests a missing page from render-time queries (`pages.rs:120-133`), so "remove the pending entry on failure" would create a hot retry loop on the reader lane while the store keeps failing. The catch-up waiter (`ready`) also needs an explicit failure result. |
| runtime-005 | Accept | P3 | The analysis connection is `query_only` and races the writer. `expect("source checked")` (`probe.rs:632`) panics; `run_job` catches it and discards the connection (`store_service.rs:664-667`). Use `guide_repair_snapshot` (`guide_repair/verification.rs:19`). |
| runtime-006 | Accept with correction | P3 | The API is experimental (`runtime-api-v1.md:208-209`) and the shell already mitigates it. `load_authorized` and `publish_source` re-check `source_allowed` synchronously (`player.rs:699,811`); rejection → `stop_surface` → `stop` → `close_recording_playback` (`player.rs:291`); VOD revalidates through `validate_vod_publication`. What remains: a direct caller of `open_recording` leaves the relay in `recordings.playback` until it closes or opens another. |
| runtime-007 | Accept | P3 | `validate_candidate` creates a fresh token and a 60 s deadline (`probe.rs:511-516`), called from `probe.rs:1092`. `current_configuration` also uses an unbounded `block_on` (`probe.rs:503`). |
| app-001 | Accept | P3 | `content_image.rs:103-104` returns a cache hit without sending it on `wanted`. |
| app-002 | Accept | P3 | Back is accepted while busy (`library/guide_assign.rs:246-247`); the save callback closes the picker unconditionally (`guide_assign_view.rs:159-162`). |
| app-003 | Accept with correction | P2 | Confirmed (`Screen::new` sets `generation: 0`, `guide_repair_screen.rs:57`). `apply` is affected as well as `status`: a stale apply outcome in another source's screen would offer **Undo** of the old commit. Prefer one shell-owned monotonic instance ticket checked by every callback; status currently discards `control` (`_`). |
| app-004 | Accept | P3 | A hard-coded placeholder description ships in production (`menu_view.rs:486`). |
| app-005 | Accept | P2 | `stop_playback` ignores VOD and early resume (`player_keys.rs:167-171`). The sleep timer only clears `playing` (`chooser.rs:286`), so VOD and recordings keep a source (`video.rs:171-175`), the provider reservation stays open and progress is never flushed. Realistic on a TV left playing overnight. |

## 3. Corrected priority order

- **P1:** runtime-001, store-001, player-001, search-001 (wrong-channel Play), runtime-002.
- **P2:** guide-002 + runtime-003, runtime-004, import-001, player-002, app-003, app-005.
- **P3, malformed provider input:** providers-001, providers-003, epg-001, epg-002, import-002, library-002.
- **P3, robustness:** net-002, runtime-005, runtime-006, runtime-007, player-003 (pending investigation), app-001, app-002, app-004, settings-002.
- **P3, diagnostic formatting only (no observed logs):** core-001, guide-001, library-001, providers-002.
- **P4, exotic public values or hygiene:** core-002, core-003, settings-001, search-002, store-002, net-001, epg-003.

## 4. Special-scrutiny areas

**Runtime inspection: parental filtering, unlocks and pagination (runtime-001)**
- **Filter before LIMIT.** `inspection_channels_inner` already filters in SQL before `LIMIT` (`inspection.rs:342-345`), and `inspection_coverage` reuses it (`:667`). One shared store predicate therefore covers channels, the audit and (through `inspection_channel`) candidates and the channel-filtered programmes path.
- **Match the runtime model.** The predicate must cover:
  - the `parental_blocks` table;
  - legacy overlay blocks;
  - adult groups without an explicit unblock;
  - category and custom-group ancestry (`parental.rs:268-298`).
- **Policy:** skip filtering when `policy.protects(Live)` is false.
- **Grants:** pass the small set of granted targets from a snapshot captured on the owner thread into SQL (a VALUES list or temp table). Do not filter each page in Rust afterwards; that underfills pages and breaks cursor semantics.
- **Cursors:** bind the parental epoch (or a digest of the effective block set) into the cursor context. Lock or unlock mid-pagination then returns `StaleCursor` instead of quietly changing which items exist.
- **Final fence before publication:** re-check revocation only for rows that were actually allowed through a grant. **Do not** copy `native_allowed`'s rule of "any epoch change means denied" (`parental.rs:543`): zapping bumps the epoch (`app/lib.rs:354` → `parental.rs:403`), so concurrent playback would make inspection fail at random.

**Durable recording admission versus already-accepted background jobs (runtime-002)**
- Capture the snapshot **after** `parental_check` succeeds.
- Gate inside the same `services.call` closure that runs `store.schedule_recording` / `update_recording`. Check protection first, and treat an epoch change as revocation only when the target is actually blocked.
- Do not apply the gate to the recurring-rule materializer, recovery, or jobs already accepted. `stop_recording` must stay ungated.
- The duplicate check runs on the reader before the writer commit. Two concurrent identical requests can both pass it; move it into the writer transaction as well.

**Shell mitigations for direct runtime playback APIs (runtime-006)**
- The shell gate is genuine, in `player.rs:699` and `:811`. App test `recordings_transport.rs:493` covers revocation during preparation.
- Runtime fix: after the last await, call `parental_allowed` for the item and its parent (as `vod.rs:558-562` already does). In `open_recording`, drop the relay if that check fails.
- Keep and independently test the shell gate.

**Stale guide and search identities (guide-002/runtime-003, search-001)**
- Guide:
  - The page carries an identity minted inside `Pages`.
  - `insert` rejects a mismatched instance or ticket; `evict` invalidates the tickets of pages still loading.
  - Bump the ticket when a page is evicted and re-requested.
- Search:
  - Hits carry an opaque, source-qualified key.
  - `Play` resolves the key against the current schedule and falls back gracefully if the key disappeared.
  - Rows that don't match the current index generation are hidden or disabled until `search_again` finishes.

**SQLite read snapshot consistency (store-001)**
- Use one shared helper (in place of `vod_read_snapshot` and `guide_repair_snapshot`) that begins a deferred transaction **only if** `is_autocommit()`. A plain `unchecked_transaction` fails with "cannot start a transaction within a transaction" when called from those snapshot helpers.
- Apply it to `load_epg`, `for_each_epg_title`, `epg_guide`, `inspection_channel` (which currently runs `scope` and `channel` as separate statements) and probe `configuration`.
- Keep the transaction deferred so WAL readers still run in parallel with the writer.

**Player notification wakeups and teardown (player-001/002/003)**
- **001 (refused load):** add a `LoadRejected(code)` note that decrements `loads` exactly once and fails only the newest load. Every refusal path in `execute`, and the hook substitution failure, must produce exactly one.
- **002 (wakeup):** the `timeout = 0.0` fix is safe. libmpv keeps a pending wakeup for the next wait, so no wakeup is lost.
- **003 (teardown join):** any redesign must keep this order:
  1. free the render context on the GL thread;
  2. the worker exits;
  3. the last `Arc<Mpv>` is dropped (libmpv terminate);
  4. callbacks are retired only after that.

  Avoid creating a reaper thread on every multiview change. Surface 0 must never be detached.

**Cancellation waiter eviction (import-001)**
- Use per-future registration IDs, replace the waker on each re-poll, unregister on drop, and keep no bound that evicts other live waiters. No new dependency, no Tokio, no `Send` bound on the portable crate.
- The regression test must include long-lived shared tokens (`Services.closed`, the recordings `shutdown` token) with more than 32 completed waiters, followed by one live waiter that must still be woken.

**Deadlines for retained media (net-002, runtime-006)**
- Include a "paused for more than 15 s, then resume" case before adopting a write deadline.
- Relays retained by direct callers need explicit closing; see runtime-006.

## 5. Unsafe or incomplete proposed patches and missing interactions

1. **`native_allowed` as an admission fence** (runtime-001/002): its epoch semantics produce false denials whenever the user zaps (§4).
2. **Debug redaction:**
   - Rejecting Stalker userinfo (providers-002) is a behaviour change with no caller justification.
   - The `HttpRequest` header allowlist must not expose `Referer`.
   - Containing types that derive Debug (Resolution, screen state, `SourceProps`) must be included in the tests.
3. **settings-002** as written does not compile: inline key construction in runtime and app must be replaced by constants first.
4. **runtime-004** needs a failed state with backoff; simply removing the pending entry causes a hot re-request loop.
5. **guide-002** does not need a runtime generation parameter; requiring one couples runtime and portable crate APIs unnecessarily.
6. **library-002:** "reject the update atomically" punishes valid playlists. It must also be coordinated with `from_m3u_header` and the repair feed allocator.
7. **net-002:** a fixed write deadline needs pause/resume evidence (§4).
8. **epg-003:** strict rejection may drop programmes from tolerant feeds.
9. **search-001:** the proposal covers focus but needs an explicit fence for Play against a stale index.
10. **player-003:** the sketch reorders destructors without native evidence; implement only after a reproduction.
11. **app-003:** the generation-only check also guards `apply`, `preview` and `history`. A stale successful preview must still be released (`guide_repair_view.rs:226` already does this).

## 6. Fix packages, in dependency order

| # | Package (crates owned) | IDs | Key regression scenarios (synthetic data) | Gates |
|---|---|---|---|---|
| 0 | Baseline gates (CI configuration) | — | — | Resolve the NSH pin; install the wasm target; record the guarded libmpv in use. Nothing below can close until `tools/check.sh` runs end to end. |
| 1 | Cancellation primitive (`iptv-import`) | import-001 | 33+ wakers counted independently; dropped waiters; a waker that changes; cancel racing registration; shared-token churn | `cargo test -p iptv-import --tests`; wasm build; runtime `inspection` and `read_lane_contention` scenarios |
| 2 | EPG read snapshot (`iptv-store-sqlite`, then `iptv-runtime`) | store-001, runtime-005 | Barrier between binding read and programme read with `replace_epg` forcing ID reuse: result is all-old or all-new. Call from inside an existing snapshot. Source deleted during probe configuration gives `SourceMissing`, with no panic. | Store `tests/it/concurrency.rs`, real SQLCipher, latency/timing gates run alone |
| 3 | Guide paging identity (`iptv-guide`, `iptv-runtime`, `iptv-app` consumer) | guide-002, runtime-003, runtime-004 | Old reply arrives after a same-window refresh; evict then re-request; deleted and recreated source; fail-once-then-succeed with backoff; catch-up gets an explicit failure | guide + runtime tests, app `guide_pages`, wasm build for guide |
| 4 | Parental admission (`iptv-store-sqlite` predicate, `iptv-runtime`, app inspection views) | runtime-001, runtime-002, runtime-006 | Direct/legacy/adult/custom-group blocks; known-ID and paged reads; unlock; lock mid-pagination → `StaleCursor`; zapping during a query does **not** deny unprotected content; revoke while the writer is held → no job or rule committed; existing jobs continue after lock; direct `open_recording` revoked → no relay retained | runtime `parental_access`, `parental_recovery`, foundation inspection; app `parental`, `recordings_transport`; contract-test update per `runtime-api-v1.md:213-218` |
| 5 | Player completion and wakeup (`iptv-player-mpv`) | player-001, player-002, then investigate 003 | Each refusal produces exactly one error then idle, followed by a successful load; quiet handle receives a reply without a second command; teardown with a pending command under a child-process deadline | Worker unit tests; native lane on a host with the **guarded** libmpv; desktop multiview/PiP evidence; Android `tab36`/`tv36` if the change touches the shared surface lifecycle |
| 6 | Search identity (`iptv-search`, `iptv-app`) | search-001, search-002 | Reorder the catalogue while search is open; combined sources with duplicate provider IDs; Play during a stale window; key that disappeared | search tests + wasm; app `search_history` |
| 7 | App shell state (`iptv-app`) | app-005, app-003, app-002, app-001, app-004 | Sleep timer with live/VOD/recording/early resume → no source, reservation released, progress saved; Stop/Long Back on VOD; stale repair apply/status in a reopened screen; picker reopened while the writer is busy; artwork cache hit while permits are queued; real description shown in Details | App scenarios; `cargo build -p iptv-app`; Android device run for any player/back handling |
| 8 | Untrusted provider input (`iptv-providers`, `iptv-epg`, `iptv-import`, `iptv-library`) | providers-001, providers-003, epg-001, epg-002, epg-003 (after decision), import-002, library-002 (after decision) | Integer-boundary timestamps; `UTC+2147483647`; query-only Stalker replies; u32 boundaries for ordinals and slots; Stalker crawl peak warning count; header with ≥999 EPG URLs | Crate tests, wasm builds, fuzz lanes, debug and release (`overflow-checks` on/off) |
| 9 | Diagnostics redaction (core, guide, library, providers) | core-001, guide-001, library-001, providers-002 | Unique markers in URL path/query/userinfo and in headers; format containing types | Crate tests + wasm |
| 10 | API hardening (various) | core-002, core-003, settings-001, settings-002, store-002, net-001, net-002 | Per the reports, with the corrections in §2 and §5 | Crate tests; net recording harness with pause/resume |

Packages 1, 8 and 9 touch only portable crates and can run in parallel with 2–5. Package 4 depends on 2's snapshot helper for the store predicate. Package 6 is independent of 3.

## 7. Scope gaps

- **Not executed:** nothing was run, so every "confirmed" above means confirmed by source reading. I did not re-run the reviewers' `/tmp` probes.
- **Claims checked only at the cited lines:**
  - each leaf field behind library-001 and guide-001;
  - import-002's "about one million warnings" figure;
  - the epg-002 path through import (as opposed to stored rows);
  - Windows reparse handling for net-001.
- **Open interaction questions:**
  - whether mpv/ffmpeg reconnects after a relay write timeout on a paused recording;
  - libmpv render/core blocking behaviour (player-003);
  - whether NSH devtools or crash reporting Debug-print UI state (this would affect redaction priority);
  - the guide-repair feed allocator's ID range relative to provider feed IDs.
- **Not audited:** sibling `move_focus` loops; other compound reader-lane reads (`stored::guide_inputs`, library and organization snapshots); macOS, iOS, Windows and Android branches; JNI; `iptv-nav` (excluded); test bodies beyond the ones cited.
- **Not verified:** that the app's inspection screens are reachable without a settings PIN under default parental settings. The default for `parental.require_pin_for` decides whether runtime-001 is reachable from the app's own screens (P1) or mainly by an independent consumer.
