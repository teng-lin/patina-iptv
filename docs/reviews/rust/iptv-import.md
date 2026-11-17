# Rust review of iptv-import

This portable orchestration library drives provider parsers through async Fetch and staging sinks. It preserves runtime independence and generally isolates failed optional work, but its bounded cancellation-waker list can leave legitimate operations asleep forever. Fix cancellation registration before broad API cleanup. Clippy passed in the coordinated workspace run; see validation below.

## Must fix

### iptv-import-001 Cancellation drops live waiters after 32 registrations

**Location:** `crates/iptv-import/src/cancel.rs:74` (32-entry limit and `remove(0)`).

**Rule:** `std::future::Future::poll` wake contract; rust-best-practices section 7 requires correct message/cancellation coordination.

**Problem:** When 33 distinct tasks await a shared token, polling the last waiter discards the first task's waker. `cancel()` drains and wakes only the remaining 32; the first future stays pending forever unless unrelated work repolls it. Dropped waiter registrations are also not removed, so sequential abandoned requests can evict a still-live request without 33 simultaneous users. An isolated probe loading the exact cancel.rs source printed `waiters_woken=32/33; first_waiter_woken=0`.

**Fix proposal:** Give each Cancelled future its own registration identity, replace its registered waker when repolled, and unregister on Drop/completion. Never evict another live waiter merely to maintain a bound. An alternative bounded design must explicitly reject admission before returning a pending future; silently losing wakeups is invalid.

```rust
pub struct Cancelled<'a> {
    token: &'a CancelToken,
    registration: Option<RegistrationId>,
}
// In poll, under the lock after rechecking cancellation:
self.registration = Some(waiters.register_or_replace(self.registration, cx.waker()));
// In Drop: remove the registration if present.
```

The registration helpers are a proposed design, not existing API. Keep this portable and do not add Tokio. **Regression:** Private unit tests with 33+ independently counted wakers; dropped waiters; a changed task waker; concurrent registration/cancel; and cancellation of multiple pending Fetch operations. **Disposition:** open; lost wakeup reproduced.

## Should fix

### iptv-import-002 The Stalker crawl temporarily bypasses the warning retention cap

**Location:** `crates/iptv-import/src/stalker.rs:395`, `:396`, `:449`.

**Rule:** Bound resources at the point of accumulation; rust-best-practices section 11 untrusted-input testing and `model::MAX_STORED_WARNINGS` retention contract.

**Problem:** Each provider page limits stored warnings, but `crawl` appends those warnings into `Crawled::warnings` without a cap. Up to 1000 pages can therefore retain approximately one million warning strings before `into_catalog` finally applies Catalog's 1000-warning limit. The final catalog looks bounded while transient memory scales with all bad pages. This also affects streaming imports, which intentionally avoid retaining catalog-sized data.

**Fix proposal:** Apply the shared warning cap while merging each page, accumulating excess into `out.dropped`; use the same helper for terminal page-failure warnings.

```rust
if out.warnings.len() < iptv_providers::model::MAX_STORED_WARNINGS {
    out.warnings.push(warning);
} else {
    out.dropped += 1;
}
```

**Regression:** Synthetic multi-page Stalker responses containing one valid new row and many malformed rows per page. Observe peak retained warning count through a private unit test, and verify final `warning_count` and `warnings_dropped` account for every warning. **Disposition:** open; confirmed accumulation path by source analysis, no memory benchmark run.

## Consider

Keep Fetch's local-future support and explicit clock/timer ports. Adding a general Send bound or a concrete executor would violate the intended portable adapter contract. `EpgSourceConfig::from_m3u_header` also requires a defined feed-ID exhaustion policy before callers may allocate near u32::MAX; do not silently wrap identifiers.

## What's already good

- Fetch supplies time and I/O without imposing an async runtime or Send on all adapters.
- Guide attempts have separate byte/time budgets and discard incomplete feeds before retry.
- Xtream optional-step failure rolls back provisional rows and retains the previous catalog semantics.
- Fatal content-destination violations are distinguished from optional metadata failures.

## Baseline and validation

- Review baseline: `0e197073a272eceb0631e4d53b08d2823950c416`; Rust edition 2024, MSRV/toolchain 1.99.1; unpublished workspace crate.
- Review only: proposed code appears in this report; no implementation, test, manifest or lockfile changes were made.
- Parent-coordinated workspace Clippy (`--locked --workspace --all-targets --features iptv-runtime/test-support,iptv-app/test-support -- -D warnings`) passed.
- Full `tools/check.sh` stops at the pre-existing NSH pin mismatch: checkout `32769658ad34ad364b960a134cc73c754d50f2d9`, pinned `9ed782cbf362ab63ba18c47dc657fe944c405e50`. Workspace test results are recorded centrally by the coordinator; this report does not claim a separate crate test/fuzz/device run.
- Synthetic source/linked-library probes were built with `rustc`, outside the repository. Linked probes and output: `/tmp/patina-rust-review-20261116/ingestion-linked-probes.rs` and `ingestion-linked-probes.txt` on the review host. These temporary paths are evidence pointers, not portable regression tests.

## File coverage

Production modules were read for contracts and defect paths; inline tests were read where relevant. Integration files were inventoried by scenario and relevant regression cases inspected. “Inventoried” does not mean every test body was audited. No build script exists in this crate.

| File | Coverage |
|---|---|
| `crates/iptv-import/Cargo.toml` | Manifest reviewed |
| `crates/iptv-import/src/backup_manifest.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/cancel.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/catalog.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/catalog_sink.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/ctx.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/epg.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/error.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/fetch.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/lib.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/link.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/m3u.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/on_demand.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/progress.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/refresh.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/refresh_tests.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/sink.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/stalker.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/src/xtream.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-import/tests/it/common/mod.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-import/tests/it/common/synth.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-import/tests/it/epg.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-import/tests/it/limits.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-import/tests/it/m3u.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-import/tests/it/main.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-import/tests/it/on_demand.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-import/tests/it/parser_properties.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-import/tests/it/stalker.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-import/tests/it/xtream.rs` | Scenario inventory and targeted test inspection |

Platform and feature coverage is by source inspection only unless explicit runtime evidence appears above. Native device/fuzz validation remains a fix-stage gate.

## Resolution tracking

| Finding | Disposition | Implementation | Required evidence |
|---|---|---|---|
| `iptv-import-001` | Open | No code changed | Proposed regression and affected crate/caller checks above |
| `iptv-import-002` | Open | No code changed | Proposed regression and affected crate/caller checks above |
