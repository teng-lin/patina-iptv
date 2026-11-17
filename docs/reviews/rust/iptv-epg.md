# Rust review of iptv-epg

This portable library parses XMLTV and builds compact, matched schedules. Its streaming and strict-document paths have substantial defensive checks, but an untrusted episode ordinal can panic and a compact-slot addition can overflow after accepted input. Fix those arithmetic defects while preserving the tolerant versus strict parser distinction. Clippy passed in the coordinated workspace run; see validation below.

## Must fix

### iptv-epg-001 Untrusted zero-based episode numbers overflow

**Location:** `crates/iptv-epg/src/xmltv.rs:63`; reached by `crates/iptv-epg/src/store.rs:95` when converting parsed programmes.

**Rule:** M-PANIC-ON-BUG, Microsoft Rust Guidelines; use checked numeric conversions and treat malformed input as absence.

**Problem:** The `xmltv_ns` field parser converts an input u32 into a one-based ordinal using `n + 1`. A tiny feed with `<episode-num system="xmltv_ns">4294967295.0.</episode-num>` reaches an overflow in debug builds. In release it wraps to zero and later becomes absent. An isolated probe importing the exact source printed `episode_number_panicked=true` with the panic at line 63.

**Fix proposal:** Reject an ordinal that cannot be represented after conversion.

```rust
p.parse::<u32>().ok().and_then(|n| n.checked_add(1))
```

**Regression:** Add maximum and adjacent numeric values to the existing `xmltv::tests::episode_numbers` cases and a parser-to-ProgrammeData integration case. Assert no panic and consistent absence for the overflowing component. **Disposition:** open; reproduced on current source.

### iptv-epg-002 Accepted compact slots overflow before widening

**Location:** `crates/iptv-epg/src/settled_source.rs:92`.

**Rule:** Checked/lossless arithmetic; M-PANIC-ON-BUG, Microsoft Rust Guidelines.

**Problem:** `last.start` and `last.dur` are individually valid u32 values, but `last.start + last.dur` is added as u32 before conversion to i64. With accepted rows `(0,1)` and `(u32::MAX-1,u32::MAX+2)`, pushing a third row panics while calculating the previous end. A linked-crate probe printed `settled_builder_panicked=true`. Release wrapping makes the overlap check use the wrong end instead.

**Fix proposal:** Widen both operands before adding, matching `Schedule::stop` in store.rs. Also audit subtraction guards in the same builders for full-range public UnixTime inputs.

```rust
Some(last) => (
    sched.base,
    sched.base + i64::from(last.start) + i64::from(last.dur),
),
```

**Regression:** Extend existing SettledBuilder tests with the three rows above, ensuring the third is dropped for compact-range overflow without panic. Test overlap detection near the end of the relative slot range. **Disposition:** open; reproduced against the current built crate.

### iptv-epg-003 Invalid seconds are clamped before validation

**Location:** `crates/iptv-epg/src/time.rs:53`, `:59`.

**Rule:** Make invalid states unrepresentable; rust-best-practices section 4 and this function's impossible-calendar-value contract.

**Problem:** The seconds component is reduced with `.min(59)` before the `second > 60` check, making that check unreachable. `20260301123099 +0000` is accepted as second 59, although the documentation promises rejection of impossible values. The source probe returned `Some(1772368259)`. Folding a real leap second is deliberate; accepting 61 through 99 is not required by that comment.

**Fix proposal:** Validate the raw second and clamp only the valid leap-second value.

```rust
let second = if digits >= 14 { num(12, 2) } else { 0 };
// Keep second > 60 in the existing validation guard.
let second = second.min(59);
```

**Regression:** Table-test seconds 00, 59, 60, 61 and 99 through the public time parser and malformed programme path. Retain the deliberate second-60 folding policy. **Disposition:** open; reproduced on current source.

## Should fix

No additional independent finding retained.

## Consider

`MatchIndex::add_source` only marks ambiguity when inserting full normalized names; later tag-stripped collisions are skipped. Confirm whether `ambiguous_name` is intended to include collisions between stripped names before extending that diagnostic. This is a documented matching-policy question rather than a required behavior change in this pass.

## What's already good

- Strict inspection parsing rejects DTDs, invalid entities, malformed document structure and excessive nesting.
- Incremental decoding handles gzip, charset boundaries and record budgets without retaining complete feeds.
- Schedule settlement is shared across in-memory and persistent consumers.
- Matching uses stable identities, explicit overrides and deterministic source priority.

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
| `crates/iptv-epg/Cargo.toml` | Manifest reviewed |
| `crates/iptv-epg/src/decode.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/extra.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/intern.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/lib.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/matching.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/normalize.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/parser.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/settle.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/settled_source.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/store.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/strict.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/superscript.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/time.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/src/xmltv.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-epg/tests/it/large.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-epg/tests/it/main.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-epg/tests/it/matching.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-epg/tests/it/parse.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-epg/tests/it/record_limits.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-epg/tests/it/store.rs` | Scenario inventory and targeted test inspection |

Platform and feature coverage is by source inspection only unless explicit runtime evidence appears above. Native device/fuzz validation remains a fix-stage gate.

## Resolution tracking

| Finding | Disposition | Implementation | Required evidence |
|---|---|---|---|
| `iptv-epg-001` | Open | No code changed | Proposed regression and affected crate/caller checks above |
| `iptv-epg-002` | Open | No code changed | Proposed regression and affected crate/caller checks above |
| `iptv-epg-003` | Open | No code changed | Proposed regression and affected crate/caller checks above |
