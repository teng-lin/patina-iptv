# Store implementation first-pass review

Reviewed the W4/W13 diff in the isolated implementation worktree, based on
PR #95 (`97dbc6b0`). This is the internal native persistence library, Rust
2024 / declared Rust 1.99.1. Applied `rust-review` and `rust-best-practices`.
No Cargo, Clippy or tests were run by this reviewer; root owns validation.
The reviewer initially read source only, then made the explicitly assigned
follow-up changes in the three existing integration test modules.

## Must fix

### Historical test ledgers used placeholder migration names

**Location:** `crates/iptv-store-sqlite/tests/it/migrations.rs`, `build_old_file`.

**Rule:** Preserve supported behavior and test historical data at the actual
boundary (`rust-best-practices`, refactoring behavior contracts and testing).

**Problem:** New ordinary-open migration-name validation correctly rejects
unknown names, but the historical fixture builder used `historical` for every
ledger row. Consequently otherwise valid historical schema/data fixtures fail
before migration. Root's coordinated first test run confirmed five failures.

**Fix applied:** Cache canonical `(version, name)` values from a fresh scratch
Store once per integration executable. Populate each old fixture's ledger from
that list, while preserving every historical schema and data fixture. Keep
production wrong-name validation and its regression unchanged. No new public
migration metadata API was introduced.

**Disposition:** Test construction corrected; coordinated rerun pending.

## Should fix

### Pin transaction reuse, rollback and contended mutation behavior in regressions

**Locations:** `crates/iptv-store-sqlite/tests/it/epg.rs` and
`crates/iptv-store-sqlite/tests/it/concurrency.rs`.

**Rule:** Tests should verify relevant behavior and concurrency boundaries,
not mirror implementation (`rust-best-practices`, testing; consolidated W4).

**Problem:** The new EPG title publication regression is meaningful, but does
not exercise caller-owned transaction reuse or release after error. Existing
held-writer coverage exercised previously fixed source/guide setters, rather
than the newly changed recording/favourite/organization mutations.

**Fix applied:** Added cases in existing harness modules:

- `epg::nested_epg_snapshots_share_the_caller_and_recover_after_read_errors`
  combines the generic, VOD and repair snapshot helpers with EPG metadata,
  guide loading and title iteration. It handles an inner error while retaining
  the caller's snapshot, then verifies outer error cleanup through a new read
  and write transaction.
- `concurrency::recording_favourite_and_custom_order_mutations_wait_for_the_other_writer`
  checks recording insertion, favourite movement and custom-group movement
  while another writer holds an uncommitted update.
- `concurrency::group_move_reads_the_order_after_a_competing_commit` holds an
  actual reorder, then applies a relative move. Its expected final order
  distinguishes reading before write admission from reading the new commit.
- `concurrency::concurrent_identical_recording_requests_commit_one_job` starts
  two competing identical requests and checks one durable job/returned ID.

The held-writer cases reuse the existing 200 ms release pattern; they do not
assert strict timing thresholds and join the release thread before asserting
operation success. No integration executable was added.

**Disposition:** Coverage added; coordinated validation pending.

## Consider

No additional production correctness defect was confirmed in this diff.
`read_snapshot` starts a deferred read transaction only in autocommit mode;
its RAII guard releases owned snapshots on errors and leaves caller-owned
transactions alone. EPG inner statements return/drop before outer commit.
Actual mutation transactions take write intent; read-only and analysis
operations remain deferred. `move_group_to` reads its order after admission.

Runtime snapshot integration, parental admission and feed allocation are
separate unfinished packages at this review point and were not reported as
omissions in the store batch. This review does not certify other-platform
execution, final runtime API behavior or the full final PR.

## What's already good

- One reusable read helper fixes multi-statement snapshot ownership without
  introducing nested `BEGIN` errors through existing VOD/repair wrappers.
- Migration count/max/min and known-name checks reject malformed ledgers
  without weakening historical version or future-schema protections.
- The title callback provides a deterministic publication interleaving using
  real store APIs and synthetic data.
- Follow-up tests retain the repository's existing harness budget.

Only assigned test modules and this document were changed by this reviewer.
Direct rustfmt and `git diff --check` passed for those test files.
