# Rust review of iptv-guide

This portable library adapts imported channels and EPG data to the core Schedule
trait, including combined sources, lazy pages and playback resolution. Its most
important correctness gap is that a page reply cannot identify the schedule
that requested it. Playback diagnostic values also expose raw credentials.

## Baseline and validation

- Reviewed revision: `0e197073a272eceb0631e4d53b08d2823950c416` on 2026-11-16.
- Rust edition 2024; workspace MSRV/toolchain 1.99.1; internal unpublished crate.
- Review only: source, tests and manifests were not changed; no fixes or commits made.
- Coordinated workspace formatting and Clippy (`--locked --workspace --all-targets
  --features iptv-runtime/test-support,iptv-app/test-support -- -D warnings`) passed.
  The coordinated non-player workspace `--tests` run passed, including this crate.
  Native-player/device evidence is separate; see the consolidated baseline.
- Full `tools/check.sh` stops at NSH pin drift: actual `32769658ad34ad364b960a134cc73c754d50f2d9`
  versus pinned `9ed782cbf362ab63ba18c47dc657fe944c405e50`. The wasm core build cannot
  run because the target is absent. These are validation gaps, not crate defects.
- Synthetic probes were compiled with `rustc --edition=2024 -L dependency=target/debug/deps`
  against the existing crate artifacts. Probe sources are outside the repository at
  `/tmp/patina-portable-review/`; they establish the behavior described below, not
  full platform or fuzz validation.

## Must fix

### iptv-guide-001 Playback Debug output exposes secret stream commands and headers

**Locations:** `crates/iptv-guide/src/playback.rs:32`,
`crates/iptv-guide/src/playback.rs:97`,
`crates/iptv-guide/src/playback.rs:254`.

**Rule:** Redact secrets in Debug (rust-best-practices, Types and API design);
credential/logging boundary in `docs/security.md`.

**Problem:** `PlaybackContext` derives Debug through `Inner::M3u`, whose Referrer
may contain account/token query values. `StalkerLink` derives it over raw `cmd`
and channel-header values. `Resolution::Play` also inherits the separate
`iptv-core-001` PlaySource exposure. Existing redacted SourceAuthority and
provider session implementations do not protect these other fields. The finding
is formatting exposure, not a claim that a production logger currently emits it.

**Evidence:** A generated token in M3U Referrer survives `format!("{context:?}")`;
the linked probe prints `guide_context_debug_leaks=true`.

**Proposed fix:** Implement Debug manually at the leaf secret-bearing types.

```rust
impl std::fmt::Debug for StalkerLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StalkerLink")
            .field("session", &self.session)
            .field("cmd", &"[redacted]")
            .field("channel_headers", &"[redacted]")
            .finish()
    }
}
// Redact M3u Referrer and other raw identity header values in PlaybackContext.
```

**Regression coverage:** Format all context/resolution variants with generated
credentials in commands, Referrer, headers and returned stream URLs. Include
the containing Resolution, not just leaf wrappers.

### iptv-guide-002 Same-window schedules accept each other's page replies

**Locations:** `crates/iptv-guide/src/pages.rs:36`,
`crates/iptv-guide/src/pages.rs:155`,
`crates/iptv-guide/src/lib.rs:257`.

**Rule:** State and asynchronous work must retain content/owner identity
(AGENTS.md and runtime v1 contract); encode valid states explicitly
(rust-best-practices, Mindset).

**Problem:** `PageRequest` has only page index and time bounds. `Pages::insert`
checks those values and the presence of a pending slot, so an old schedule's
reply is accepted by a replacement schedule with the same window. This also
permits a reply from a prior evict/re-request of the same page to win the newer
request. Runtime `data/pages.rs::arrived` finds the current schedule by source,
so the missing identity is relevant to refresh races rather than merely to a
caller deliberately misusing the API.

**Evidence:** The `stale_page.rs` linked probe constructs two independent
same-window schedules and starts the same page in both. Their requests compare
equal; inserting the first schedule's request into the second returns true.
The runtime-side finding is tracked separately by the runtime reviewer.

**Proposed fix:** Carry a schedule/snapshot generation and per-request ticket
through the loader and verify both before publication. The runtime supplies a
fresh accepting generation when constructing the schedule; avoid platform APIs
or a new ambient global owner in the portable crate.

```rust
pub struct PageRequest {
    pub generation: u64,
    pub ticket: u64,
    pub page: u32,
    pub from: UnixTime,
    pub to: UnixTime,
}
// Inside insert, before moving store into the pending slot:
if request.generation != self.generation || request.ticket != page.ticket {
    return false;
}
```

The exact new constructor arguments and ticket ownership must be implemented
with the runtime consumer. A source ID or time-window comparison alone is not a
snapshot fence.

**Regression coverage:** Old/new schedules with identical bounds; same-page
request after eviction; source refresh/recreation; out-of-order replies. Verify
rejected stores do not alter loaded counts or schedule contents, and a current
reply still succeeds.

## Should fix

No additional finding promoted to this category.

## Consider

`MAX_PAGES` intentionally caps resident loaded stores, not pending requests.
The public probe can enqueue 100 distinct missing pages, but that alone is not
a violation of its documented cap. If runtime queue pressure is addressed,
bound in-flight requests separately and provide explicit retry/failure semantics;
do not mislabel the current loaded-page cap.

## What's already good

- Prepared catalogue/organization values move expensive work off the UI thread
  while retained schedules keep deliberate local Rc state.
- Combined schedules source-qualify identities and convert child epochs.
- Native source authority has explicit redacted diagnostics and captures the
  source incarnation used for resolution.
- Resident paging has LRU eviction and a pin for the archive being played.

## Disposition

| ID | Status | Required follow-up |
|---|---|---|
| iptv-guide-001 | Open | Redact playback diagnostic leaves and containers |
| iptv-guide-002 | Open | Add schedule/request identity jointly with runtime paging |

## File coverage

Production implementations and manifest boundaries were reviewed. Tests were
inspected for existing behavioral coverage and relevant assertions; the checklist
distinguishes that from a fresh line-by-line audit of every test body. No native
platform branches or build scripts exist in these five portable crates.

- [x] `Cargo.toml` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/combined.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/held.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/held_tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/index_tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/lib.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/pages.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/pages_tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/playback/tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/playback.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `tests/it/main.rs` — Test coverage inspected; full-body audit pending.
- [x] `tests/it/organization_worker.rs` — Test coverage inspected; full-body audit pending.
- [x] `tests/it/personalization.rs` — Test coverage inspected; full-body audit pending.

Validation and full test-body coverage limitations remain explicit; checked file
entries mean the stated review activity occurred, not that every possible defect
has been ruled out.
