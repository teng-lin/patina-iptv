# Final portable and ingestion implementation review

Independent source review against PR #95 head
`97dbc6b00ac9f3afba966bd4def112f11d8501f3`, covering W1, W2, W3,
W5, W6, W10 and W11 and their runtime/app/store callers. These are unpublished
portable Rust libraries with native application adapters, edition 2024 and
declared Rust 1.99.1. Applied `rust-review`, `rust-best-practices`, AGENTS.md,
the consolidated fix plan and its consolidation decisions. No source changes,
Cargo/build/test processes, formatter or commits were run by this reviewer.
Root owns validation. Earlier passing suites are supporting evidence, not proof
of the final corrected tree.

## Must fix

No outstanding confirmed findings in the final reviewed scope.

## Corrected during this review

### final-portable-001: page-owner identity did not fence a recreated stored source (P1; source recheck passed)

**Location:** `crates/iptv-runtime/src/commands.rs:617`,
`crates/iptv-runtime/src/data/pages.rs:71`, and
`crates/iptv-runtime/src/data/library.rs:357`.

**Rule:** Preserve asynchronous operation/content ownership and read related
state in one coherent snapshot (consolidated W5; `rust-best-practices`,
Concurrency and async and Refactoring someone else's code).

**Problem and reproduction:** Keep source 1's original schedule installed and
hold the store reader. Request an older page, then delete and recreate source 1
on the independent writer with the same provider channel key and different
generated EPG data. Release the reader before replacing the schedule. The
queued page reads the replacement source because `guide_page_native` receives
only the numeric SourceId. Its original PageRequest still matches the original
installed schedule's owner/attempt, so `arrived` accepts replacement data into
that schedule. This is reachable during the ordinary delete path: after the
delete commits, it awaits the reader-backed metadata refresh and favourite
refresh before retiring/rebuilding the shown schedule. The lower-level public
`library::delete_source` also leaves schedule replacement to its caller.

**Correction rechecked:** The loader now captures the accepting schedule's
SourceAuthority incarnation, and verifies the authority's numeric source before
capturing it. The native page read compares that incarnation with the stored
source and enabled policy inside the same `read_snapshot` that calls `load_epg`.
Missing/mismatched authority fails closed. The error flows through `arrived`,
completing only the original request's waiter with PageReadError and allowing a
later explicit retry. Page-owner/attempt validation remains the separate
publication fence. Its essential structure is:

```rust
store.read_snapshot(|store| {
    if store.source_incarnation(source)? != accepting_incarnation
        || !store.source_policy(source)?.enabled
    {
        return Err(StoreError::Unavailable("guide source changed"));
    }
    store.load_epg(source, &window)
})
```

`data::pages::tests::queued_pages_reject_recreated_disabled_or_unowned_sources`
holds the real reader while an independent encrypted-store connection recreates
or disables the source. It retains the original installed page owner, polls the
loader task to queue its read, then releases the reader. Recreated, disabled and
missing-authority cases fail their exact waiter, publish no loaded page and
retain no waiter. The unfenced baseline would instead accept a successful empty
page and fail these assertions. This source review does not claim that the
reviewer executed the new regression.

### final-portable-002: duplicate name-only rows shared one search result identity (P2; source recheck passed)

**Location:** `crates/iptv-search/src/channels.rs:70`,
`crates/iptv-search/src/lib.rs:33`, and
`crates/iptv-search/src/state.rs:197`.

**Rule:** Preserve distinct content/operation identity and valid caller behavior
(`rust-best-practices`, Types and API design and Refactoring someone else's
code; consolidated W6).

**Problem and reproduction:** `ChannelIndex::from_names(["Generated",
"Generated"])` originally assigned both rows `ChannelId(None, "Generated")`.
After SearchState enters the first result, Down produces the same ResultId and
`position` keeps finding row 0; the second row cannot be independently selected
or played. Production keyed schedule builders overwrite identities, but this
was an introduced regression in the public portable constructor. Default
ProgrammeIndexBuilder identities also needed an explicit anonymous contract.

**Correction rechecked:** Root added a private, monotonically allocated opaque
anonymous discriminator to ChannelId; each name-only/default-builder row gets
its own ID, and rebuilt indexes cannot reuse it. Explicit content/source
constructors and setters retain durable semantic identities. The runtime
resolver rejects anonymous IDs before lookup. Constructor docs now distinguish
anonymous index-local identity from explicit cross-refresh content identity.
The duplicate-label regression checks Down/Play for both rows, distinct extra
keys and rebuilt-index non-aliasing. Root reports its search suite passed;
this reviewer did not execute it.

## Should fix

No additional confirmed findings in the reviewed scope.

## Consider

Add an unchanged-authority success control to the new held-reader native page
test. The current regression meaningfully detects the original bug; a positive
control would additionally guard against accidentally denying every real page.
This is a nonblocking test improvement, not an established implementation bug.

## What's already good

- Cancellation registrations belong to individual futures, survive more than
  32 live waiters, replace changed wakers and unregister on drop. Clone, drop
  and wake callbacks execute outside the cancellation lock. Probe validation
  carries the original operation cancellation/deadline through queued
  configuration and integrity work, with tests observing actual worker exit.
- URL query-only replies preserve raw old/reply bytes, duplicate pair order,
  fragments and portal path prefixes. Numeric fixes widen complete arithmetic,
  reject invalid grids/intervals and UTF-8 offsets, and retain leap-second 60
  while rejecting seconds 61–99. Parser-to-consumer boundary tests are present.
- Guide feed allocation respects active and historical reservations across
  ordinary edits, import preflight, transactional final replanning and repair.
  Header ingestion warns and caps surplus guides before destination processing;
  allocation errors retain the old catalog instead of silently renumbering it.
- Page owner/attempt identity rejects stale success/failure after replacement
  and re-request. Typed read failure stays distinct from successful empty data;
  render queries do not retry failure, and request-specific callbacks finish
  outside mutable borrows.
- Stored search identities include source incarnation and content key; native
  title reads check that incarnation in their snapshot. Programme filtering
  resolves current rows before visibility, catch-up, favourites and limits.
  Normal playback and multiview resolve semantic identity immediately before use.
- Secret Debug leaves and containing contexts redact URLs, arbitrary header
  values, raw entry/retry text and commands while retaining wire values.
  Membership cache inputs and typed-key construction now enforce their
  invariants, with migrated production callers and compile-fail examples.

## Verdict

**No blocker remains in W1/W2/W3/W5/W6/W10/W11 after the two corrections above.**
Both were rechecked in the source and associated regressions. W7 and native
recording/player work are outside this review; no finding here treats their
concurrent implementation as an omission. An adjacent preexisting question
about pending catch-up retries across schedule/epoch replacement was separately
sent to root for its native reviewer; this report does not claim an executed
reproduction or classify it as an introduced defect. Final Cargo, Clippy, wasm,
app and required platform gates remain root-owned.
