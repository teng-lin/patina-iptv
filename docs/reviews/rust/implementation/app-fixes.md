# App implementation review and corrections

Reviewed W9's internal application integration under `rust-review` and
`rust-best-practices`, edition 2024/Rust 1.99.1. Source review covers the artwork
receiver, assignment/repair callback ownership, selected descriptions and shared
Stop/sleep handling. Root owns coordinated Cargo runs; no Cargo invocation or
commit was made by this reviewer. All fixtures and marker strings are synthetic.

## Material correction: pending repair additions

`guide_repair_view::add` still performed inspect_channel with an uncaptured
control, then notified or created/mutated screen state after any intervening
close, history request or replacement. That was another route to app-003's
stale-screen behavior.

The correction attaches the inspection to the existing repair cancellation
lifetime, captures the originating inspection's token, and checks both before
success or error effects. Concurrent selections on the same current screen can
share the token; a busy prior repair operation is canceled first. Close, a new
operation and Clear fence old additions. Clear now explicitly cancels before
resetting Screen, covering the generation-reset ABA path without a new general
screen-instance facility.

All preview/history/apply/status/verify/undo state gates share the same private
acceptance predicate: live captured cancellation, open screen and matching
operation generation. Completion uses the same predicate before clearing busy
or setting an error. The regression deliberately resets generation on another
source and verifies that an old canceled completion cannot finish or set an
error on that new operation.

Stale successful previews continue to be released through the runtime owner
captured before the await. No asynchronous stale-plan cleanup uses a later
ambient owner. Existing `spawn_in_app` runtime-owner polling remains intact.

## Other W9 conclusions

- The cache-hit fix sends the oneshot observed by the view. Its permit-handoff
  test covers the queued cache hit; an added cancellation case verifies that a
  dropped receiver returns None after permit release rather than publishing the
  cached response. It does not assert a new general single-flight feature.
- Assignment's saved REQUEST ticket gates notice and close. A new integration
  interleaving holds the actual writer, accepts a guide choice, closes and
  reopens the picker, releases the writer, acknowledges the durable disabled
  guide choice through SQLite, and asserts the newer picker stays open.
- Header and Details share the real description lookup. Unit cases cover two
  combined sources with duplicate local keys and different epochs, a single
  source, missing description and invalid channel. The existing guide UI
  scenario now opens Details and asserts its generated programme description.
- Stop and sleep route through the same complete stop, clearing live/VOD/
  recording/early source state, pending zap, browse intent and resume admission.
  Existing close cancels resolution, clears native publication, stops health and
  releases viewing/recording capabilities. Native views stay mounted.
- `early` checks the monotonic resume=false fence before and after the stored
  row await, and excludes VOD. Stop cannot re-enable it. This source interleaving
  is correct; no new lifetime counter is required.

## Regression changes and coordinated validation

Private unit tests were added/extended in existing source modules:

- `content_image::tests::a_queued_cache_hit_replies_to_the_waiting_view`
- `content_image::tests::a_cancelled_queued_image_does_not_take_the_cached_reply`
- `guide_repair_view::callback_regressions::a_cancelled_callback_cannot_finish_a_reopened_screen_at_the_same_generation`
- `header::description_regressions::descriptions_follow_selected_source_programme_and_empty_data`

Existing integration modules were extended, with no new test executable:

- `cov_sources_assign::choosing_a_channels_guide_data`: acknowledged old save
  against a reopened picker.
- `guide_look::the_guide_uses_the_bundled_fonts_and_proportions`: selected Details
  text comes from EPG, not the old fixture wording.
- The three VOD discovery/resume scenarios now end with remote Stop on TV/desktop
  or an actually fired sleep timer on tablet; all assert no native source, the
  same persistent native node and final 67-second VOD progress in SQLite.
- `recordings_transport::saved_recording_resumes_seeks_paused_and_starts_again_without_changing_surfaces`
  additionally asserts Stop clears the source and retains the exact native node.

The existing advanced-control navigation helper is shared by the tablet sleep
case; the timer advances through the engine clock, rather than sleeping fifteen
wall-clock minutes. Direct rustfmt and diff whitespace checks passed. Root must
compile and run these scenarios with app/runtime test-support before treating
these cases as verified; source review alone does not establish their success.

The precise early-resume row-read/Stop barrier and every separate repair callback
family have not each received a new end-to-end scenario here. The common
acceptance predicate has an ABA/error-completion regression, existing foundation
repair journeys exercise apply/verify/undo, and the final suite must retain
those journeys. A dedicated reader-lane acknowledgement hook would be needed to
make an early-row-read race deterministic through the public app harness.

## Native recording pause evidence added separately

The existing player unit harness now contains
`mpv::tests::a_recording_resumes_by_range_after_its_paused_writer_expires` for W12.
It generates a padded MPEG-TS on disk, bounds native packet-cache bytes, waits
for an owner-scoped guarded downstream relay write-timeout observation, then requires a fresh
range request, resumed native timeline/rendering and the same playlist entry.
The network reviewer added owner-scoped test-support observations, so elapsed
pause time or prefetched bytes cannot make the check vacuous. Root runs that
native test separately; it is not implied by the app headless checks above.

## Adjacent W5 pending archive replacement correction

Confirmed that a ScheduleReady event can synchronously finish a pending archive waiter before the zero-delay follow_playing task remaps the old playing row. When the replacement has the same row count, set_schedule does not remap that row synchronously. Its new epoch can also give the old catch-up minute a different UTC meaning.

The pending request now captures content key, absolute UTC start, offset and available source authority before waiting. Successful completion finds that key in the current schedule, verifies the original source/incarnation/configuration when authority exists, converts the UTC start into the new epoch and synchronously updates the target before retrying the same health session. Missing/recreated identities or unrepresentable time fail through the existing typed guide-error path. A changed catch-up selection in the same session cannot be overwritten by an older completion. Persistent video surfaces and session start semantics remain unchanged.

The owning unit harness has a deterministic same-size two-channel reorder plus one-day epoch change, preserving key/UTC/offset at the remapped row, and rejects matching content IDs from a recreated source or removed channel. This is the pending callback's shared remapping boundary, rather than a timer/sleep race. Central compile/test is pending.

Root's final guarded native suite has since passed all 66 cases, including the corrected real decoder pause/range-resume scenario. See native-lifecycle.md for the measured pending-control child result. This result is separate from the app headless suite.
