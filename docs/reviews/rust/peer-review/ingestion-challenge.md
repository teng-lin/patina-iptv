# Ingestion findings: independent challenge

Reviewed 2026-11-16 against baseline `0e197073a272eceb0631e4d53b08d2823950c416`.
Scope: all ten numbered findings in the providers, EPG, import and net reports.
These are unpublished libraries (edition 2024, declared Rust 1.99.1), with portable
provider/EPG/import boundaries and a native transport adapter. This pass applies
the `rust-review` and `rust-best-practices` skills and AGENTS.md. It changes this
report only: no implementation, tests, manifests, staging or commits.

I inspected the cited source, contracts, relevant existing tests, callers and
first-pass probe sources/output. I did not run Cargo or independently rerun those
probes; executable observations below belong to the coordinated first pass.
Static verification supports every defect, but several initial “Must fix” labels
overstate next-release exposure. P1 below means concrete release correctness or
security work; P2 means robustness, API or constrained-threat hardening; P3 means
polish. There are no P3 findings in this group.

## Dispositions

| Finding | Decision | Priority | Corrected release scope |
|---|---|---|---|
| `iptv-providers-001` | Correct | P2 | Extreme/malformed account clock and zone inputs; ordinary valid offsets are unaffected |
| `iptv-providers-002` | Correct | P2 | Secret disclosure when these values are formatted; no demonstrated production logging path |
| `iptv-providers-003` | Correct | P1 | Ordinary query-bearing Stalker commands lose appended authentication parameters; proposed re-encoding fix is unsafe for opaque queries |
| `iptv-epg-001` | Accept | P2 | Maximum u32 episode component panics with overflow checks; unchecked builds wrap the component to zero |
| `iptv-epg-002` | Correct | P2 | Relative schedule end beyond u32 range, approximately 136 years; full-range public-time subtraction is related hardening |
| `iptv-epg-003` | Accept | P2 | Invalid seconds 61–99 are accepted contrary to parser contract; valid seconds remain correct |
| `iptv-import-001` | Correct | P1 | Shared-token distinct task registrations can lose a live cancellation wakeup; no proof that ordinary sequential imports hang |
| `iptv-import-002` | Accept | P2 | Temporary warning retention bypasses the cap across a bounded multi-page crawl |
| `iptv-net-001` | Correct | P2 | Requires local mutation of a private media directory; not a remote arbitrary-file-read exploit |
| `iptv-net-002` | Correct | P2 | Capability-holding stalled readers can occupy four playback slots indefinitely; choose a bounded write policy without a total playback deadline |

No ID should be rejected as a false positive or silently merged away. Findings
can share implementation packages while retaining their separate regressions.
The final plan should not turn the P2 classification into “fixed” or omit it.

## Per-finding verification and patch constraints

### iptv-providers-001 — correct arithmetic and rejection order

`xtream/mod.rs:199–208` accepts `timestamp_now` from
`xtream/parse.rs:108–113`. The first-pass linked probe establishes the saturated
float-to-i32 result overflowing during multiplication at line 206. Additionally,
`wall - epoch` itself can overflow for an early valid calendar date and a large
positive epoch. Negative/zero epochs are deliberately ignored today. The unchecked
release result can be bogus rather than merely absent, including the pathological
signed-minimum `abs` case. This is not evidence that normal contemporary timestamps
or quarter-hour offsets fail.

`parse_fixed_offset` at lines 231–248 accepts signed integer subcomponents and
evaluates `then_some` eagerly. Requiring unsigned ASCII decimal components must
happen **before** its byte-based `split_at`: a malformed name such as `UTC+é1`
also crosses a UTF-8 character boundary at the current split. This additional
same-parser panic is supported by source inspection, not an executable probe.

The report's i128 subtraction, rounding before narrowing, and lazy validated
construction address the overflow. Keep the established snapping and accepted
valid-offset behavior; do not accidentally introduce a new `14:00` boundary policy
while fixing overflow (`14:59` currently passes fixed-name parsing). Use the
existing no-offset fallback rather than clamping nonsense into a plausible zone.

Rule: fallible input must not panic (Microsoft `M-PANIC-ON-BUG`); checked/lossless
arithmetic. Gate: extend `tests/it/xtream.rs::server_clock_offsets` and account-info
parsing with i64 endpoints, early dates, oversized/signed/Unicode components and
normal offsets. Cover checked and unchecked arithmetic configurations. Exercise
catchup formatting and timestamp-less EPG callers, not just the constructor.

### iptv-providers-002 — retain the confidentiality finding, narrow exposure

`http.rs:149–173` prints any header value except Authorization, Cookie and SN.
The public header vector and builder can carry custom credentials.
`stalker/mod.rs:192–227` derives Debug over a Url whose userinfo the constructor
does not reject. The linked probe verifies both synthetic marker disclosures.
Existing redaction tests cover known headers and separately wrapped credentials,
not arbitrary headers or endpoint userinfo. `net/client.rs:248–259` logs a
redacted URL, method and status, not the whole HttpRequest; this pass found no
production formatting call that establishes an actual logged-secret incident.

The original `C-DEBUG` citation alone is insufficient: that rule requests Debug
implementations, not redaction. The directly applicable rule is Microsoft
`M-PUBLIC-DEBUG` (secrets require hand-written formatting and a marker test), plus
the crate's explicit confidentiality contract.

Redact header values by default, or choose a narrow safe diagnostic allowlist.
Changing this deliberately changes the current test expecting visible Accept.
Prefer a redacting `StalkerPortal::Debug` for this package: rejecting userinfo
changes accepted configuration and requires a separate compatibility decision.
Do not assert that rejecting it in this constructor closes every SourceConfig,
endpoint-candidate or model formatting surface. Audit those surfaces with unique
synthetic markers, without expanding this into an unbounded logging refactor.

Gate: retain existing wire/header behavior, format mixed-case arbitrary credential
headers and userinfo through direct and enclosing Debug values, assert markers are
absent, and retain existing transport log-redaction scenarios.

### iptv-providers-003 — accept the defect, replace the patch strategy

`stalker/parse.rs:622–646` explicitly promises append semantics and concatenates a
second `?` after the original command. Existing `tests/it/stalker.rs::link_resolution`
covers query-free absolute and portal-relative originals only. The first-pass
linked output `.../play?id=7?token=synthetic-new` proves the ordinary failure:
the token becomes part of the id value. Appending after an original fragment also
puts the token inside the fragment rather than the HTTP query.

The proposed `query_pairs_mut().extend_pairs(form_urlencoded::parse(...))` is an
incomplete fix. It decodes and re-encodes query material, normalizing spaces,
escape spelling and bare keys; signed/opaque URLs can depend on exact bytes.
A map-based replacement also loses duplicate keys or ordering. Do not use
`Url::join("?token=...")` alone: it replaces the existing query.

Preserve the raw existing and reply query bytes and append with the appropriate
separator **before** a fragment. Preserve duplicate keys in order by default,
consistent with the existing documented “appended” behavior. If replacement of
an old token is desired, settle and test that provider policy separately before
implementing it. Validate the resulting http(s) representation without a
decode/re-encode round trip. Keep the existing portal base-prefix behavior:
`/media/...` intentionally remains below `/stalker_portal` in current tests;
a generic root-relative URL join changes that contract.

Rule: preserve documented observable behavior and opaque credential material.
Gate: existing absolute/relative cases plus existing query, empty query, duplicate
id/token keys, bare key, `%2f`/`%2F`, `%20`/`+`, percent-encoded delimiters and
fragments. Verify both returned bytes and request-target semantics through the
on-demand/playback caller using a local synthetic endpoint. Preserve destination
validation and credential-origin scoping after composition.

### iptv-epg-001 — accept with debug/release distinction

`xmltv.rs:52–68` uses unchecked `u32 + 1`; `store.rs:92–99` reaches it during
programme conversion. A malformed tiny episode field is enough; no huge feed is
required. The first-pass source probe establishes the checked-build panic.
Unchecked builds yield zero; storage later treats zero as absent, so this is not
a demonstrated release process crash or normal-episode failure.

The proposed `and_then(|n| n.checked_add(1))` is appropriately small. Preserve
the independent readable season/episode component: overflow of one should not
discard the other. Gate: existing episode tests plus parser-to-ProgrammeData
coverage for maximum and adjacent numbers, totals, absent components, and valid
ordinary input. Retain the current u16 storage saturation policy separately.
Rule: fallible input must not panic (`M-PANIC-ON-BUG`).

### iptv-epg-002 — widen first, then close related public-input holes

The first-pass linked probe accepts `(0,1)` and
`(u32::MAX-1,u32::MAX+2)`, then panics on a third push at
`settled_source.rs:92`. `start` and `dur` fit independently, but their sum does
not. This needs roughly a 136-year relative end; it is not reproduced by an
ordinary days-long guide. A short duration can trigger it once the relative
start is sufficiently large; a centuries-long individual programme is not
required. `Schedule::stop` already widens operands individually.

The suggested widening is correct for that case. It is insufficient to claim
arbitrary public i64 inputs are now safe: `SettledBuilder::push:99`,
`ScheduleRules::settle:111–112` and retention construction at `settle.rs:44`
subtract unchecked i64 values before fallible narrowing. Audit the matching
SourceBuilder slot construction and preserve rejection/report accounting.
Use checked subtraction or i128 range checks rather than saturating invalid
durations into accepted slots. For accepted slots, reconstructing an end should
continue to equal the already validated original stop.

Rule: lossless arithmetic and fallible-input rejection (`M-PANIC-ON-BUG`). Gate:
existing settled/source-builder equivalence tests, the exact three-row probe,
near-boundary overlap, i64-endpoint public ProgrammeData inputs, retention
cutoffs and persisted readback. `iptv-store-sqlite/src/epg_stage/writer.rs`,
`epg.rs:619` and `epg_window.rs` use these shared rules/builders: run the real
SQLite staging/load scenarios with synthetic extreme rows. Parsed XMLTV dates
have a far narrower range than i64; do not claim ordinary parser output reaches
every public-time endpoint case.

### iptv-epg-003 — accept contract bug, preserve leap-second compatibility

`time.rs:52–60` clamps raw seconds to 59 before testing `>60`, so 61–99 pass.
The public documentation at lines 31–34 rejects impossible calendar values.
The initial source probe's returned timestamp matches that path. No panic,
resource amplification or normal timestamp failure is established.

Validate raw seconds first and then fold accepted second 60 to 59. Do not
change leap-second policy or tighten unrelated tolerant parsing. Gate: raw
seconds 00/59/60/61/99 through `parse_xmltv_time`, and malformed programme
handling through both relevant parser modes, with correct skip/error accounting.
Rule: validate the domain before constructing accepted state (rust-best-practices
section 4 and the public parser contract).

### iptv-import-001 — accept lost wakeup, scope caller impact precisely

`cancel.rs:63–78` evicts the oldest distinct Waker at registration 33; cancellation
only wakes the retained list. The first-pass source probe counts 32/33 wakeups.
The mutex recheck correctly prevents registration-after-cancel races and should
be kept. Repeated polls using the same task waker do not create new entries;
ordinary sequential work in one unchanged task does not itself demonstrate the
bug. Distinct abandoned tasks retain stale registrations and can evict a live
waiter even without 33 simultaneously live operations. Import tests exercise one
in-flight cancellation, not this multi-waker lifecycle.

Keep a registration identity in each Cancelled future, replace its waker on
repoll, and unregister on completion/Drop. Stable vacant slots or another small
private registry suffice; never silently evict a live waiter. Removing the cap
alone fixes wake loss but leaves abandoned-task retention. A single AtomicWaker
does not support the public multi-waiter contract. Stable IDs must not overwrite
a live registration on reuse/wrap, and cancellation should take registrations
under the lock and wake outside it. Avoid invoking destructive user-waker behavior
while holding the registry lock where possible.

Keep public signatures and local-future support. Do not add Tokio, a runtime
dependency, Send bounds, blocking waits or a thread to iptv-import. Net live-input
preparation and runtime owner/import teardown also consume these tokens. A
public-API lost wakeup is release correctness even though this pass has not
reproduced a typical UI import hang.

Rule: Future::poll wake contract and portable cancellation invariants. Gate:
private counted-waker unit cases for 33+ live tasks, one live plus many dropped
registrations, changed waker, duplicate polls, cancellation before/during
registration, completion/Drop cleanup and idempotent cancel. Retain import
in-flight cancellation, net live preparation cancellation and runtime staging
rollback/owner-retirement scenarios. Build the portable crate for wasm without
introducing a native executor requirement.

### iptv-import-002 — accept bounded transient amplification

`stalker.rs:395–402` collects each page's capped warning vector into an uncapped
per-crawl vector; terminal failure at line 449 also appends directly. Only
`Crawled::into_catalog:349–355` applies Catalog's 1000-warning cap.
`PagedCrawl` stops at 1000 pages and stops if a page adds no new row
(`providers/stalker/parse.rs:260–307`). Approximately one million retained
warnings therefore requires continued valid new content alongside bad rows;
all-malformed or all-duplicate pages alone do not run to 1000 pages. Request,
byte, item and deadline limits can also terminate the crawl. This is finite
transient amplification, not an unbounded final catalog.

Apply the cap at both accumulation sites and increment `out.dropped` for each
discarded warning, while preserving provider-page dropped counts and the final
Catalog cap. Do not cap the page loop or silently lose warning counts to hide
the symptom. A small Crawled helper is enough; no cross-crate warning framework
is warranted. Gate: multiple pages each with a unique valid row and many bad
rows, terminal failure after useful rows, exact final total/drop accounting and
an assertion that per-crawl retained storage never exceeds the cap. Exercise
both collecting and streaming-sink imports. Rule: bound resources where they
accumulate; documented `MAX_STORED_WARNINGS` retention contract.

### iptv-net-001 — correct local threat and acquisition semantics

`recording_file.rs:76–81` separates lstat from plain open and only reads length
from the opened descriptor. Local path replacement between them can redirect
reads or block the blocking-pool open on a FIFO. `recording.rs:123–150` uses
O_NOFOLLOW for resume on Unix, but not nonblocking open or descriptor type
validation before truncation. A writer-only FIFO without a reader can block;
with a reader it can open and then fail truncation. These are real acquisition
gaps. A timeout around tokio::fs::open cannot terminate a blocked OS open.

The caller checks recording status, parental admission and retained media
(`runtime/recordings.rs:759–768,823–850`). Startup protects the private directory
and refuses symlinks (`runtime/storage.rs`, `docs/security.md:121–124`). The
finding requires later mutation by an actor able to write there; a remote
provider cannot choose this pathname. No race was dynamically reproduced in the
first pass. Calling this a remote arbitrary-file-read or confirmed exploit would
be unsupported. No-follow plus regular-file checks also do not authenticate the
identity/content of a malicious replacement regular file or prevent hard links.

Use the opened handle for type, length and resume alignment/limit decisions,
then mutate it; the pre-open lstat size is stale under replacement. Unix needs
O_NOFOLLOW and O_NONBLOCK before opening plus descriptor is_file validation.
`net/file.rs::open_regular` supplies a useful nonblocking/type pattern, but
intentionally follows user-picked symlinks: do not globally add private-file
restrictions to that helper. Windows needs an explicit reparse-point acquisition
policy; the startup check alone is not a race-safe open. Keep std::fs writes
synchronous to preserve the existing cancellation/finalization invariant, rather
than introducing queued writes that race finalization.

Rule: validate acquired resources and preserve the private-file contract. Gate:
helper-level deterministic symlink/FIFO replacement on Unix without stranded
workers, nonregular handles, changed regular-file lengths, normal resumed torn
packet truncation/append, byte limits and runtime recording recovery/playback.
Exercise Windows handle/reparse behavior on that host before declaring its gate
passed. A timed-out test alone does not prove no OS worker remains blocked.

### iptv-net-002 — correct policy and reproduction limits

Only request headers have a deadline (`recording_file.rs:50–62`); 404, 416,
successful headers and the body copy all write without one (`:71–111`). Four
authorized GETs can occupy every slot. The first-pass exact-source sparse-file
probe shows four headers delivered and a new range GET receiving EOF after
18 seconds while the old clients remain open. It establishes the missing
lifetime bound, not that an unauthenticated remote client knows the random
loopback capability or that ordinary playback reproduces it.

Adopt a bounded write policy consistent with native playback (`playback.rs:816`)
and live chunks (`live.rs:469`), including small response/error headers. Do not
wrap the entire copy in a fixed 15-second timeout: legitimate recordings can
play for hours. Bounded reads followed by timed writes are a minimal approach.
However, timeout(write_all(chunk)) is a per-chunk deadline, not a strict
progress-reset idle timer: a slow but progressing chunk can exceed it. Choose
and document chunk size/policy accordingly, or implement a progress-reset write
loop if promising exact idle semantics. A Tokio timeout cannot unblock an
underlying special-file open from net-001; fix acquisition separately.

Gate: four stalled authenticated synthetic clients release slots after the
chosen write deadline; a later range GET succeeds; a healthy transfer lasts
longer than 15 seconds while making acceptable progress; HEAD/error writes and
shutdown release resources. Cover player pause/resume or reopen behavior so
expected read pauses do not become an unexplained playback regression. Use
existing native harness modules and CI timing isolation; avoid timing assertions
that depend on an unspecified OS socket buffer filling at a particular instant.
Rule: bounded resource lifetime and established transport backpressure policy.

## Minimal cohesive implementation packages and gates

1. **Stalker query reply composition (providers-003, P1).** Preserve byte-level
   query semantics and existing portal-relative behavior; providers link tests,
   import on-demand flow and native destination/credential-scoping checks.
2. **Portable cancellation registration (import-001, P1).** Private identity and
   Drop lifecycle; import, net live and runtime cancellation/rollback scenarios;
   wasm build. No executor or API generalization.
3. **Parser arithmetic and validation (providers-001, epg-001/002/003, P2).**
   Small owning-module fixes can land separately within this package. Include
   EPG store rule/readback parity, checked versus unchecked arithmetic, portable
   wasm builds and the relevant existing parser fuzz lanes. Do not combine
   unrelated time-format policy changes with overflow rejection.
4. **Secret-safe diagnostic formatting (providers-002, P2).** Header/portal Debug
   implementations and enclosing model marker audit; preserve accepted URLs and
   wire bytes unless a separate configuration policy is approved.
5. **Stalker transient warning retention (import-002, P2).** Two accumulation
   sites, exact counts, streaming and collecting paths. No warning-system rewrite.
6. **Native recording file acquisition (net-001, P2).** Read and resume handles,
   platform flags/type checks, opened-handle length and byte limits; runtime
   recovery and cancellation behavior. Keep platform-dependent evidence visible.
7. **Native recording serving writes (net-002, P2).** Per-write or progress-idle
   policy, bounded copy, all response paths, slot recovery, long healthy playback
   and teardown. It can follow acquisition but must retain a separate regression.

All regression scenarios belong in existing private unit tests or existing
`tests/it/` harness modules; add no new integration executable. No Cargo checks
were run in this second pass. Reuse coordinated baseline evidence rather than
claiming it validates future patches. At implementation time use the shared CI
plan for strict timing lanes, affected ordinary tests and separate doctests,
portable builds and the app build. The missing wasm standard library and exact
NSH revision mismatch recorded in README remain unresolved gates; host successes
do not satisfy them. Native Windows or changed Android branches require their
platform checks/device evidence under AGENTS.md.

## What to preserve

- Runtime-free sans-IO provider parsing and local-future Fetch support.
- The cancellation mutex recheck and waking outside the cancellation lock.
- Independent page/crawl admission limits and exact total warning accounting.
- Parental/capability admission, origin-scoped credentials and bounded native
  relay resources, with long playback permitted.
- Shared EPG settlement semantics between memory and real persisted storage.

## Consolidated-plan review

Reviewed `docs/plans/2026-11-16-rust-review-consolidated-fix-plan.md`, draft dated
2026-11-16, against every ingestion ID and the source-backed constraints above.
The finding table preserves all ten IDs and the challenged priorities: P1 for
providers-003/import-001, P2 for the other eight. None is incorrectly marked fixed.

| ID | Final package | Verification against challenge |
|---|---|---|
| providers-001 | W2 | Extreme-input scope retained; widening precedes narrowing; non-ASCII rejection precedes byte splitting, including `UTC+é1` |
| providers-002 | W10 | Header values redact by default; nested marker gates; no logging incident claimed and endpoint-userinfo compatibility preserved |
| providers-003 | W2 | Raw append chosen explicitly; duplicates/order, signed-query encoding, fragments and portal base-prefix semantics preserved; replacement requires a separate evidence-backed behavior change |
| epg-001 | W2 | Maximum ordinal rejects consistently; debug/release arithmetic and parser-to-consumer gates retained |
| epg-002 | W2 | Centuries-scale scope retained; arbitrary i64-base arithmetic explicitly included rather than widening only the u32 sum |
| epg-003 | W2 | Raw-second validation precedes folding; deliberate leap-second behavior preserved |
| import-001 | W1 | Stable registration, changed waker and Drop/completion cleanup; live-waiter eviction prohibited; waking outside lock, native live preparation/runtime rollback and portable/runtime-free boundary retained |
| import-002 | W1 | Per-page accumulation capped with dropped-count accounting; multi-page regression retained |
| net-001 | W12 | Private-directory replacement scope; no-follow/nonblocking/type checks, Windows handling and opened-handle length/limits all explicit |
| net-002 | W12 | Actual progress-reset 15-second header/body write policy selected; whole-chunk timeout distinguished; progressing playback over 15 seconds, HEAD/error headers, pause/reopen and teardown covered |

The work-wave sequence and global gates preserve existing harnesses, wasm builds,
parser fuzzing, real storage/caller integration, isolated timing lanes and native
validation. W10 does not turn formatting hardening into configuration rejection.
W12 correctly chooses one write policy rather than leaving mutually incompatible
timeout semantics for the implementer. No unsafe implementation is authorized by
these packages.

**Final verdict: approved for future implementation planning; no blocking ingestion
omission remains.** I reread the revised exact W1/W2/W12 wording. W2 now explicitly
chooses documented raw append, preserves duplicate ordering and portal base-prefix
behavior, and reserves replacement for a separate evidence-backed change. W1 now
names waking outside the cancellation lock, native live preparation and runtime
rollback. W12 now requires healthy progress beyond 15 seconds, HEAD/error headers
and pause/reopen in addition to slot recovery and cancellation. These changes close
the prior plan blocker and the associated gate clarifications.

For import-002, this owning challenge still supplies the terminal failure-warning
and collecting/streaming regression details under W1's accumulation/counting gate;
the shorter package summary does not waive them. This is an execution detail to
retain, not an unresolved behavior decision or approval blocker.

This is a plan review, with no Cargo or probe rerun. Implementation, missing wasm
toolchain support, the NSH exact-revision gate and affected native/device checks
remain future gates, not completed work.
