# Rust review of iptv-providers

This portable library builds provider requests and parses M3U, Xtream and Stalker data. Its sans-IO and typed-row boundaries are useful, but untrusted account-clock values can panic and Debug redaction is incomplete. Prioritize those defects before changing public API style. Clippy passed in the coordinated workspace run; see validation below.

## Must fix

### iptv-providers-001 Account clock arithmetic panics on valid JSON integers

**Location:** `crates/iptv-providers/src/xtream/mod.rs:205`, `:206`, `:248`; reached from `crates/iptv-providers/src/xtream/parse.rs:115`.

**Rule:** M-PANIC-ON-BUG, Microsoft Rust Guidelines; rust-best-practices section 5 requires expected malformed input to return absence/error rather than panic.

**Problem:** A server can send `time_now="2026-03-01 12:00:00"` with `timestamp_now=9223372036854775807`. The float-to-i32 cast saturates, then multiplication by 15 overflows. A source probe against the built crate printed `server_clock_panicked=true` at line 206. Fixed-offset parsing also accepts signed components and eagerly evaluates the arithmetic inside `then_some`, even when the hours already fail validation. A timezone such as `UTC+2147483647:00` can therefore overflow while being rejected. Debug builds panic; unchecked release arithmetic can manufacture a bogus offset.

**Fix proposal:** Validate and bound before narrowing; perform subtraction as i128 or with checked arithmetic. Reject non-digit/negative hour and minute components and use lazy evaluation after the range check.

```rust
let delta = i128::from(wall) - i128::from(epoch);
let snapped = ((delta as f64 / 60.0) / 15.0).round() * 15.0;
(snapped.abs() <= 14.0 * 60.0).then_some(snapped as i32)
// In parse_fixed_offset, after requiring unsigned decimal components:
(0..=14).contains(&h)
    .then_some(())
    .filter(|_| (0..60).contains(&m))
    .map(|()| sign * (h * 60 + m))
```

**Regression:** Extend existing Xtream clock/account cases with integer endpoints, negative component spellings, oversized hours, and valid quarter-hour zones. Check identical no-panic behavior with overflow checks enabled and disabled. **Disposition:** open; reproduced derived-clock panic, static fixed-offset analysis.

### iptv-providers-002 Debug formatting exposes configured secrets

**Location:** `crates/iptv-providers/src/http.rs:153`, `:157`; `crates/iptv-providers/src/stalker/mod.rs:192`, `:211`.

**Rule:** The crate's documented “No secret leaks” contract; rust-best-practices section 4 requires custom Debug to redact secrets (C-DEBUG, Rust API Guidelines, with the project's confidentiality invariant).

**Problem:** `HttpRequest` only hides Authorization, Cookie and SN values. Its public header API accepts arbitrary credential names, including `X-Api-Key`, `X-Auth-Token` and Proxy-Authorization, which Debug prints verbatim. `StalkerPortal::new` accepts URL userinfo and derived Debug prints its raw `Url`. Synthetic probes printed `custom_header_debug_leaks=true` and `portal_debug_leaks=true`. This establishes a disclosure on formatting; it does not claim a current production log statement exercises it.

**Fix proposal:** Debug should expose header names with redacted values by default. Reject endpoint userinfo or implement a redacting Debug for the endpoint rather than deriving it. Retain explicit methods for wire access.

```rust
let headers: Vec<_> = self.headers.iter()
    .map(|(name, _)| (name.as_str(), "***"))
    .collect();
// Before retaining a Stalker endpoint:
if !endpoint.username().is_empty() || endpoint.password().is_some() {
    return Err(ProviderError::InvalidConfig("URL credentials are not supported".into()));
}
```

**Regression:** Existing redaction tests should format arbitrary header names and endpoint URLs containing unique synthetic markers. Assert that no marker appears. Check affected SourceConfig/stream model Debug paths as part of the same confidentiality audit. **Disposition:** open; both examples reproduced.

### iptv-providers-003 Query-only Stalker links corrupt an existing query

**Location:** `crates/iptv-providers/src/stalker/parse.rs:643` (query-only reply branch).

**Rule:** Respect documented output semantics and validate parsed representations; rust-best-practices section 4, refactoring behavior contracts.

**Problem:** A command `http://media.example.test/play?id=7` and reply `?token=synthetic-new` produce `http://media.example.test/play?id=7?token=synthetic-new`. The second question mark becomes part of the `id` value, so no token parameter is sent and replay can fail. This exact output was reproduced with the public parser.

**Fix proposal:** Compose a parsed URL and append reply pairs to existing query pairs, defining duplicate-key precedence explicitly. Preserve existing no-query behavior.

```rust
let mut parsed = url::Url::parse(&original).map_err(|_| invalid_link())?;
parsed.query_pairs_mut().extend_pairs(
    url::form_urlencoded::parse(url.trim_start_matches('?').as_bytes()),
);
return Ok(parsed.into());
```

`invalid_link` above stands for the existing typed InvalidBody construction; apply equivalent composition when the original command is portal-relative. **Regression:** Query-free, existing-query, fragment and percent-encoded command cases in the existing Stalker integration module. **Disposition:** open; malformed output reproduced; duplicate-key policy to settle before implementation.

## Independent challenge correction

The second source review also found that `parse_fixed_offset` byte-splits a
non-ASCII component before parsing it: `UTC+é1` can panic at a UTF-8 boundary
in `xtream/mod.rs:245`. This belongs to `iptv-providers-001` and requires ASCII
component validation before splitting, in addition to checked arithmetic. This
additional source-confirmed path was not an executable probe in the first pass.
The consolidated plan also replaces the query form decode/re-encode sketch
with raw-query-preserving composition and chooses redacting Debug over rejecting
otherwise supported endpoint userinfo.

## Should fix

No additional independent finding retained.

## Consider

Keep compatibility assumptions such as Stalker archive-hour interpretation and portal-owned alternate ports explicit. They are deliberate behavior and should not be changed by a style cleanup.

## What's already good

- Provider parsing remains portable and contains no networking or unsafe code.
- Typed list framing bounds records/items and skips unknown JSON fields without building complete catalog trees.
- Credential wrapper types zeroize on drop and expose secrets through explicit methods.
- Stable content identity and deterministic duplicate handling protect user overlays across refreshes.

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
| `crates/iptv-providers/Cargo.toml` | Manifest reviewed |
| `crates/iptv-providers/src/b64.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/content_url.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/error.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/http.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/id.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/json_list.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/lenient.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/lib.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/m3u/catchup/mod.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/m3u/catchup/template.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/m3u/catchup/tests.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/m3u/extinf.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/m3u/mod.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/m3u/tests.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/model.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/secret.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/stalker/mod.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/stalker/parse.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/stalker/series.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/stalker/session.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/stalker/typed.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/time.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/typed_row.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/urlenc.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/xtream/mod.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/src/xtream/parse.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-providers/tests/it/common/mod.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-providers/tests/it/main.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-providers/tests/it/stalker.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-providers/tests/it/xtream.rs` | Scenario inventory and targeted test inspection |

Platform and feature coverage is by source inspection only unless explicit runtime evidence appears above. Native device/fuzz validation remains a fix-stage gate.

## Resolution tracking

| Finding | Disposition | Implementation | Required evidence |
|---|---|---|---|
| `iptv-providers-001` | Open | No code changed | Proposed regression and affected crate/caller checks above |
| `iptv-providers-002` | Open | No code changed | Proposed regression and affected crate/caller checks above |
| `iptv-providers-003` | Open | No code changed | Proposed regression and affected crate/caller checks above |
