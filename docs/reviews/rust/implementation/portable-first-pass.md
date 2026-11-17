# Rust review: first portable implementation batch

Read-only implementation review of W1's import changes, W2's URL/numeric
changes, W10 redaction, and W11 invariant/key boundaries against PR #95 head
`97dbc6b00ac9f3afba966bd4def112f11d8501f3`. These are unpublished portable
libraries with application/runtime caller migrations, edition 2024 and pinned
Rust 1.99.1. The largest remaining concern is incomplete preservation of raw
query bytes. W3, W5, W6 and other pending packages are outside this pass.

Applied the `rust-review` and `rust-best-practices` skills, local AGENTS,
the consolidated fix plan and consolidation decisions. No implementation edits,
Cargo commands, formatter, Clippy, tests or commits were run by this reviewer.
Root owns builds and is continuing implementation concurrently. Existing root
logs show successful initial affected suites, but those results do not establish
verification of the eventual final tree or the counterexamples below.

## Must fix

### portable-fix-001 URL serialization still rewrites raw query bytes (P2)

**Location:** `crates/iptv-providers/src/stalker/parse.rs:647` through the
query-only branch's `set_query`/serialization at line 655.

**Rule:** Preserve documented bytes/behavior during refactoring
(`rust-best-practices`, Refactoring someone else's code); W2 explicitly requires
raw-query preservation for signed material.

**Problem:** Replacing form decode/re-encode with `Url::parse` and `set_query`
does preserve existing percent escapes, but these APIs still normalize raw
characters. For HTTP, URL 2.5.8 uses `SPECIAL_QUERY = QUERY.add(b'\'')`
(`url/src/parser.rs:46` in the pinned local dependency). An original command
`http://media.example.test/play?sig=a'b#part` and reply `?token=new` therefore
produce `sig=a%27b`, violating the selected raw append contract. Reply query
apostrophes also normalize. The present regression covers `%20`, `%2f` and `+`,
so it does not catch this case. This was established from the pinned parser and
setter source, not by running a new reproducer.

**Fix:** Validate the complete original URL separately, then splice the raw
query onto the original string before its fragment. Do not serialize the
validation URL. Preserve both original and reply query bytes, duplicates and
order. For example, the core splice can use the original string boundaries:

```rust
let fragment_at = original.find('#').unwrap_or(original.len());
let before_fragment = &original[..fragment_at];
let separator = if before_fragment.contains('?') { "&" } else { "?" };
original.insert_str(fragment_at, &format!("{separator}{reply_query}"));
```

Keep the existing deliberate empty-query policy when choosing the separator.
Add exact-output cases with raw apostrophes in the old and reply queries,
alongside the existing percent-encoding, fragment and portal-prefix cases.

### portable-fix-002 Xtream playback context retains raw credential-bearing fields (P2)

**Locations:** `crates/iptv-guide/src/playback.rs:119` and
`crates/iptv-providers/src/xtream/mod.rs:254`.

**Rule:** Redact secret-bearing Debug leaves and containing types
(`rust-best-practices`, Types and API design; W10 confidentiality contract).

**Problem:** M3U context headers are now redacted, but the new Xtream Debug arm
formats `XtreamClient`, which still derives Debug over raw `base` and
`user_agent`. A client created with
`with_user_agent(Some("synthetic-private-marker"))` discloses that header through
both its own Debug and the containing PlaybackContext. A generated marker in a
configured base path is also exposed. The new context regression exercises M3U,
pending Stalker and Resolution::Play but omits Xtream. This is formatting
exposure, not an observed production log leak.

**Fix:** Give XtreamClient a manual redacting Debug, preserving its wire
accessors and supported base paths. At minimum:

```rust
f.debug_struct("XtreamClient")
    .field("base", &"[redacted]")
    .field("username", &self.username)
    .field("password", &self.password)
    .field("user_agent", &"[redacted]")
    .finish()
```

Test markers through the leaf and Xtream PlaybackContext. The related public
SourceConfig leaves remain worth including in this same confidentiality gate:
`M3uSourceConfig` derives over raw referrer/user_agent and `XtreamCredentials`
over raw server_url/user_agent (`model.rs:80`, `:89`). Their documented whole-source
Debug guarantee is otherwise stronger than the implementations. Keep any change
limited to formatting; do not reject supported wire input.

## Should fix

### portable-fix-003 Clone an incoming waker before taking the cancellation lock (P2 hardening)

**Location:** `crates/iptv-import/src/cancel.rs:87`.

**Rule:** Do not invoke user code under a coordination lock
(`rust-best-practices`, Concurrency and async; W1 callback ownership).

**Problem:** The revised code correctly wakes and drops replaced wakers outside
the mutex, but it still calls `cx.waker().clone()` under that mutex. A custom
RawWaker's clone callback can reenter `token.cancel()`, which attempts the same
lock and deadlocks. Ordinary Arc-backed executor wakers do not do this; this is
a remaining valid-custom-waker edge case inherited from the old design, not a
newly demonstrated application hang. No custom-waker probe was executed.

**Fix:** Clone before acquiring the lock, then move the clone into the slot.
Declare it before the guard so an early return also drops the clone after the
guard has been released:

```rust
let incoming = cx.waker().clone();
let mut wakers = token.lock();
// Recheck cancellation, allocate/reuse this future's slot.
let previous = wakers[slot].replace(incoming);
drop(wakers);
drop(previous);
```

## Consider

- W10 has no synthetic-marker regression for `guide_inspect::Screen.link` yet;
  add one when closing the package. The manual Debug itself omits the raw value.
- W2's maximum xmltv_ns ordinal is exercised directly at EpisodeNum. Retain the
  planned parser-to-ProgrammeData case so the integration path is covered too.
- The warning cap unit test checks the helper and accounting. A real multi-page
  crawl case would additionally guard against a future merge path bypassing it.
- Formatting and all-target caller compilation remain root-owned gates. No
  finding here substitutes for them, portable wasm builds, or release-mode
  arithmetic checks.

## What's already good

- Cancellation uses per-future slots, replaces the current waker, unregisters
  dropped futures, avoids evicting live waiters, and wakes outside the mutex.
- Numeric fixes widen full calculations, reject invalid custom grids through a
  typed error, and skip unrepresentable search intervals instead of fabricating
  timestamps. Leap-second folding retains its explicit policy.
- Membership's cache-bearing inputs are private, caller reads migrated to
  borrowed accessors, and compile-fail examples protect the boundary. Menu zero
  movement is an immediate no-op.
- Typed-key constructors validate the catalogue before exposing an infallible
  typed read; actual ad hoc callers and the three browse alternatives migrated
  together, with the added constants included in schema validation.
- Stalker endpoint userinfo, playback wire accessors and serialization semantics
  are preserved while the relevant Debug implementations redact their leaves.

## Verdict

Address portable-fix-001 and portable-fix-002 before closing this implementation
batch. portable-fix-003 is small concurrency hardening within W1. No other
blocking regression was established in the reviewed changes. This verdict is
limited to the first batch and will need reconciliation with subsequent edits.

## Correction recheck and regression additions

Rechecked the subsequent implementation by source inspection:

- portable-fix-001 is corrected: the URL is validated separately and query/
  fragment components are spliced from their original bytes, without serializing
  the validation URL. An exact raw-apostrophe case was added.
- portable-fix-002 is corrected: XtreamClient now redacts base/header fields,
  all SourceConfig credential leaves have redacting Debug, and an Xtream context
  marker regression exercises the containing type.
- portable-fix-003 is corrected: the incoming waker is cloned before locking;
  the declaration order and explicit guard drops retain callback safety on early
  cancellation and replacement.

With root authorization, this reviewer added tests only in these three source
files; no Cargo commands, commits or production behavior edits were made:

- `crates/iptv-import/src/stalker.rs`: a private crawl-level test fetches three
  synthetic pages with real parser warnings, then a terminal failure. It checks
  the crawl's retained vector before Catalog conversion, skipped/dropped totals,
  actual page/request counts, valid rows and the final catalogue warning count.
- `crates/iptv-epg/tests/it/parse.rs`: bytewise/chunked XMLTV parsing converts
  maximum and adjacent xmltv_ns ordinals through ProgrammeData and SourceBuilder,
  proving absent overflowing components and unchanged representable metadata.
- `crates/iptv-library/src/guide_inspect.rs`: a link-entry Screen and its containing
  Option redact unique userinfo/path/query markers while preserving wire input.

**Recheck verdict:** no blocking omission remains in this reviewed portable
batch. Root still owns compilation, tests, formatting, wasm and final-tree
integration validation; the new regressions have not been run by this reviewer.
