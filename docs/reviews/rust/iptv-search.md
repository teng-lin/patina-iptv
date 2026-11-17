# Rust review of iptv-search

This portable library owns channel/programme indexes, query matching, search
history and screen state. Ranking and visibility have useful behavioral tests.
The main defect is that supposedly stable result identity contains catalogue
positions, allowing refreshes to retain focus on a different channel.

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

### iptv-search-001 Result identities use catalogue indices across refreshes

**Locations:** `crates/iptv-search/src/state.rs:48`,
`crates/iptv-search/src/state.rs:192`,
`crates/iptv-search/src/state.rs:216`,
`crates/iptv-search/src/channels.rs:20`.

**Rule:** Restore focus by content identity, never list index (AGENTS.md);
model domain identities with types (rust-best-practices, Model the domain with types).

**Problem:** `ResultId::Channel(usize)` and `Programme { channel: usize, start }`
call the channel's position an identity. `landing` retains it exactly, or falls
back to another programme on that same numeric channel. After a catalogue is
reordered or source offsets change, the same number may identify different
content. The app rebuilds indexes and calls `set_results` on the retained
SearchState when data changes, so focus and the subsequent Play effect can move
to a different channel. Sorting result rows with unchanged catalogue indices is
covered by current tests; changing the catalogue itself is not.

**Evidence:** The linked state probe focuses channel index 1, replaces results
with indices from a reordered catalogue and retains `Result(Channel(1))`, even
though index 1 changes from generated Birch to Amber. Inspection of
`iptv-app/src/search_view.rs:283` and `:360` confirms versioned index rebuilds
feed the existing state without converting IDs to content keys.

**Proposed fix:** Carry source-qualified stable keys alongside transient indices
in hits and result IDs. Keep the index only for the current lookup/playback
adapter, resolved against the same current generation.

```rust
pub struct ChannelHit {
    pub channel: usize,
    pub key: String, // Schedule::channel_key, source-qualified in combined mode.
    pub score: MatchQuality,
}
pub enum ResultId {
    Channel(String),
    Programme { channel: String, start: UnixTime },
}
```

Update builders, ProgrammeHit, remembered focus and shell playback resolution
together. Stable keys should remain opaque rather than reconstructed from labels.

**Regression coverage:** Reorder channels while search remains open; reorder
combined sources with duplicate provider IDs; remove/recreate a source; remove
the focused programme. Assert focus stays on the same key and Play resolves its
current row, or uses a documented fallback when the key vanished.

### iptv-search-002 Timestamp compression and search window arithmetic can overflow

**Locations:** `crates/iptv-search/src/programmes.rs:262`,
`crates/iptv-search/src/programmes.rs:399`,
`crates/iptv-search/src/programmes.rs:421`,
`crates/iptv-search/src/programmes_builder.rs:43`.

**Rule:** Do not panic on caller/input values (M-PANIC-ON-BUG, Microsoft Rust
Guidelines); avoid lossy arithmetic and conversions (rust-review, Correctness
and soundness).

**Problem:** `ProgrammeIndexBuilder::push` accepts arbitrary signed start/stop
values without a validity/range check. `start - base` and `stop - start` can
overflow before `u32::try_from` runs. Values beyond the compact u32 span are
silently replaced with u32::MAX/zero duration, changing programme times. Search
similarly uses unchecked now/window/archive/distance arithmetic. Default options
with `now == i64::MIN` panic in a nonempty index; release wrapping produces
incorrect ranges instead. The ordinary current-time catalogue is not known to
reach these extremes, so this is a reproducible public-API boundary defect.

**Evidence:** Linked probes reproduce a finish panic with rows starting at
i64::MIN and zero, and a search panic with a normal one-minute row and
`now == i64::MIN`.

**Proposed fix:** Validate intervals and compact representability explicitly;
return a typed build error or retain full-width times. Use checked/widened
arithmetic for accepted values and saturating window bounds where that preserves
the query contract. Do not silently clamp an occurrence to another date.

```rust
let offset = start.checked_sub(base).ok_or(BuildError::InvalidTimeRange)?;
let offset = u32::try_from(offset).map_err(|_| BuildError::TimeSpanTooWide)?;
let duration = stop.checked_sub(start).filter(|d| *d > 0)
    .ok_or(BuildError::InvalidTimeRange)?;
let from = now.saturating_sub(opts.look_back);
let to = now.saturating_add(opts.look_ahead);
```

The build error/API is proposed, not implemented. Apply the same arithmetic
contract to archive thresholds, reconstructed endpoints and ranking distances.

**Regression coverage:** i64 endpoint boundaries, reversed/zero intervals,
u32::MAX span and span+1, negative look bounds, very large query windows and
ordinary timestamp round trips. Preserve the existing linear-scan equivalence tests.

## Should fix

No additional finding promoted to this category.

## Consider

Search results collect candidates before limiting. This is intentional for total
ranking, especially grouped results; do not replace it with an arbitrary early
limit. Revisit only with a measured memory or latency regression and an
order-preserving selection strategy.

## What's already good

- The inverted index is checked against a synthetic linear scan.
- Visibility filters run before result limits, preventing hidden entries from
  consuming visible result capacity.
- Ranking uses explicit tie-breakers; history has deterministic folding/deduplication.
- Query matching shares EPG normalization, and time enters through parameters.

## Disposition

| ID | Status | Required follow-up |
|---|---|---|
| iptv-search-001 | Open | Stable keys across indexes, state and shell playback |
| iptv-search-002 | Open | Checked timestamp storage/query contract and boundary tests |

## File coverage

Production implementations and manifest boundaries were reviewed. Tests were
inspected for existing behavioral coverage and relevant assertions; the checklist
distinguishes that from a fresh line-by-line audit of every test body. No native
platform branches or build scripts exist in these five portable crates.

- [x] `Cargo.toml` — Implementation reviewed (including applicable inline test coverage).
- [x] `examples/programme_bench.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/channels.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/history.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/lib.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/programmes.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/programmes_builder.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/programmes_tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/state.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/state_tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/text.rs` — Implementation reviewed (including applicable inline test coverage).

Validation and full test-body coverage limitations remain explicit; checked file
entries mean the stated review activity occurred, not that every possible defect
has been ruled out.
