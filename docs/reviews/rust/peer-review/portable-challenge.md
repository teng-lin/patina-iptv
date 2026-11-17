# Portable findings: independent challenge review

Reviewed 2026-11-16 against baseline
`0e197073a272eceb0631e4d53b08d2823950c416`. Scope is all eleven numbered
findings in the core, settings, library, guide and search reports. These are
internal, unpublished portable libraries in an edition-2024 workspace declaring
Rust 1.99.1. I applied the `rust-review` and `rust-best-practices` skills,
checked the cited implementations, their current application/runtime consumers
and the relevant existing tests. This pass ran no Cargo commands or new probes;
the coordinated evidence in [the review index](../README.md) is the baseline,
not fresh validation by this reviewer. No source, tests or manifests changed.

The mechanisms behind all eleven findings are present. Their severity is not
uniform: catalogue focus and asynchronous page ownership affect ordinary app
refreshes; several panic examples require unusual public calls that the current
app does not make. Diagnostic formatting exposure is established, but the
reviewed production consumers do not establish a log leak.

## Disposition of every finding

P1 means next-release correctness/security; P2 means bounded robustness or API
work; P3 means polish. A disposition of **correct** retains the defect while
correcting its priority, contract or proposed fix. **Merge** retains the ID for
traceability but assigns one implementation package.

| ID | Disposition | Priority | Independent conclusion |
|---|---|---|---|
| iptv-core-001 | correct | P2 | Raw playback URL/header and wizard text Debug exposure is real; no demonstrated production logging path. |
| iptv-core-002 | correct | P2 | Safe public mutation invalidates the cached indices; current custom-group app mutations use reload/filter. |
| iptv-core-003 | correct | P2 | Zero movement hangs on a nonfocusable row; app menu navigation supplies only -1/+1. |
| iptv-settings-001 | correct | P2 | Arithmetic defects are real at public boundaries; app steps and catalogue grids are bounded. |
| iptv-settings-002 | correct | P2 | Forging keys contradicts the typed-key promise; real callers also construct valid ad hoc keys and must migrate. |
| iptv-library-001 | accept | P2 | Raw editing/link fields bypass redacted stored values; containing derives expose them. |
| iptv-library-002 | accept | P1 | Valid provider-header input can overlap persisted user feed IDs; all mutation/publication paths need atomic failure. |
| iptv-guide-001 | accept | P2 | M3U identity headers and Stalker command/header fields are raw Debug leaves; PlaySource is a shared dependency. |
| iptv-guide-002 | merge | P1 | Merge with iptv-runtime-003: same-window replacement and evict/re-request need owner and attempt identity. |
| iptv-search-001 | accept | P1 | Rebuilt catalogue offsets can redirect retained focus and subsequent playback to another channel. |
| iptv-search-002 | accept | P2 | Compact time storage and query arithmetic overflow or alter times; extreme public clocks are not current app clocks. |

## Must fix: P1 packages

### Stable search identity — iptv-search-001

`crates/iptv-search/src/state.rs:48` stores channel positions in both result
variants. `landing` (`:188`) compares them across replacement results and its
same-channel fallback repeats the positional assumption. The app prepares
versioned indexes (`crates/iptv-app/src/search_view.rs:285`) and installs results
in the existing state (`:358`). `resume` prepares again and reruns the query
(`:464`), so this is reachable when a suspended search returns after catalogue
changes even without relying on an arbitrary external state call.
`choose` (`:478`) and `outcome` (`:126`) turn the surviving position directly
into live playback, catch-up or details.

The existing `crates/iptv-search/src/state_tests.rs` focus tests retain numeric
channel IDs while rearranging result rows; they do not simulate catalogue
reindexing. The channel visibility tests intentionally preserve catalogue
indices for playback, which remains useful within a single snapshot.

The proposed `String` identity is a direction, not a complete patch. Preserve a
transient index for current rendering/ranking, carry the schedule's opaque stable
key for state identity, and resolve the key against the current schedule at the
final choose/pick boundary. Update channel and programme hits, remembered focus,
`ChannelIndex::from_schedule`, the programme builder/runtime reader and the app's
row, outcome and multiview-pick consumers together. `ChannelHit` and `ResultId`
currently derive `Copy`; adding owned strings requires correcting their value
passing and derives. `ChannelIndex::from_names` cannot invent provider identity:
give synthetic/name-only users explicit keys or a clearly snapshot-scoped
construction path. Do not derive stable keys from channel labels or continue
using stale indices after only fixing `ResultId`.

Combined schedules already qualify keys by source
(`crates/iptv-guide/src/combined.rs:291`); single schedules expose their key at
`crates/iptv-guide/src/lib.rs:1010`. Preserve that convention and check how source
replacement affects incarnation, rather than adding a second identity scheme.
Cover suspended/open search refresh, reordered channels and combined-source
offsets, duplicate provider-local IDs, removed focused programmes, and a vanished
key. Assert the focused key and the channel actually played/picked, including
the interval before fresh results arrive. Keep state transitions in the portable
crate and scenario integration in the existing app harness.

### Paging ownership — iptv-guide-002 + iptv-runtime-003

`crates/iptv-guide/src/pages.rs:36` identifies only the day/window; `insert`
(`:155`) checks that window and an empty slot. `evict` removes pending slots as
well as loaded slots (`:190`), so the same schedule can accept an earlier attempt
after re-requesting the same page. Runtime `loader`
(`crates/iptv-runtime/src/data/pages.rs:55`) captures the source, while `arrived`
(`:78`) looks up the *current* same-source child. Thus a stale read can satisfy a
new schedule's request after a refresh in the same hour. Merely checking owner
runtime or SourceId is insufficient.

Existing `crates/iptv-guide/src/pages_tests.rs:224` rejects an evicted reply when
there is no new pending slot and rejects changed bounds. It does not request the
same page again before delivering that reply. Runtime page tests cover combined
readiness and weak ownership, which should remain intact.

Use one cross-crate package. Add an issuing page-owner/snapshot identity and a
per-attempt ticket; check both in success and failure completion before changing
the slot. A runtime-supplied generation is suitable if it is fresh for every
accepting schedule and source recreation. The report's sketch must also add the
generation to construction, the ticket to each pending Page, allocate it at
request time, and carry the unchanged request through `commands::guide_page`
and runtime delivery. An exact retained page-owner handle is another viable
ownership implementation, but still needs attempt identity and separately fenced
current-UI notifications. Do not introduce global platform state into the
portable crate, retain a strong Runtime cycle, or replace intentional `Rc`/
`RefCell` ownership with threads or `Send` bounds.

Implement runtime-004's explicit failure/retry lifecycle in the same paging
package or immediately afterward on this request identity. A failure from an old
attempt must not clear a new attempt. Preserve successful-empty-page caching.
Regressions: identical-window schedules, evict/re-request, old reply before and
after new reply, single/combined refresh, source deletion/recreation, and stale
failure after a newer request. Check contents, pending/loaded counts and current
notifications/waiters, not just the boolean insertion result.

### Feed allocation — iptv-library-002

`crates/iptv-library/src/guides.rs:43` considers only user sources; `:74` allocates
provider feed `2 + n`. The second user source is 1001, so the 1000th provider URL
also becomes 1001. `crates/iptv-providers/src/m3u/mod.rs:285` accepts a comma list
without this count cap, making it reachable from provider content within the
record-size budget. `next_user_feed` also has unchecked increment. Normal tests
in `guides.rs:184` claim no collision using only two provider feeds.

The proposed cap of 999 prevents *new canonical* provider IDs from entering
1001+, but does not establish integrity of already persisted/custom source
records. Preserve persisted identities and mappings; reject duplicate retained
feed IDs or allocation conflicts and use checked conversions/addition before
building the replacement. Define typed allocation/synchronization failure and
retain the previous configuration/catalogue atomically. Do not silently truncate
URLs, renumber feeds, or persist half an edit.

Caller scope is concrete: imports plan synchronization at
`crates/iptv-runtime/src/imports.rs:627`, publication-time resynchronization at
`:911`, and guide editing at `crates/iptv-runtime/src/library.rs:349`. The latter
currently uses `Option` for unchanged, which must remain distinguishable from
failure. Map errors through existing runtime/store public failure contracts;
avoid an unrelated blanket return-type redesign. `stored.rs:834` is a test
fixture, not a production restore call. Update it and other affected fixture
builders; inspect store restoration/validation for acceptance of invalid retained
IDs and reject them at the appropriate publication boundary.

Tests belong in existing `guides.rs` tests and runtime/store harness modules:
998/999/1000 providers, both orders of adding user guides and refreshing,
duplicate/custom retained IDs, exhausted user ID, and failed refresh/edit with
the former usable catalogue and guide assignments preserved. Ensure a cap error
at publication does not leave an added source orphaned or mask an earlier failure.

## Should fix: P2 packages

### Diagnostic boundaries — core-001, library-001 and guide-001

The cited derives are directly visible in source:
`iptv-core/src/playback.rs:25`, `wizard.rs:539`;
`iptv-library/src/playlists.rs:70`, `epg_sources.rs:64`/`:75`,
`guide_inspect.rs:35` (the raw link is at `:47`, rather than report line 44);
`iptv-guide/src/playback.rs:32`/`:97`/`:254`. Every path can expose raw generated
secrets through ordinary `format!("{value:?}")`. `Resolution::Play` depends on
the core PlaySource fix; `Resolution::StalkerLink` depends on its own leaf fix.

The app playlist and guide-source handlers log `effect_name`/`edit_name`
(`crates/iptv-app/src/playlists_view.rs:153`, `epg_sources_view.rs:137`), and the
wizard logs import kind/ID (`wizard_view.rs:161`). These are not evidence that
the raw values are logged. Keep the findings as P2 diagnostic-boundary defects
unless a concrete logging/telemetry consumer is established separately. Do not
claim a production credential disclosure merely because Debug is public.

One cohesive redaction package can implement manual Debug on the actual leaves:
PlaySource; text-carrying WizardEvent variants; playlist Editing; guide Editing
and its separately public EditKind::AddName; inspection Screen; PlaybackContext
or private Inner; and StalkerLink. A safe outer Debug alone does not repair a
separately public raw-input leaf. Redact arbitrary URL path/query/userinfo and
header values, including User-Agent/Referrer strings; whitelisting only common
password parameter names is insufficient. Keep useful variant names, field
identifiers and nonsecret counts. Existing provider/session redaction remains.

`wizard.rs:1761` tests stored wizard/draft formatting, not incoming events.
`iptv-guide/src/playback/tests.rs:423` tests ResolutionAuthority, not the actual
PlaybackContext/Resolution leaves. Existing guide-source tests verify that links
are not prefilled, a different property. Add synthetic marker assertions for
leaf and containing screens/events/resolutions, including failed edit retry,
AddLink -> AddName, inspection input, and mixed-case/custom headers. Put private
tests beside the types and integration scenarios in existing harnesses. Formatting
changes need no runtime API redesign, dependency or new executable.

### Cached membership — iptv-core-002

`crates/iptv-core/src/membership.rs:11` exposes `rows`/`query` while `visible`
(`:29`) and `visible_at` (`:35`) index through private `matching`. Clearing rows
panics; reordering rows silently changes which names match. This is a real safe
API flaw, but `crates/iptv-app/src/custom_groups.rs:196`/`:313` currently calls
reload/filter. There is no demonstrated ordinary app mutation bypass.

Seal only invariant-bearing rows/query and provide borrowed accessors. Keep
selection/focus mutation used intentionally by the app unless a separate defect
is established. The app reads query at `:275`, `:378` and rows length at `:662`;
update those sites. Do not substitute `filter_map(get)` and silently drop stale
indices: that suppresses a panic without repairing the stale filter. Extend the
existing membership tests with shorter/reordered reload under a filter and
selected/focused stable IDs. A small compile-fail doctest can pin external field
privacy; no new test harness is needed.

### Menu zero step — iptv-core-003

`crates/iptv-core/src/menu.rs:103` really repeats the same nonfocusable row for
step zero. `Menu::new` allows all-disabled or heading-only rows. The app menu
calls in `crates/iptv-app/src/input.rs:350`/`:353` pass -1/+1, so classify this as
bounded public-API nontermination rather than a demonstrated remote-input hang.
The proposed early return is sufficient for this specific defect. Extend the
owning menu unit module for zero step on enabled, disabled-only, heading-only
and empty menus, preserving existing end-stop behavior. No constructor or
application navigation rewrite is required.

### Numeric settings — iptv-settings-001

`crates/iptv-settings/src/screen.rs:594` multiplies before saturating, and
`validate.rs:76`/`:84` assumes valid positive grids and narrow differences.
`screen.rs:523` through `:526` maps app keys to -1/+1/-10/+10; app touch controls
also supply bounded values. `tests/it/schema.rs:64` validates built-in grids.
Backups supply arbitrary values but use those fixed schemas. Thus huge `steps`
and malformed custom Setting schemas establish P2 API robustness, not an
ordinary malicious-backup crash with the current catalogue.

Saturating multiplication fixes current small catalogue grids. For arbitrary
full-width bounds, compute the complete step candidate and grid differences in
i128 before clamping/narrowing: saturating each intermediate can lose a valid
full-width result. Validate min <= max and step > 0 before clamp/modulo/division.
Select a narrow checked-grid/helper contract and handle malformed custom schemas
explicitly; do not turn every existing screen action/settings access into Result.
Update backup normalization at `backup.rs:366`, check validation and screen
callers together if the helper becomes fallible. Tests: extreme step counts on
the built-in editor; zero/negative step, reversed/full-width bounds and grid
alignment; ordinary backup clamping remains unchanged.

### Typed key construction — iptv-settings-002

`crates/iptv-settings/src/keys.rs:7` and its peers expose constructors, while
`store.rs:262`/`:267` presume catalogue-checked key types. Existing
`tests/it/store.rs:568` deliberately forges keys and asserts the panics; this
demonstrates the behavior was known but does not establish the advertised
external type guarantee. Keep this P2: current app/runtime callers construct
known valid literals, not user-controlled keys.

The suggested `pub(crate)` fields plus `as_str` are workable, but not sufficient
by themselves. Add catalogue-checked constants for valid ad hoc constructors in
runtime `parental.rs:93`/`:94`, `imports.rs:552`, and app `vod_view.rs:732`,
`grid.rs:97`, `grid/channel.rs:56`/`:105`, `browse_playback.rs:74`. That last site
selects between setting names, so select between typed constants instead.
Update `.0` readers in app settings/view and owning store; keep construction
inside the key module (or crate, if retaining private misuse tests). Convert the
current integration forged-key panic tests to private unit tests if still
useful, and use compile-fail docs for inaccessible external construction. Retain
the fallible string-key API for genuinely dynamic lookups. Do not make typed
reads fallible throughout the app solely to accommodate forging.

### Search time boundaries — iptv-search-002

`crates/iptv-search/src/programmes.rs:262`/`:263` subtract before conversion and
silently substitute shifted starts/zero duration when compression fails.
Query bounds (`:399`), archive threshold (`:421`), reconstruction (`:412`) and
distance (`:423`/`:427`) also require a consistent arithmetic policy.
`programmes_builder.rs:43` accepts raw signed endpoints. The runtime builder at
`crates/iptv-runtime/src/search.rs:25` reads stored times; the app search passes
current wall time and default windows. Existing EPG settling already rejects
per-channel spans over u32 at `iptv-epg/src/settle.rs:110`, but search uses one
global base across channels, so those per-channel checks do not prove global
compact representability.

The report's snippet is incomplete: checking offset/duration alone does not
validate reconstructed endpoint addition or ranking distance, and saturating
window bounds alone does not define negative look-back/look-ahead semantics.
Pick an explicit storage policy before changing callers: preserve full-width
times, or reject unrepresentable rows/builds through a typed documented result.
Never clamp a valid occurrence to a different date. If adding a build error,
update both `ProgrammeIndex::build` and `ProgrammeIndexBuilder::finish`, runtime
`search.rs`, app `search_view/programme_index.rs`, docs and the existing example;
preserve a useful prior index/error state rather than leaving the screen
permanently Building. A full-width storage solution can avoid that API churn,
subject to an explicit memory assessment.

Use widened/checked arithmetic for accepted intervals, exact distance ordering
(for example unsigned distance) and documented query-window handling. Extend
existing `programmes_tests.rs`/builder tests for endpoints, invalid intervals,
u32::MAX and span+1, cross-channel global spans, reconstruction near i64::MAX,
negative windows and ranking ties. Keep linear-scan equivalence and ordinary
round trips. This package is independent of search identity in principle, but
coordinate shared hit/builder changes to avoid two incompatible API migrations.

## Consider and retained design

The unnumbered timeline-span, empty settings-page and in-flight page-count notes
do not acquire P1 status from this review. The first two have bounded current
callers; MAX_PAGES explicitly caps loaded stores rather than all requests.
Any future queue-pressure work needs its own demand/failure evidence.

Keep local futures and `Rc`, portable state ownership, existing synthetic data
and existing test executable budgets. No blanket Send/Sync conversion, enum
non-exhaustiveness sweep, fallible-API sweep, dependency change or application
abstraction layer is justified by these eleven findings.

## What's already good and validation handoff

- Existing opaque schedule keys and combined-source qualification supply the
  search repair's identity source; reuse them.
- Runtime loaders hold the owner weakly, and current page tests exercise that
  lifetime property; retain it while fixing reply admission.
- Stored provider types, wizard values and authority snapshots already redact
  diagnostics; extend that established approach at the remaining raw leaves.
- Grid schema checks, search equivalence tests and focused navigation tests
  give useful behavioral coverage to extend without new test targets.

For implementation, run affected existing unit/integration scenarios and portable
wasm lanes, plus runtime/app caller compilation and the shared CI plan. Required
core tests/core wasm/app build and any affected Android lanes remain gates. The
baseline's missing wasm target, NSH exact-revision mismatch and player failures
must remain explicit; this source review does not resolve or waive them. Review
the combined fix plan again before implementation, especially page lifecycle,
key construction callers and atomic feed publication.

## Final consolidated-plan review

Reviewed the complete draft of
[the consolidated plan](../../../plans/2026-11-16-rust-review-consolidated-fix-plan.md)
on 2026-11-16 after it was written, rather than inferring its contents from the
crate reports. **Verdict: no blocking omissions in reviewed portable scope.**
This approves a future implementation plan; all defects remain open.

Every assigned ID is carried into an appropriate package:

| Package | Assigned IDs checked in the final draft | Final review |
|---|---|---|
| W2 | iptv-settings-001, iptv-search-002 | P2, checked/widened arithmetic and explicit invalid/unrepresentable-value policy; normal UI/calendar behavior retained. |
| W3 | iptv-library-002 | P1, durable identity preservation, real import/edit callers, persisted-data validation and atomic old-catalog retention included. |
| W5 | iptv-guide-002 | P1 merged with runtime-003 and runtime-004; exact owner/request incarnation, stale success/failure admission and retry recovery covered. |
| W6 | iptv-search-001 | P1, semantic channel/programme identity through state, builders, app choose/outcome and multiview pick; final current-schedule resolution included. |
| W10 | iptv-core-001, iptv-library-001, iptv-guide-001 | P2 formatting confidentiality, nested marker assertions and raw input/header boundaries retained without unsupported claims of logged disclosure. |
| W11 | iptv-core-002, iptv-core-003, iptv-settings-002 | P2 invariant/privacy changes with real caller migration, zero-step handling and bounded regression coverage. |

W4 before W5 supplies coherent page reads; W5 then addresses asynchronous
acceptance independently of database consistency. W2 precedes W6, so changes to
the shared search builder/time contract can be integrated before migrating hit
identity. W10's joint core/guide redaction scope accounts for transitive
Resolution formatting. W11 explicitly migrates ad hoc typed-key construction
before hiding fields. The plan avoids blanket fallibility, Send/Sync changes,
runtime replacement and unnecessary source renumbering.

The detailed implementation cautions above remain part of this review handoff:
in W2, widen the complete full-width step/grid computation rather than assuming
separately saturated intermediates preserve it; in W5, attempt identity must
gate failure completion as well as successful insertion; in W10, redact the
separately public AddName/EditKind leaf as well as containing Editing/Screen;
in W11, privacy regressions use compile-fail docs or private misuse tests rather
than leaving the existing external forged-key tests uncompilable. These refine
the named package contracts and do not require another package or broader API.

The plan preserves existing harness budgets, separate doctests, portable wasm
checks, required core/app checks and affected native/Android gates. It records
the actual baseline limitations and states that host simulation does not prove
device or decoder behavior. Neither the first-pass file coverage nor this plan
review represents an exhaustive audit of every test body or an executed device
validation. No Cargo commands, source edits or new tests were performed during
this final-plan review.
