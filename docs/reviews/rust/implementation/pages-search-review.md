# Guide page and search implementation review

Reviewed W5's guide/runtime/app completion path and W6's search identity diff
in the isolated implementation worktree. These are internal portable models
and native runtime/shell adapters, Rust 2024 / declared Rust 1.99.1. Applied
`rust-review` and `rust-best-practices`. Root controls Cargo and all central
validation; this agent ran no builds or tests. Direct rustfmt and diff checks
passed for the assigned page files after the fixes below.

## Must fix

### Default/unpaged readiness newly entered page-index arithmetic

**Location:** `crates/iptv-guide/src/pages.rs:232` and `:244`.

**Rule:** Preserve behavior of valid callers; panic only on programmer bugs
(`rust-best-practices`, errors/panics and refactoring contracts).

**Problem:** `EpgSchedule::ensure_loaded` now calls `retry_failed` before
`with_store` checks whether a time is already in the held store. On default
Pages, `loaded_from == i64::MIN`; entering `index_of` evaluates
`loaded_from - 1 - t` and overflows even for `t == 0`. The new public pending
lookup exposed the same unnecessary arithmetic for in-window times.

**Fix applied:** Return immediately for in-window retry attempts, and return
None for in-window pending lookups. Added
`pages_tests::an_unpaged_schedule_needs_no_page_for_full_width_times`, covering
MIN/0/now/MAX and held-only readiness.

### Held first-screen readiness was mistaken for a loaded page

**Location:** `crates/iptv-runtime/src/data/pages.rs`, `ready`.

**Rule:** Preserve the operation's actual data prerequisites and distinguish
absence/failure from completed data (`rust-review`, correctness/error handling;
consolidated W5).

**Problem:** `ready` returned true whenever an unsuccessful `ensure_loaded`
had no pending PageRequest. A held-only first-screen schedule deliberately
cannot request programmes beyond its held window: it waits for the full
schedule replacement. Returning true allows catch-up to resolve placeholder
programme times before the real guide arrives.

**Fix applied:** A waiter now explicitly represents either an exact page
request or replacement-only readiness. Only matching page waiters complete
on arrivals; held-window waiters complete on `replaced`, preserving the old
first-screen behavior without weakening new page identity checks.

### Rejected evicted replies could leave their own waiter retained indefinitely

**Location:** `crates/iptv-runtime/src/data/pages.rs:126`.

**Rule:** Bound resource lifetime and complete accepted asynchronous work;
retain operation ownership (`rust-best-practices`, concurrency; consolidated W5).

**Problem:** Early-returning on every rejected arrival left the obsolete
request's own callback retained after explicit unpin/eviction and re-request.
The old reply must not complete the new waiter, but its old waiter should not
remain indefinitely merely because publication is correctly rejected.

**Fix applied:** Retire only the obsolete request's exact waiter with a typed
PageReadError. Preserve the newer request and its pending page. Stale arrivals
emit no guide-change event. Matching waiters are removed before user event
callbacks run and are invoked outside the RefCell borrow.

## Regression coverage added

In the existing runtime unit module:

- `data::pages::tests::page_completion_reports_only_its_own_source_success_or_failure`
  completes A while B remains pending, then delivers B's typed failure and
  checks no successful empty cache/render-driven retry; explicit demand retries.
- `data::pages::tests::replacement_schedule_ignores_old_success_and_failure_completions`
  recreates the same source/window with a distinct page owner; old success
  and failure cannot publish or finish the new waiter.
- `data::pages::tests::evicted_reply_retires_its_waiter_without_consuming_the_new_attempt`
  evicts and re-requests the same range, rejects duplicate old replies, and
  proves both callbacks complete once under their own identities.
- `data::pages::tests::held_first_screen_waits_for_replacement_before_resolving_catchup`
  keeps a missing past programme pending until a full replacement arrives.

No test executable was added. App player source was inspected but not edited;
its owner handles callback/session integration and affected native checks.

## Search review

The source-qualified channel/content key is carried separately from current
presentation positions. Channel/programme focus and fallback use that key;
normal Play and multiview resolve against the current schedule immediately
before acting. Combined-source offset changes no longer retarget the action
through a stale channel index. Root is adding the W6 integration regressions.

One adjacent identity question was sent to root rather than independently
changing its active search diff: SQLite sources use an INTEGER PRIMARY KEY,
so deleting/recreating a source may reuse the numeric source ID. ChannelId
currently stores that number plus content key, not the durable incarnation.
An old hit could alias a recreated source carrying the same provider channel
ID. A delete/recreate regression or explicit invalidation/incarnation policy
should decide the scope. This source-level possibility is not claimed as an
executed app reproduction or parental bypass.

## What's already good

- Portable page-owner/per-attempt identity rejects both stale successful data
  and stale failures without requiring runtime-supplied source generations.
- Typed failure remains distinct from a valid empty page and normal rendering
  does not automatically retry failed reads.
- The page payload stays Copy; source qualification is preserved through
  combined schedules and multiview search actions.
- Waiter callbacks run after ownership is resolved and outside mutable borrows.

The fixes and new regressions require root's coordinated Cargo/Clippy/app
validation; this report does not declare those gates passed.

## Final W5 source-recreation correction

A later independent portable review found a remaining source-lifetime hole: an old shown schedule and its exact page request could survive source deletion while a replacement read waited, then load guide rows from a recreated numeric SourceId. `paged` now captures the accepting schedule's SourceAuthority incarnation and passes it through the weak loader. The native page command validates a present matching authority, source existence/enabled state and current incarnation inside the same Store::read_snapshot that loads the guide window. Missing authority fails closed; synthetic tests with custom loaders retain their explicit behavior.

`queued_pages_reject_recreated_disabled_or_unowned_sources` holds a real WAL reader snapshot, queues the real page-loader future, recreates the source with the same numeric ID or disables it before releasing the reader, and checks exact waiter failure with no cached empty page. It separately covers missing authority. The weak-owner test now passes an explicit missing authority without executing its loader. Direct formatting/whitespace checks passed; central Cargo validation is pending.

The held-reader regression also runs an unchanged-authority positive control through the same real loader: success, one loaded page, and a satisfied repeat lookup. Fixture setup/edits use inferred Store closures on the existing named service lanes, so orchestration gains no direct adapter dependency or architecture allowlist exemption. Existing guide journey callbacks now assert typed page success before continuing.
