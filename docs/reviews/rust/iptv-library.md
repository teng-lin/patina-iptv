# Rust review of iptv-library

This portable library supplies shared domain records, inspection/repair contract
values and playlist/guide screen state. Source-qualified IDs and typed repair
values are useful foundations. The strongest defects are secret-bearing Debug
output and overlapping provider/user feed-ID allocation.

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

### iptv-library-001 Editor and inspection Debug output includes credentials

**Locations:** `crates/iptv-library/src/playlists.rs:70`,
`crates/iptv-library/src/epg_sources.rs:64`,
`crates/iptv-library/src/epg_sources.rs:75`,
`crates/iptv-library/src/guide_inspect.rs:44`.

**Rule:** Redact secrets in Debug (rust-best-practices, Types and API design);
credential/logging boundary in `docs/security.md`.

**Problem:** Playlist `Editing` derives Debug while its public `text` can contain
a newly typed password. Guide editing derives it over credential-bearing link
text, an AddName URL, and retained retry text. Inspection `Screen` similarly
derives Debug over `link`. The containing screen derives transitively expose
these values. Redacting stored `SecretUrl` or `details::Form` does not cover the
unwrapped editing state. This proves diagnostic exposure; no production log
call is required or claimed.

**Evidence:** The synthetic public `Editing` probe reports
`library_edit_debug_leaks=true` for a generated password.

**Proposed fix:** Use secret wrappers or manual Debug for each raw-input holder.
Redact before the containing derives can format it.

```rust
impl std::fmt::Debug for playlists::Editing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editing")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("text", &"[redacted]")
            .field("prefilled", &self.prefilled)
            .finish()
    }
}
// Redact epg_sources::EditKind::AddName.url, Editing.text and inspection link too.
```

**Regression coverage:** Format leaf and containing screen values during
password entry, AddLink, AddName, Relink, failed-link retry and inspection-link
entry. Assert generated secret markers never appear.

### iptv-library-002 Provider feeds collide with user feed IDs

**Locations:** `crates/iptv-library/src/guides.rs:43`,
`crates/iptv-library/src/guides.rs:74`.

**Rule:** Make identity invariants unrepresentable or validate construction
(rust-best-practices, Mindset); source/feed identity contracts in the architecture.

**Problem:** Provider feed `n` is `2 + n`; the second user guide is 1001.
After two user additions, syncing 1000 provider URLs therefore creates two
sources with FeedId(1001). The M3U header parser accepts an unbounded comma list
within its ordinary record budget, so the allocator's assumed namespace gap is
not enforced. Duplicate IDs can fail publication or make subsequent ID-based
lookup/editing target the wrong feed. `next_user_feed` can also overflow at
u32::MAX rather than returning a controlled exhaustion error.

**Evidence:** The linked probe adds user guides 1 and 1001, syncs 1000 generated
URLs, and reports `library_feed_1001_count=2`.

**Proposed fix:** Preserve existing persisted IDs, enforce a documented provider
feed count before synchronization and make allocation failure explicit. A cap
of 999 provider feeds preserves provider IDs 2..=1000; check for conflicts with
all retained sources and use checked addition for user allocation.

```rust
const MAX_PROVIDER_FEEDS: usize = 999;
// Change synchronization to a typed Result and update its callers.
if urls.len() > MAX_PROVIDER_FEEDS {
    return Err(GuideEditError::TooManyProviderFeeds);
}
let next = highest.max(MORE_USER_FEEDS)
    .checked_add(1).ok_or(GuideEditError::FeedIdsExhausted)?;
// Reject duplicate retained IDs before publishing any update.
```

Do not silently truncate provider guides or renumber existing feeds: reject the
invalid update atomically and preserve the previous usable configuration.

**Regression coverage:** Provider counts 998/999/1000 with existing user guides;
adding a user guide after a provider refresh; duplicate restored IDs; exhaustion
at u32::MAX; failure preserves the previous configuration and explicit mappings.

## Should fix

No additional finding promoted to this category.

## Consider

Keep the raw request fields in inspection/repair values distinct from validated
runtime admission. `Request::valid` is structural validation and does not grant
access or establish candidate/feed incarnation freshness. Preserve those runtime
checks when fixing IDs or error APIs.

## What's already good

- Shared records use source-qualified content IDs and redacting provider types.
- Inspection windows use checked subtraction and bounded page/query constants.
- Repair requests, previews and outcomes distinguish durable commitment from
  effective publication and unknown completion.
- Screen generations reject stale replies; focus generally uses content identity.

## Disposition

| ID | Status | Required follow-up |
|---|---|---|
| iptv-library-001 | Open | Redact raw-input holders and test containing Debug output |
| iptv-library-002 | Open | Enforce disjoint allocation and atomic failure; update callers |

## File coverage

Production implementations and manifest boundaries were reviewed. Tests were
inspected for existing behavioral coverage and relevant assertions; the checklist
distinguishes that from a fresh line-by-line audit of every test body. No native
platform branches or build scripts exist in these five portable crates.

- [x] `Cargo.toml` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/details.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/epg_sources.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/epg_sources_tests.rs` — Test coverage inspected; full-body audit pending.
- [x] `src/guide_assign.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/guide_inspect.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/guide_repair.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/guide_repair_screen.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/guides.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/inspection.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/lib.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/playlists.rs` — Implementation reviewed (including applicable inline test coverage).
- [x] `src/records.rs` — Implementation reviewed (including applicable inline test coverage).

Validation and full test-body coverage limitations remain explicit; checked file
entries mean the stated review activity occurred, not that every possible defect
has been ruled out.
