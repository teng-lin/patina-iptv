# Search result identity implementation (W6)

Implementation review of the stable search result patch found one remaining
identity alias: SQLite can reuse a deleted source's numeric ID while the new
source exposes the same provider channel key. A source connection edit also
changes the authority of the same content key. Root approved the incarnation
fence and the two read-only SourceAuthority accessors before implementation.
This agent has not run Cargo or committed; validation remains centrally owned.

## Accepting identity

`ChannelId` now compares the source incarnation as well as numeric source and
content key. `ChannelId::in_source` constructs stored identities;
`ChannelId::new` retains unscoped portable/synthetic identities. The incarnation
comes from the immutable SourceAuthority captured by the accepting schedule,
without database I/O on the UI thread. Combined schedules use each child's
authority and local key. Missing or mismatched stored authority fails closed.

Ordinary source refreshes retain their incarnation and therefore retain focus.
Store source deletion/recreation or a connection/configuration/credential edit
changes it. The existing store rotates the incarnation when source kind,
configuration or secrets change; a mere display-name edit does not.

The in-memory programme fallback explicitly installs the same qualified
identities as channel search. Native programme reads capture the accepting
incarnation and check it before reading titles, with all checks and title reads
inside one WAL snapshot. Work cannot adopt a recreated numeric source while it
waits to run. Combined offsets affect positions only, never result identity.

## Consumers and resource behavior

Normal playback, details, multiview selection and stale row rendering resolve
the owned identity against the current schedule immediately before using a row.
Removed content or another source incarnation cannot be selected accidentally.
Programme visibility, archive windows and favorite ranking resolve current rows
before filtering and limiting. Removed rows therefore cannot consume the
limited result set, and reordered archive windows apply to the intended content.

Channel ranking now retains only positions and scores until after truncation;
only returned hits clone their owned identity. Programme identities remain one
per channel rather than one per indexed occurrence. Heap accounting includes
the incarnation strings. No platform dependency, unsafe code or manifest change
was introduced in the portable search crate.

Native build errors and missing authority now complete the callback with an
error. The current build flag is cleared, no empty successful index is installed,
and a later reopen/refresh can retry. The UI uses its existing unavailable-guide
state and logs the runtime error. Old-version completions cannot clear the
current build. This also prevents the new incarnation fence from leaving search
permanently marked as building after a rejected queued read.

## Added regression coverage

- Runtime identity tests cover an ordinary reorder/refresh, content removal,
  switching sources, duplicate provider keys in combined sources, combined
  offset changes and recreated/configuration-edited source authority.
- A real encrypted local-store runtime test deletes/recreates the source with
  the same numeric ID, then edits its configuration. Reads carrying each prior
  incarnation fail; reads carrying the current one succeed.
- Programme tests share one compressed title across two source identities and
  prove post-build fallback qualification changes only the intended channel.
- Runtime programme tests apply current visibility/archive windows after a
  reorder and remove stale content before the result limit.
- SearchState tests preserve programme focus across same-incarnation reorders
  and fall back when the same provider key belongs to another incarnation.
- App tests exercise the identity resolver shared by normal playback and
  multiview for both channel and programme result IDs, including combined
  offset changes and source recreation.
- Build-state tests verify failure completion permits retry and a stale-version
  completion does not clear a newer in-flight build.

All fixtures use generated playlists, reserved example domains and local stores.
Tests remain in their existing owning modules/harnesses.

## Files changed by this follow-up

`iptv-search/src/{lib,channels,programmes,programmes_builder,state_tests}.rs`,
`iptv-runtime/src/search.rs`, `iptv-guide/src/playback.rs` (two accessors only),
and `iptv-app/src/search_view.rs` plus
`search_view/{programme_index,pick,view}.rs`. Earlier root changes in these files
are retained. The review artifact is this file. Root was notified when source
edits became stable, including the missing Schedule trait import discovered by
its central compilation.

The remaining portable API caveat is explicit: `ChannelIndex::from_names` has
only names available as identities. Callers needing distinct same-name content
must supply stable identities with `set_identity`; production schedule builders
do so. No claim of passed validation is made by this implementation review.
