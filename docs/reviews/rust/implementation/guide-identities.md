# Guide feed identity implementation (W3)

Finding: `iptv-library-002`. Root approved this implementation design before
API changes. The pure library, importer, store publication/edit boundaries and
runtime callers now share one allocation policy. Validation remains centrally
owned; this agent has not run Cargo or created commits.

## Identity policy and compatibility

- Existing configured feed IDs are never renumbered. Existing provider indices
  retain their IDs, position, names, enabled state and time shift across refresh.
- Pure helpers reject duplicate IDs and duplicate provider indices with
  `GuideFeedError`. New allocations use checked addition and return Exhausted
  instead of wrapping. Callers may distinguish the specific library error;
  storage reports its typed InvalidData category without URL/credential data.
- New provider IDs start at 2, then allocate above every active or durably
  reserved identity. New user IDs use 1 for a completely fresh list; otherwise
  they retain the preference for 1001 or higher while respecting reservations.
  Thus the first newly added user guide after providers can now be 1001 instead
  of 1. Existing user IDs and manual mappings remain unchanged.
- Persistent callers supply the store's shared high-water floor. It includes
  configured feeds, retained EPG/cache/provenance rows, manual mappings and the
  existing bounded legacy repair-history seed. This avoids assuming that active
  configuration rows are the complete reserved set.
- `set_guide_sources_in` remembers the highest current/new/reserved ID before
  removing any row, in the caller's transaction. Ordinary edits, publication,
  repair and undo therefore share the same existing durable ledger. No schema
  migration or silent mapping repair is introduced. Invalid metadata/configs
  fail closed before publication.

## Resource bounds and admission

`MAX_PROVIDER_GUIDES = 999` is a work bound, not a numeric ID namespace. M3U
imports discard surplus declared guides before destination validation/fetching
and record one warning with the omitted count. Valid playlist content survives;
warning text contains no discarded URL. Checked header planning rejects numeric
exhaustion before constructing any partial list. Direct oversized planning is a
typed error, while the normal import path has already retained its warned cap.

Runtime preflight uses `Store::plan_provider_guides` in one coherent read
snapshot, propagating errors instead of converting unreadable configuration to
an empty guide list. Final catalog publication replans against current rows and
the durable reservation floor inside its existing Immediate transaction. An
intervening guide edit cannot reuse a preflight numeric ID. The helper returns
the exact guides published with the catalog. Ordinary guide edits likewise plan,
reserve and store under one Immediate transaction.

## API/caller changes

- `guides::sync_provider` now returns `Result<Vec<GuideSource>, GuideFeedError>`.
  Persistent planning uses `sync_provider_with_high_water`.
- `guides::apply` now returns
  `Result<Option<Vec<GuideSource>>, GuideFeedError>`; no change remains None.
  Persistent edit application uses `apply_with_high_water`.
- `next_user_feed` is checked; its reservation-aware counterpart accepts the
  durable floor. `validate_sources` exposes the shared identity check.
- `EpgSourceConfig::from_m3u_header` and `Catalog::epg_sources` now return checked
  results. Existing import/network tests and network/store examples migrated.
- Runtime `imports.rs` uses fallible snapshot planning and atomic publication
  replanning; `library::edit_guides` delegates to `Store::edit_guide_sources`.
- Existing library/runtime/app fixture callers unwrap their known valid data.
  The changes do not introduce another integration executable or dependency.

## Added regression coverage

- Library tests cover 1000 supplied URLs plus IDs 1/2/1001, the warned-path
  retained 999, distinct allocation, duplicate identity/index errors, full-width
  exhaustion, preserved existing rows and high-water floors for both allocation
  kinds.
- Import tests preserve one valid playlist while warning about the surplus
  guide, verify only the playlist is requested, and reject overflowing header ID
  plans while accepting the adjacent last representable IDs.
- Store tests emulate a historical repair receipt, retain a manual feed mapping,
  allocate through ordinary edits/provider planning, remove an allocated guide
  and prove its ID stays reserved. Another test changes guides after preflight
  and proves final publication allocates against the intervening edit.
- Store exhaustion tests preserve source details, guide configuration, channel
  catalog and the high-water ledger; failed staged files are removed.
- Runtime import recovery now checks retained provider configuration/reservations
  on cancellation after staging, and separately verifies exhausted planning
  reports Storage while retaining the old source, catalog and guide IDs.

The earlier three review gaps were also filled by this agent: the private real
multi-page Stalker crawl warning test, parser-to-ProgrammeData episode ordinal
test, and inspection-link leaf/container redaction test. Their review record is
in [portable-first-pass.md](portable-first-pass.md).

## agy packet 5 disposition

Accepted the public-boundary hardening finding. Runtime `imports::save_in`
already supplies `initial` only immediately after `add_source`; the ordinary
refresh path passes None. The store helper nevertheless previously trusted any
Some(initial), allowing another caller's stale seed to discard intervening
guide edits. The suggested source-existence test would reject valid seeds,
because even new sources exist before staged publication begins.

Provider replanning now always reads current guide rows and uses `initial` only
when those rows are empty and the shared durable feed high-water is zero.
Current rows and historical reservations otherwise take precedence. The API
documentation states this actual initialization boundary, including a list
cleared after earlier edits. The check runs inside the existing publication
transaction and does not change source creation or the direct replacement API.

The existing intervening-edit publication test now deliberately passes its
stale preflight plan as Some(initial). Two further owning-module tests verify
that a cleared list with a retained high-water rejects the seed and allocates
above history, while uninitialized state accepts its prepared user/provider
IDs, enablement and time shift. All scenarios use real local staged catalog
publication. No Cargo command or commit was run for this follow-up; Android
validation retained exclusive use of the shared build cache. Scoped
`git diff --check` passes.

## Validation handoff

Root was notified when the patch became compile-ready. Required central checks
include library/import/store ordinary tests, runtime test-support import recovery,
all-target caller compilation, wasm builds and the final combined suite. Passing
source inspection here does not mark those checks passed.
