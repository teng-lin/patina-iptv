# Rust review of iptv-settings

This portable library provides the declarative settings catalogue, scoped store,
backup migrations and screen state. The catalogue itself uses sensible bounded
values. Public numeric helpers can nevertheless panic on values their signatures
accept, and public typed-key constructors do not enforce the promised type safety.

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

### iptv-settings-001 Numeric entry points can panic before clamping or validation

**Locations:** `crates/iptv-settings/src/screen.rs:594`,
`crates/iptv-settings/src/validate.rs:76`,
`crates/iptv-settings/src/validate.rs:84`.

**Rule:** Expected invalid inputs should be errors rather than panics
(M-PANIC-ON-BUG, Microsoft Rust Guidelines); use checked arithmetic before
conversion/validation (rust-review, Correctness and soundness).

**Problem:** `Screen::step` uses `saturating_add`, but evaluates `steps * step`
with ordinary signed multiplication first. A large public `steps` argument
therefore panics in checked builds or wraps to the wrong direction in release.
The public grid helpers also accept zero/negative steps and arbitrary bounds:
`clamp_to_grid(1, 0, 10, 0)` divides by zero; `check` accepts a public `Setting`
whose `ValueType` has step zero and reaches modulo zero. Full-width signed bounds
can overflow `value - min`. Current built-in catalogue grids avoid the latter
conditions, so those cases are public-API hardening rather than an observed
ordinary settings screen crash.

**Evidence:** With the normal `player.info_panel_timeout` editor open,
`step(i64::MAX)` panics. `clamp_to_grid(1,0,10,0)` also panics in the linked probe.

**Proposed fix:** Saturate the step multiplication; validate grid construction
or return a typed schema error before arithmetic. Compute signed differences in
a wider integer if supporting arbitrary i64 bounds.

```rust
*value = validate::clamp_to_grid(
    value.saturating_add(steps.saturating_mul(step)), min, max, step,
);
// For public custom grids: validate min <= max and step > 0 first.
// Compute (i128::from(value) - i128::from(min)) on valid full-width grids.
```

Choose one consistent checked contract for custom `Setting` construction and
`clamp_to_grid`; do not quietly return an arbitrary value for an invalid schema.
Document any resulting fallible API and update all callers together.

**Regression coverage:** `i64::MIN/MAX` step counts clamp to the expected limit.
Zero/negative steps, reversed bounds and full-width bounds produce documented
results without overflow or divide-by-zero. Use existing schema/screen modules.

## Should fix

### iptv-settings-002 Public typed-key tuple fields allow forged wrong-type keys

**Locations:** `crates/iptv-settings/src/keys.rs:7`,
`crates/iptv-settings/src/store.rs:258`.

**Rule:** Invariant-bearing fields stay private (C-STRUCT-PRIVATE, Rust API
Guidelines); newtypes should validate or constrain construction
(rust-best-practices, Model the domain with types).

**Problem:** All seven key types expose their string field publicly, so callers
can construct `BoolKey("general.clock")` or an unknown key. `Settings::bool`
then panics despite the module saying typed reads cannot ask for the wrong type.
Tests of the predefined constants cannot establish that guarantee for values
constructed by other crates.

**Proposed fix:** Restrict construction to the key module and expose read-only
names as needed. Use the fallible string API for dynamic keys.

```rust
pub struct BoolKey(pub(crate) &'static str);
impl BoolKey {
    pub const fn as_str(self) -> &'static str { self.0 }
}
// Apply the same construction boundary to the other typed keys.
```

**Regression coverage:** Compile-fail documentation for external tuple
construction, plus existing tests that validate every exported constant against
the catalogue. Update callers that currently read `.0`.

## Consider

`Screen::new` accepts an empty static page slice but `page()` indexes its first
entry. Existing app pages are static and nonempty. Either document that
construction precondition or make empty pages explicit when revisiting the API;
this was not elevated to a product defect.

## What's already good

- Writes validate before mutation and preserve explicit choices equal to defaults.
- Deterministic backup serialization preserves unknown and rejected entries.
- Schema tests validate catalogue ranges, defaults, dependency references and migrations.
- Typed error enums implement Display and Error; the crate forbids unsafe code.

## Disposition

| ID | Status | Required follow-up |
|---|---|---|
| iptv-settings-001 | Open | Checked numeric operations and grid contract tests |
| iptv-settings-002 | Open | Restrict key construction and update name readers |

## File coverage

Production implementations and manifest boundaries were reviewed. Tests were
inspected for existing behavioral coverage and relevant assertions; the checklist
distinguishes that from a fresh line-by-line audit of every test body. No native
platform branches or build scripts exist in these five portable crates.

- [x] `Cargo.toml` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/backup.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/catalogue/choices.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/catalogue/commands.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/catalogue/general.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/catalogue/guide.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/catalogue/library.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/catalogue/mod.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/catalogue/player.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/error.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/keys.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/lib.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/schema.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/scope.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/screen.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/store.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/validate.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/value.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `tests/it/backup.rs` — Test coverage inspected; full-body audit pending.
- [x] `tests/it/catalogue.rs` — Test coverage inspected; full-body audit pending.
- [x] `tests/it/main.rs` — Test coverage inspected; full-body audit pending.
- [x] `tests/it/schema.rs` — Test coverage inspected; full-body audit pending.
- [x] `tests/it/screen.rs` — Test coverage inspected; full-body audit pending.
- [x] `tests/it/store.rs` — Test coverage inspected; full-body audit pending.

Validation and full test-body coverage limitations remain explicit; checked file
entries mean the stated review activity occurred, not that every possible defect
has been ruled out.
