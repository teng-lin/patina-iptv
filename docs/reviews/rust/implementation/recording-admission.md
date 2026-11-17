# Recording admission and final playback authority (W7)

Implemented runtime-002/runtime-006 in the existing recording/VOD paths.

The runtime captures the source incarnation before its parental/read/provider waits and captures a parental snapshot immediately after a successful parental check. Recording schedule, edit, first recurring occurrence and re-enable use narrow checked Store methods. Each method acquires `BEGIN IMMEDIATE`, then invokes `authorize(&Store)` against that same connection before channel admission, duplicate return or durable mutation. The callback rechecks the accepting application owner, shutdown state, original source incarnation, current policy revision and any required current grant. SQL and connection types remain within the Store adapter.

Raw Store entry points continue to reject legacy blocked channels, including duplicate schedule, edit and re-enable. Checked runtime entry points can admit a genuinely unlocked legacy block. Re-enable also requires the stored rule's original incarnation to remain current. Materializing an already accepted rule retains its accepted authority through a later viewing relock, while still requiring an enabled rule, original source incarnation, existing channel and enabled source. Stop, cancellation and disabling remain available without a new PIN.

Removed the runtime's read-only duplicate fast return. Idempotency remains in the accepting transaction, after authorization.

`vod_play` now checks the current owner and movie/episode plus parent grant synchronously after its final Store await. `recording_file` checks after its filesystem worker result; `open_recording` checks again after relay creation and before retaining its playback lease. A rejected relay remains local and is dropped. Test-only acknowledged completion holds exercise those precise final-return boundaries without changing production protocol or timers.

Added regressions in the existing owning unit harnesses:

- Store: checked callbacks observe a live transaction; rejection cannot return an old duplicate ID, edit a job, create a rule or re-enable one.
- Store: raw legacy-block commands fail closed; checked admission succeeds; accepted recurrence continues through viewing protection; source replacement cannot re-enable old intent.
- Runtime: queued duplicate/edit/repeat/re-enable commands all reject a relocked captured grant behind an acknowledged real writer hold.
- Runtime: captured admission cannot follow a deleted/recreated source with the same numeric ID.
- Runtime: file and relay capabilities reject relock after their final service completion; an unaccepted relay does not acquire the playback lease.
- Runtime: direct movie and episode playback reject relock after their final Store read, including the series-parent grant.

Validation: direct rustfmt and scoped `git diff --check` passed. Cargo is centralized by the root agent; these new cases have not yet been compiled or run by this agent. Suggested filters: `recordings::tests`, `playback_rechecks_movie_and_episode_parent_grants_after_the_final_read`, plus the existing runtime recovery, recurring scheduling and VOD source-change suites. No new executable or manifest change was added.

A later combined runtime run passed 241 cases and exposed an acknowledgement gap in `shared_tests::rejected_reused_viewer_releases_only_its_own_capture_input_reference`: terminal storage status and the mock upstream socket reaching zero did not establish that the capture task had released its lease and `retire_unused` had removed the registry entry. Closing entries deliberately remain budgeted until shutdown is joined. The test now observes actual capture lease release and awaits the existing bounded `retire_viewing_inputs(1)` retirement/join before asserting empty accounting. The capture-retention, single-request and peak-one-connection assertions remain. No production behavior changed; central revalidation is pending.

## Capture consumes accepted intent

Claude's implementation review found two remaining viewing-overlay checks at capture resolution: the initial stored channel read rejected `blocked`, and the post-provider-await comparison rejected it again. Those checks contradicted checked admission and durable recurrence authority, producing accepted jobs that could never capture an unlocked legacy-blocked channel.

Private capture resolution only consumes Store-loaded jobs that passed durable admission and conditional claim. Raw admission still rejects viewing overlays; checked admission proves its grant within the accepting writer transaction. Capture therefore consumes the durable job's authority, rather than requesting a fresh viewing grant. Both obsolete overlay tests were removed. Enabled-source checks, original job/source incarnation, channel existence and exact channel/configuration stability across provider I/O remain intact. Newly scheduling/re-enabling/editing intent still requires current admission; playback of retained media still requires its own current viewing grant.

Two existing-harness runtime scenarios now capture actual synthetic TS bytes and finalize completed recordings: a public checked recording accepted for an unlocked legacy block then relocked, and a publicly accepted series whose new current guide occurrence is discovered/materialized after a later block and relock. Both verify new viewing/playback remains denied; the series also stops without a new PIN. Central execution pending for these additional cases.

The existing acknowledged delayed Stalker-link test now explicitly expects a late viewing block to preserve accepted recording resolution while disabled source, removed member and changed stream still reject. This retains provider-await revalidation coverage under the corrected durable-intent semantics.
