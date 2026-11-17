# Rust review: iptv-store-sqlite

This native library owns encrypted persistence, migrations, staged publication
and application queries. The most important defect is that several EPG reads
span independent SQLite snapshots while retaining reusable physical channel
IDs. A concurrent publication can therefore attach another channel's programmes
to an old binding. Workspace Clippy passed; the parent coordinates test evidence.
All fixes below are proposals. No implementation changes or commits were made.

## Baseline and validation

- Application revision: `0e197073a272eceb0631e4d53b08d2823950c416`.
- Edition 2024; declared Rust version 1.99.1; internal native library.
- Parent-coordinated workspace Clippy with `--all-targets`, runtime/app
  test-support features and `-D warnings`: passed.
- The full host wrapper is blocked by the existing NSH revision mismatch;
  non-player workspace tests passed with `--tests` and runtime test-support.
  The full workspace invocation exited 101 in the player crate (50 passed,
  10 failed there). App build and workspace doctests passed. The NSH revisions
  have identical tracked trees, but the wrapper checks commit identity.
  This reviewer did not start another Cargo run.
- Two temporary synthetic Python/SQLite probes confirmed the SQL interleaving
  and migration-bookkeeping predicates described below. They validate SQLite
  semantics and the actual SQL shapes, not a completed Rust regression test.
- Apple, Windows and Android execution, filesystem crash injection and physical
  device checks were not performed by this reviewer.

## Must fix

### iptv-store-sqlite-001 EPG reads can combine old bindings with replacement programmes

**Locations:** `crates/iptv-store-sqlite/src/epg.rs:590`, particularly the
binding cursor release at line 616 and programme reads at line 630;
`crates/iptv-store-sqlite/src/epg_titles.rs:27`;
`crates/iptv-store-sqlite/src/epg.rs:519`.

**Rule:** Keep linked invariants within one consistency boundary
(`rust-best-practices`, Concurrency and async); preserve content identity
and atomic publication (`AGENTS.md`, State lives in the core; store contract).

**Problem:** `load_epg` reads bindings, feed settings and programmes in
separate statements without a surrounding read transaction. The completed
binding cursor no longer pins the first snapshot. `replace_epg` deletes and
reinserts `epg_channels`, whose `INTEGER PRIMARY KEY` values can be reused.
After a writer commits between these statements, a cached numeric channel ID
can point to a different channel in the new guide. `for_each_epg_title` has the
same split between collecting bindings and visiting each numeric channel;
`epg_guide` can pair old guide metadata with replacement feed settings. Runtime
callers use the separate reader lane, so serialized writer commands do not
exclude this interleaving.

**Evidence:** A temporary WAL database used the same binding SELECT and
numeric programme lookup. After the binding cursor was exhausted, a second
connection deleted the guide and inserted a replacement row with ID 1. The
result was `cached identity 'a'/'old-a' now reads [('New B',)] via reused id 1`.
The existing concurrency tests check individual catalog statements, not this
multi-statement guide read.

**Proposed fix:** Pin one read snapshot for the entire logical read, sharing
an already active transaction when called by repair/publication snapshot
helpers. An unconditional nested `BEGIN` would break those callers.

```rust
let snapshot = self.conn.is_autocommit()
    .then(|| self.conn.unchecked_transaction())
    .transpose()?;
// Read bindings, feeds and every programme from this same snapshot.
// ...
if let Some(snapshot) = snapshot {
    snapshot.commit()?;
}
```

Apply this consistency policy to `load_epg`, `for_each_epg_title` and
`epg_guide`; factor a shared helper rather than coupling guide operations to
the existing VOD-specific helper name. Keep the read transaction deferred so
WAL readers continue independently of the writer.

**Regression coverage:** In the existing concurrency harness, coordinate an
EPG replacement between binding collection and programme lookup. Assert the
whole result is either the old guide or the new guide, never a channel/title
mixture. Include a row-count/order change that forces numeric ID reuse, title
iteration, and invocation inside an existing snapshot transaction. Preserve
the isolated guide-input and busy-reader latency gates.

**Disposition:** Open; source and SQL-interleaving evidence confirmed.

## Should fix

### iptv-store-sqlite-002 Migration bookkeeping accepts nonpositive replacement versions

**Location:** `crates/iptv-store-sqlite/src/migrate.rs:150`.

**Rule:** Validate invariants at the boundary (`rust-best-practices`, Model
the domain with types); return an actionable error for corrupted input
(Rust API Guidelines `C-GOOD-ERR`).

**Problem:** `current_version` promises to verify exactly `1..=max`, but checks
only `COUNT(*) == MAX(version)` and nonnegative maximum. The primary key does
not prohibit zero or negative entries. For example, versions `[0, 2]` pass,
despite a missing migration 1; a current-version database with one missing
positive version and a zero row can open without any further migration
validation. This weakens the intended fail-closed corruption check. Portable
snapshot validation also checks individual migration names and catches this
case later; ordinary store opening does not.

**Evidence:** A synthetic SQLite table containing versions 0 and 2 produced
`count=2, max=2, rejected=False` under the current predicate.

**Proposed fix:** Also require the minimum version to be 1 for a nonempty
table. With the integer primary key, count/max/min then establish contiguity.

```rust
let (count, min, max): (i64, i64, i64) = conn.query_row(
    "SELECT COUNT(*), COALESCE(MIN(version), 0), \
     COALESCE(MAX(version), 0) FROM schema_migrations",
    [],
    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
)?;
if count != max || max < 0 || (count > 0 && min != 1) {
    return Err(StoreError::SchemaCorrupt(
        "migration versions are not contiguous from one".into(),
    ));
}
```

**Regression coverage:** Extend private migration tests with empty, valid,
gapped, zero and negative version sets, including `[0, 2]` and `[-1, 2]`.
Verify an attempted ordinary open preserves the damaged file and returns
`SchemaCorrupt` before applying migrations.

**Disposition:** Open; source and predicate probe confirmed.

### iptv-store-sqlite-003 Read then write operations still use deferred transactions

**Locations:** `crates/iptv-store-sqlite/src/recordings.rs:194`,
`crates/iptv-store-sqlite/src/recordings.rs:351`,
`crates/iptv-store-sqlite/src/overlay.rs:225`,
`crates/iptv-store-sqlite/src/organise.rs:152`.

**Rule:** Coordinate shared mutable state at the operation boundary
(`rust-best-practices`, Concurrency and async). The existing
`sources::update_source` implementation documents the same SQLite invariant.

**Problem:** Several public mutations start a deferred transaction, perform
SELECTs and later write. If another connection commits after the initial
read, SQLite WAL cannot promote that stale snapshot to a writer and returns
`SQLITE_BUSY_SNAPSHOT` immediately, regardless of the configured busy timeout.
Recording scheduling/materialization, favorites, and organization edits retain
this pattern even though source and guide writes already use `Immediate` for
the same reason. Independent native owners and ordinary second connections
make this a supported concurrency case; avoid treating every such failure as
an unavailable channel or invalid user request.

**Proposed fix:** Use an immediate transaction for mutations whose first
statement reads, then examine the other such methods as one cohesive change.
Do not change read-only snapshots to immediate transactions.

```rust
let tx = self.conn.transaction_with_behavior(
    rusqlite::TransactionBehavior::Immediate,
)?;
```

Keep identity-sensitive reads inside that same transaction. For example,
`library_preferences::move_group_to` currently constructs its group order
before starting its write transaction, which needs separate relocation of
the reads rather than only changing the transaction mode.

**Regression coverage:** Extend `tests/it/concurrency.rs` to hold a writer,
start one recording/favorite/organization operation on another connection,
then commit and verify the operation waits and succeeds. Add concurrent
same-request recording scheduling to retain idempotency. Confirm writer
contention does not move these operations onto the display reader lane.

**Disposition:** Open; source pattern confirmed. The proposed Rust
interleaving regressions have not been added or run.

## Consider

Parental admission must be reviewed together with `iptv-runtime`. These
adapter APIs expose legacy `channel_overlays.blocked`; they do not implement
the runtime's grant lifetime, modern explicit blocks or category membership
policy. The runtime report owns the admission findings and their fix plan.
Do not independently remove already accepted recording authority while
correcting authorization of new requests.

## What's already good

- The crate forbids unsafe code and routes native initialization through the
  narrow SQLCipher adapter.
- Encrypted staging and atomic publication preserve user overlays and existing
  catalogs on import failure; encryption migration checks the copy before rename.
- Source/feed incarnations, durable repair receipts and affected-row checks
  protect important stale-work and retry boundaries.
- Errors redact JSON values and secret wrappers redact credentials; migration,
  encryption, snapshot and native first-opener tests cover substantial behavior.

## File coverage

The source checklist below records production-code inspection. Private and
integration tests were inspected for relevant scenario coverage; a checked
source file is not a claim that every assertion in its test body was audited.
Platform-conditioned filesystem behavior was read but only Linux evidence is
available. Test and example inventory follows separately to make that limit
explicit.

### Production source and build inputs

- [x] `Cargo.toml` — production source inspected, including conditional branches.
- [x] `src/backup.rs` — production source inspected, including conditional branches.
- [x] `src/catalog.rs` — production source inspected, including conditional branches.
- [x] `src/catalog_stage.rs` — production source inspected, including conditional branches.
- [x] `src/channels.rs` — production source inspected, including conditional branches.
- [x] `src/codec.rs` — production source inspected, including conditional branches.
- [x] `src/encryption.rs` — production source inspected, including conditional branches.
- [x] `src/epg.rs` — production source inspected, including conditional branches.
- [x] `src/epg_assign.rs` — production source inspected, including conditional branches.
- [x] `src/epg_extent.rs` — production source inspected, including conditional branches.
- [x] `src/epg_stage/writer.rs` — production source inspected, including conditional branches.
- [x] `src/epg_stage.rs` — production source inspected, including conditional branches.
- [x] `src/epg_titles.rs` — production source inspected, including conditional branches.
- [x] `src/epg_window.rs` — production source inspected, including conditional branches.
- [x] `src/error.rs` — production source inspected, including conditional branches.
- [x] `src/guide_repair/verification.rs` — production source inspected, including conditional branches.
- [x] `src/guide_repair.rs` — production source inspected, including conditional branches.
- [x] `src/guide_revisions.rs` — production source inspected, including conditional branches.
- [x] `src/guide_sources.rs` — production source inspected, including conditional branches.
- [x] `src/history.rs` — production source inspected, including conditional branches.
- [x] `src/inspection.rs` — production source inspected, including conditional branches.
- [x] `src/inspection_stage.rs` — production source inspected, including conditional branches.
- [x] `src/lib.rs` — production source inspected, including conditional branches.
- [x] `src/library_preferences.rs` — production source inspected, including conditional branches.
- [x] `src/migrate.rs` — production source inspected, including conditional branches.
- [x] `src/order.rs` — production source inspected, including conditional branches.
- [x] `src/organise.rs` — production source inspected, including conditional branches.
- [x] `src/overlay.rs` — production source inspected, including conditional branches.
- [x] `src/parental.rs` — production source inspected, including conditional branches.
- [x] `src/private_file.rs` — production source inspected, including conditional branches.
- [x] `src/recordings/tests.rs` — test harness and relevant scenarios inspected; assertions not exhaustively audited.
- [x] `src/recordings.rs` — production source inspected, including conditional branches.
- [x] `src/search.rs` — production source inspected, including conditional branches.
- [x] `src/search_history.rs` — production source inspected, including conditional branches.
- [x] `src/source_settings.rs` — production source inspected, including conditional branches.
- [x] `src/sources.rs` — production source inspected, including conditional branches.
- [x] `src/vod.rs` — production source inspected, including conditional branches.

### Test fixtures and auxiliary targets

- [x] `tests/common/mod.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/epg_synth/mod.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/initialization_support/mod.rs` — complete file inspected.
- [x] `tests/it/backup.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/catalog.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/catalog_stage.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/child_output.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/cipher_initialization.rs` — complete file inspected.
- [x] `tests/it/concurrency.rs` — complete file inspected.
- [x] `tests/it/custom_group_reset_scale.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/epg.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/epg_stream.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/guide_assign.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/library_preferences.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/main.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/migrations.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/organise.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/overlays.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/perf.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/personalization.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/playlists.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/redaction.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/relative_moves.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/it/search.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/memory/allocations.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/memory/catalog_stream_memory.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/memory/epg_memory.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/memory/epg_stream_memory.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/memory/main.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/staged/mod.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `tests/stream_support/mod.rs` — scenario and harness inventory inspected; full assertion audit deferred.
- [x] `examples/epg_import_bench.rs` — synthetic benchmark source inspected; not executed.
- [x] `examples/epg_window_bench.rs` — synthetic benchmark source inspected; not executed.
- [x] `examples/startup_bench.rs` — synthetic benchmark source inspected; not executed.
- [x] `migrations/0001_provider_data.sql` — schema and migration SQL inspected.
- [x] `migrations/0002_overlays.sql` — schema and migration SQL inspected.
- [x] `migrations/0003_organisation.sql` — schema and migration SQL inspected.
- [x] `migrations/0004_epg.sql` — schema and migration SQL inspected.
- [x] `migrations/0005_source_settings.sql` — schema and migration SQL inspected.
- [x] `migrations/0006_guide_sources.sql` — schema and migration SQL inspected.
- [x] `migrations/0007_library_preferences.sql` — schema and migration SQL inspected.
- [x] `migrations/0008_content_url_policy.sql` — schema and migration SQL inspected.
- [x] `migrations/0009_search_history.sql` — schema and migration SQL inspected.
- [x] `migrations/0010_guide_revisions.sql` — schema and migration SQL inspected.
- [x] `migrations/0011_recordings.sql` — schema and migration SQL inspected.
- [x] `migrations/0012_recording_progress.sql` — schema and migration SQL inspected.
- [x] `migrations/0013_parental.sql` — schema and migration SQL inspected.
- [x] `migrations/0014_vod_progress.sql` — schema and migration SQL inspected.
- [x] `migrations/0015_recording_rules.sql` — schema and migration SQL inspected.
- [x] `migrations/0016_recording_rule_padding.sql` — schema and migration SQL inspected.
- [x] `migrations/0017_vod_resume_episode.sql` — schema and migration SQL inspected.
- [x] `migrations/0018_recording_rule_occurrences.sql` — schema and migration SQL inspected.
- [x] `migrations/0019_vod_episode_parent.sql` — schema and migration SQL inspected.

Historical `tests/fixtures/schema_v*.sql` and `data_v*.sql` were inventoried through the migration harness; individual fixture bytes were not re-audited.

## Disposition summary

| Finding | Severity | State | Required evidence before closure |
|---|---|---|---|
| iptv-store-sqlite-001 | Must fix | Open | Deterministic Rust EPG publication/read interleaving regression |
| iptv-store-sqlite-002 | Should fix | Open | Corrupt migration version-set tests |
| iptv-store-sqlite-003 | Should fix | Open | Recording, favorites and organization writer contention tests |
