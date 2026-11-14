# Source and guide inspection

Patina's guide-source screen offers **Inspect guide coverage**. Select a
playlist in Playlists, open Guide sources, then inspect its channels. This
works while another playlist is on screen. A read-only inspection does not
select a playlist, move guide focus, or issue player commands.

The inspection screen shows coverage over its displayed interval. Select a
channel to compare guide candidates and see a short sample of the currently
stored programmes. Selecting a candidate downloads a bounded inspection
snapshot and displays programme samples, including entries omitted from normal
imports because no playlist channel matched them. The snapshot is reused for
later samples; it is not silently replaced by another download.

**Inspect a guide link** inspects a proposed public HTTP(S) XMLTV feed before
saving it. Existing configured feeds retain their source-specific network
policy. New public links cannot contain URL username/password credentials or
access private destinations. Both paths use the same redirect, DNS and
cross-origin transport protections as ordinary imports. Synthetic tests use
the existing explicit debug fixture policy.

Search filters channels/candidates; Next continues a bounded semantic page.
Remote navigation and pointer/touch use the same content identities. Back
first ends typing, then returns through samples, candidates and coverage.
Closing an inspection cancels its request and rejects late replies.

## Interpreting the results

Coverage is measured over a half-open UTC interval. It distinguishes source
failure, guide deliberately disabled, an unresolved saved choice, no match,
ambiguous names, empty/stale/partial schedules and suspicious timing. Fresh
programme data is evidence that a feed works; it does not establish that it
belongs to the intended station. Regional and `+1` labels remain distinct.

Candidate evidence reports exact/case-insensitive IDs, manual choice and
normalized names separately. Existing automatic matching still uses its
established precedence, including its document-order fallback for ambiguous
names. Inspection lists the alternatives rather than treating the first one
as a verified identity.

The persisted guide index currently retains the first display name. A probe
retains all channel labels supplied by its parser, within its documented
limits. Structured country/language metadata is not invented when the source
adapter does not provide it. Programme references retain channel/feed
provenance, observed intervals and semantic evidence revision. Provider event
IDs are optional and remain absent when the source adapter supplies none.

Cached evidence belongs to the feed incarnation that produced it. Reusing a
numeric feed ID, changing its configuration or restoring a different catalogue
cannot revive a stale candidate. A retained guide from an older configuration
is reported as stale rather than labelled as current coverage.

## Runtime contract and limits

The UI and independent native test consumer use the same explicit-owner
runtime operations:

- `inspect_channel`, `query_channels` and `query_programmes`;
- `epg_audit` and `epg_candidates`;
- `probe_feed`, `probe_candidates` and `probe_schedule`.

The portable input/result types live in `iptv_library::inspection`. Channels
are identified by `(SourceId, ContentId)` within their owning catalogue, never
by view index or replacement row ID. Semantic cursors bind owner epoch, scope,
filter and relevant data. A no-op physical reimport preserves continuation;
changed inputs or incompatible data produce an explicit stale result.

Queries use a separate lazy query-only analysis connection. Its SQLite
instruction handler enforces cancellation/deadlines without putting scans on
the guide display reader. There are at most 16 admitted analysis requests per
owner, a three-second maximum query deadline, at most 64 enabled selected
sources, 200 results per page, 256 input query bytes and a 31-day maximum
programme window. Successful empty results, unavailable source, denied
content, failed storage and expired evidence are different outcomes. Inspection
excludes blocked content; hiding is a separate presentation filter. Complete
PIN/access-control workflows remain in the application feature roadmap.

Probes use a 60-second maximum end-to-end deadline, three redirects, 32 MiB
input and 256 MiB expanded XML. XML records are bounded to 1 MiB, nesting to
32 elements, total parsed items to one million and listed channels to 100,000.
Channel IDs/names are limited to 4 KiB and 16 labels per channel; the temporary
channel index has a 64 MiB budget. Schedule samples use the shared programme
settlement rules. More than 200 resulting samples requires a narrower interval
and returns a limit result instead of an apparently complete truncated list.

A probe retains the exact compressed/raw input and a bounded channel index in
private encrypted on-disk staging; it does not materialize a complete live
programme store. Each owner epoch gets a fresh random disposable secret key.
The profile staging budget is 512 MiB including in-flight reservations, with a
physical 128 MiB SQLite page ceiling per stage. Stages expire after 24 hours.
A lease pins a snapshot during validation; bulk validation checks selected
candidates with one encrypted open and integrity scan under its deadline.
Unreferenced stages may be evicted. Stale crash artifacts are removed only
after obtaining their retired owner's lock. Active snapshots and leases never
become authoritative live configuration merely because parsing succeeded.

The inspection contract is provisional until the second foundation PR passes
its internal API-v1 compatibility gate. Transactional preview/apply/verify/undo
and the complete repair UI are delivered with that second PR. MCP and the
optional playback/DVR/timeshift/VOD/PIN feature work have their own later gates.
