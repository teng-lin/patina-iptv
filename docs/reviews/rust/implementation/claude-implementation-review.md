# Claude implementation source review

Claude CLI with the supported Sonnet alias produced the report below in read-only
plan mode using Read/Grep/Glob; no writes or tests. The default Fable attempt
ended at its usage limit and is not counted as a review. Sonnet completed with
exit 0 in 253.46 seconds. The report explicitly limits its detailed scope.

## Dispositions

1. Accepted. Durable recording acceptance is the retained authority for already
   accepted jobs/recurrences. Remove the two obsolete capture-time legacy overlay
   checks while retaining source enabled/incarnation/config/member fences. New
   real-media capture regressions cover granted overlay-blocked jobs and future
   occurrences materialized after block/relock. Final central checks pending.
2. Accepted. Normalize parent sets and compare empty targeted-cache entries as
   semantically absent in full refreshes; populate one coherent SQL snapshot.
   The new actual-store empty-parent refresh regression passed with all ten
   parental tests. Known block/ancestry changes still invalidate authority.
3. Verified release history. All eight application release/preview tags have
   canonical migration names. Across 23 source-history commits, version 14
   briefly named recording_rules before the unreleased VOD integration moved it
   to version 15. No released schema uses that intermediate name; the base build
   already cannot migrate that intermediate schema because the rule tables exist.
   Exact-name validation is retained, with historical release fixture checks.
4. No correction: schema v6 enforces UNIQUE(source_id, provider_index), and
   source_id/feed_id is its primary key. The proposed duplicate legacy states
   require corrupt/bypassed schema. Bounded retained-receipt validation deliberately
   fails closed rather than reassigning identities.
5. Intentional: Stop must also cancel early startup/resume and pending preparation
   when no row is currently playing; pending Stop regressions require this path.
6. Deferred performance suggestion, no measured regression: identities here are
   per channel, not per programme. Remapping current filters before ranking/limit
   is required for correctness; future caching needs actual profiling and complete
   invalidation. Existing shared latency gates remain required.
7. Intentional documented 15-second progress-idle policy, not a total-body limit.
   Healthy long playback is preserved; direct stalled-slot and actual native
   pause/range-resume regressions cover the intended contract.

## Unmodified review output

I found no correctness or security defect in the main cancellation, SQL-snapshot and player-lifetime changes. There is one medium regression in recordings and a few low-severity or unverified risks. I read the diff and the surrounding source with Read and Grep only. I ran no tests, edited nothing, and did not write the plan file.

## Findings, by priority

### 1. Medium: recording rules bypass the channel-block check, and the scheduling path accepts jobs the recorder then refuses
- **Locations:**
  - `crates/iptv-store-sqlite/src/recordings.rs:560`: `materialize_recording_rule_at` now calls `insert_job(.., true)`.
  - `crates/iptv-store-sqlite/src/recordings.rs:67`: `require_channel` skips the `o.blocked` test when `authorized` is true.
  - `crates/iptv-runtime/src/recordings/rules.rs:271`: the background materializer has no runtime or parental context.
  - `crates/iptv-runtime/src/recordings.rs:1319`: capture start still does `.filter(|c| !c.blocked)`.
- **Scenario A:** a channel is overlay-blocked and the user unlocks it. `Admission::authorize` passes, `require_channel(.., true)` skips the block test, and the job is accepted. At start time `:1319` refuses it with "Channel unavailable or blocked". The user has a scheduled job that is certain to fail. Before this change it was rejected at scheduling.
- **Scenario B:** a repeat rule was accepted, and the channel is later blocked and locked. `materialize()` runs with no grant and passes `authorized = true`. It creates new jobs for the blocked channel on every pass. Before this change the SQL `o.blocked = 0` condition stopped that. The capture check at `:1319` probably prevents any media being recorded (unverified). You still get doomed jobs and failure noise.
- **Rule violated:** authority must be re-validated at the point it is consumed. The "accepted recurrence keeps it" comment contradicts the capture-time check.
- **Smallest fix:** make `materialize_recording_rule_at` pass `authorized = false`, which keeps the old `o.blocked = 0` condition. Separately, either have capture use the same `native_allowed` rule or stop checked scheduling from skipping the block test. Pick one rule for both.
- **Preexisting:** group-level `parental_blocks` were never enforced in `materialize`. That is not new.

### 2. Low to medium: spurious `policy_revision` bumps cause false denials
- **Locations:** `crates/iptv-runtime/src/parental.rs:203` and `:267`.
- **Cause:** `parental_refresh_targets` stores `ancestry[target] = parents` even when `parents` is empty. `parental_memberships()` (`store parental.rs:186`) returns only children that have at least one parent. The next full `parental_refresh` therefore sees `state.ancestry != ancestry` and bumps `policy_revision`. The `state.complete &&` guard is always true there, because `complete` was set a few lines earlier. The same applies to `blocked` if `parental_blocks()` omits overlay or adult-derived blocks. I did not confirm that.
- **Impact:** a request that overlaps a full refresh (boot, PIN change, `parental_set_blocked`, catalogue change) gets `PermissionDenied` from inspection or "Recording access changed" from admission. It fails closed and retrying works. It is still wrong by the design rule that the revision should bump only on a real policy change.
- **Smallest fix:** compare ancestry with empty-parent entries filtered out, for example `retain(|_, p| !p.is_empty())` on both sides.

### 3. Unverified: the migration name check could lock existing databases out
- **Location:** `crates/iptv-store-sqlite/src/migrate.rs`, `current_version`. It now returns `SchemaCorrupt` if any recorded `(version, name)` is not in `MIGRATIONS`, or if `min != 1`.
- **Risk:** if any shipped migration was ever renamed, or any migration was ever recorded with a different name, existing user databases fail to open. This is a hard failure with no recovery path.
- **Action:** check `git log -p -- crates/iptv-store-sqlite/src/migrate.rs` for renamed entries. If there are none, ignore this.

### 4. Low: reads and writes of guide sources can now fail on legacy data
- **Locations:** `crates/iptv-store-sqlite/src/guide_sources.rs`, `guide_sources_in` and `set_guide_sources_in`.
- **Read side:** `guide_sources_in` now calls `validate_sources` on every read. A legacy database with a duplicate `provider_index` would make every read fail, including probes and inspection. The old `sync_provider` tolerated duplicates. Whether the schema permits duplicates is unverified.
- **Write side:** every write now calls `feed_high_water_in`. That can seed from legacy repair receipts and fails closed above the documented limits (4096 receipts, 32 MiB). Ordinary edits and catalogue refreshes on such a source then fail.
- **Fix if real:** validate only on write, or repair duplicates once.

### 5. Low: stop now always locks parental access and disables early resume
- **Location:** `crates/iptv-app/src/player_keys.rs:164`. The "nothing playing" early return was removed.
- **Effect:** Stop or long-Back with nothing playing now calls `player::stop`. That runs `parental_content_closed()`, which locks grants under the EveryTime window, and sets `resume = false`. The change does fix VOD not stopping. The side effects are broader than needed.
- **Unverified:** whether `Action::Stop` is reachable when no player is open.
- **Fix if wanted:** guard on `ui.player.vod`, `recorded`, `early` and `playing` instead of removing the guard.

### 6. Low, performance: `search_programmes` resolves every index row on each query
- **Location:** `crates/iptv-runtime/src/search.rs`, the `resolved` vector in `search_programmes`.
- **Cost:** each query calls `resolve_search_channel` for every row, and that allocates a `ChannelId` string. For roughly 100k rows per keystroke this may be noticeable.
- **Fix:** cache the resolved vector per schedule and index generation.

### 7. Informational: the relay write deadline resets on every positive write
- **Location:** `crates/iptv-net/src/recording_file.rs`, `ProgressWriter::send`.
- **Effect:** a peer that reads one byte per under 15 seconds holds one of the four admission slots indefinitely. The relay is loopback-only with a secret route, so exposure is small.

## Checked, no defect found
- **Parental admission and inspection:**
  - The `policy_revision` and `epoch` fences in `parental.rs` are sound. `native_allowed` correctly separates grant revocation from unprotected zapping.
  - The `inspection_live_filter` SQL matches `parental_target_blocked` semantics, and the bind-parameter indices are consistent in all three call sites.
  - Inspection and `read_snapshot` nest correctly because `begin_read_snapshot` checks `is_autocommit`.
- **Recordings writer:** the checked transactions take `Immediate` and run `authorize` inside the writer transaction, so a rollback on early return is correct. Source incarnation is captured before the awaits.
- **VOD playback:** the final `parental_allowed` and `services.is_current()` fence runs after the last await.
- **Guide pages:** the waiter completion is keyed by `PageRequest{owner, attempt}`. Stale and evicted replies cannot consume a newer waiter, and a failed read is not cached as an empty page. The page source-incarnation check runs inside one snapshot.
- **Stable search identity:** `ChannelId` includes the source incarnation, and reads fail on a recreated source.
- **mpv worker:**
  - `LoadId` is compared by pointer, and `Loading`, `LoadRejected` and `GuardFailed` each complete once for the current load only.
  - `timeout = 0.0` after a send cannot spin, because it breaks on `MPV_EVENT_NONE`.
- **`CancelToken`:** per-waiter slot registration with unregister on drop is correct. A cancel racing registration is covered by the flag-then-lock ordering.
- **Private recording files:** `open_private_regular` opens first and validates the descriptor. The `libc` and `getrandom` dependencies exist in `iptv-net`.
- **Provider guide limit:** the 999-guide limit is truncated at import, so it does not fail whole imports.

## Scope limits
I did not review these in depth:
- `iptv-epg` (settle, time, xmltv)
- `iptv-settings` (typed keys, validate, backup)
- `iptv-providers`
- `iptv-import` stalker
- `iptv-library` playlists and `guide_inspect`
- the app view files other than `player*`
- all test diffs

Nothing was compiled or run, and I did not check iptv-nav, which you excluded.
