# Rust review of iptv-core

This portable library owns UI-independent guide, playback and feature state. The
main defects are credential-bearing diagnostic output and public state that can
invalidate a private index. Existing navigation, parental and playback tests cover
many normal transitions, but the probes below identify untested public-API edges.

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

### iptv-core-001 Credential-bearing playback values and wizard events expose secrets through Debug

**Locations:** `crates/iptv-core/src/playback.rs:25`,
`crates/iptv-core/src/wizard.rs:539`.

**Rule:** Redact secrets in `Debug` (rust-best-practices, Types and API design);
credential/logging boundary in `docs/security.md`.

**Problem:** `PlaySource` derives `Debug` over the resolved URL and all header
values, including Authorization and Cookie. `WizardEvent` derives it over raw
`SetField.value` and `ClipboardText.text`, including passwords. Redacting the
wizard's stored form does not protect its incoming events. Formatting either
public value exposes the secret; this finding establishes diagnostic exposure,
not an observed production log leak.

**Evidence:** Synthetic linked probes printed
`core_play_source_debug_leaks=true` and `core_wizard_event_debug_leaks=true`.

**Proposed fix:** Replace the derives with explicit implementations that preserve
nonsecret state but omit all URL/header values and raw event payloads. For example:

```rust
impl std::fmt::Debug for PlaySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaySource")
            .field("public_only", &self.public_only)
            .field("url", &"[redacted]")
            .field("headers", &"[redacted]")
            .finish()
    }
}
// In WizardEvent::fmt, redact SetField and ClipboardText payloads;
// do not route those variants back through a derived Debug implementation.
```

**Regression coverage:** Format each public container with generated secrets in
URL path/query/userinfo, Authorization/Cookie and password/clipboard events.
Assert no marker survives, while variant/field identification remains useful.

### iptv-core-002 Public Membership rows can invalidate private matching indices

**Locations:** `crates/iptv-core/src/membership.rs:15`,
`crates/iptv-core/src/membership.rs:34`, `crates/iptv-core/src/membership.rs:40`.

**Rule:** Keep invariant-bearing fields private (C-STRUCT-PRIVATE, Rust API
Guidelines); make invalid states unrepresentable (rust-best-practices, Mindset).

**Problem:** `matching` caches indices into `rows`, but callers may clear, replace
or reorder the public vector without rebuilding it. `visible()` and
`visible_at()` then index stale positions, panicking or showing incorrect rows.
Direct mutation of public `query` similarly leaves the cached filter stale.
No unsafe code is involved; ordinary safe library calls reach the panic.

**Evidence:** Construct one row, call `membership.rows.clear()`, then
`membership.visible()`: the linked probe catches an index-out-of-bounds panic.

**Proposed fix:** Make `rows` and `query` private, retain `reload`/`filter` as
mutation boundaries, and provide borrowed getters. Update existing readers.

```rust
pub struct Membership {
    rows: Vec<Member>,
    query: String,
    // Other fields unchanged.
}
impl Membership {
    pub fn rows(&self) -> &[Member] { &self.rows }
    pub fn query(&self) -> &str { &self.query }
}
```

**Regression coverage:** Reload a shorter/reordered vector while filtered and
verify visible IDs and focus. Verify callers cannot bypass rebuilding through
public mutation after the API change; reuse the existing integration harness.

### iptv-core-003 Zero-step menu navigation can loop forever

**Location:** `crates/iptv-core/src/menu.rs:107`.

**Rule:** Core operations must not block (AGENTS.md); correctness and soundness
review includes reachable nontermination (rust-review, What to look for).

**Problem:** `move_focus` advances by `step.signum()`. With `step == 0` and a
current nonfocusable row, the loop repeatedly inspects the same row forever.
`Menu::new` permits a heading-only or all-disabled menu and starts its focus at
zero, so no public-state corruption is necessary.

**Evidence:** A separate linked process creates a heading-only menu and calls
`move_focus(0)`; it times out after two seconds. The probe process was terminated.

**Proposed fix:** Treat zero movement as a no-op before entering the scan.

```rust
if step == 0 {
    return;
}
```

**Regression coverage:** Zero steps on heading-only, all-disabled and ordinary
menus return immediately without changing focus. Keep these cases in the
owning private unit-test module.

## Should fix

No additional finding promoted to this category.

## Consider

`Timeline::set_span` supports 60–360 minutes, while `Timeline::wanted`
(`crates/iptv-core/src/timeline.rs:243`) and `Band::covers` still use the fixed
150-minute `VISIBLE`. The shell currently computes its wider rendering and
prefetch span itself, so this is a public-model consistency follow-up rather
than a demonstrated current rendering failure. If these helpers remain public,
make their span explicit and test the supported spans.

## What's already good

- `forbid(unsafe_code)` and caller-supplied clocks keep the model portable.
- PIN values redact diagnostics and zeroize on drop; parental grants distinguish
  settings administration from content unlocks.
- Favorites, organization and VOD models generally retain stable content keys.
- Multiview assigns persistent surfaces and records session/detour state explicitly.

## Disposition

| ID | Status | Required follow-up |
|---|---|---|
| iptv-core-001 | Open | Redaction implementations and marker regression tests |
| iptv-core-002 | Open | Seal cache mutation boundaries; update callers |
| iptv-core-003 | Open | Zero-step guard and regression test |

## File coverage

Production implementations and manifest boundaries were reviewed. Tests were
inspected for existing behavioral coverage and relevant assertions; the checklist
distinguishes that from a fresh line-by-line audit of every test body. No native
platform branches or build scripts exist in these five portable crates.

- [x] `Cargo.toml` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/browse/tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/browse.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/browse_playback.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/capabilities.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/channel_list.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/channel_picker.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/clock.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/editor.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/favorites.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/guide/floor/tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/guide/floor.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/guide/moves.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/guide.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/i18n/calendar.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/i18n/message.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/i18n/plural.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/i18n.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/import_limits.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/lib.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/lists/tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/lists.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/live.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/membership.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/menu.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/multiview/layout.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/multiview/menu.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/multiview/tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/multiview.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/on_demand.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/organization.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/panels/tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/panels.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/parental.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/pip.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/pip_window.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/play_bar.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/playback/error.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/playback/health.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/playback/live_tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/playback/states.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/playback/tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/playback.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/playback_windows.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/player_info.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/player_options.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/recording.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/reorder.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/seek_hold.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/sound.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/timeline/tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/timeline.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/tracks.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/vod.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/wizard.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `tests/it/main.rs` — Test coverage inspected; full-body audit pending.
- [x] `tests/it/membership_scale.rs` — Test coverage inspected; full-body audit pending.
- [x] `tests/it/personalization.rs` — Test coverage inspected; full-body audit pending.

Validation and full test-body coverage limitations remain explicit; checked file
entries mean the stated review activity occurred, not that every possible defect
has been ruled out.
