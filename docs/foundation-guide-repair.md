# Guide repair foundation validation

The repair boundary is the declared [internal application service contract v1](runtime-api-v1.md).
It uses portable `iptv_library::guide_repair` values and explicit-owner runtime
methods. There is no MCP transport, daemon, embedded agent, recording scheduler
or new player controller in this stage.

The [acceptance record](plans/2026-11-14-foundation-validation.md) identifies
the integrated source, platform results and limits.

## Product entry points

Open Playlists → a playlist → Guide sources → Inspect guide coverage. Select a
playlist channel, compare a configured alternative's programme samples or inspect
an XMLTV link, then choose Add this guide choice. Automatic and No guide data are
also explicit repair targets. Keep selecting channels accumulates one batch;
Reuse inspected guide reuses the exact private snapshot for another channel.

Preview shows selected before/after choices, individual replacement permissions,
feed additions/settings/precedence and indirect effects. An unknown programme
availability is displayed as unverified; a metadata candidate is not certified
as the effective binding. Apply persists the batch atomically. Check repair
status, Verify guide changes and Undo guide changes remain distinct actions.
Recent guide repairs discovers eligible durable records after ordinary restart.

The editor preserves content-based focus and scrolls to keyboard/remote focus.
Removing or replacing the last selection for a staged addition removes that
unneeded addition from the draft. Closing/editing releases its obsolete preview.

## Reproducible checks

All fixtures use generated channels, guides, opaque database keys and local mock
HTTP servers. These commands exercise the domain without a view or player:

```sh
cargo test -p iptv-runtime --features test-support guide_repair --lib
cargo test -p iptv-runtime --features test-support --test foundation_repair_fences
cargo test -p iptv-runtime --test foundation_contract
cargo test -p iptv-store-sqlite guide_repair --lib
cargo test -p iptv-store-sqlite restore_preserves_stale_transport --lib
cargo test -p iptv-library guide_repair
```

The existing UI uses that same boundary:

```sh
cargo test -p iptv-app --features test-support --test app foundation_repair
```

Development evidence uses an isolated build cache and the local NSH checkout;
the pinned full `tools/check.sh` gate and native Mac/iPad simulator checks are
recorded separately by the integration owner. A headless UI scenario alone does
not prove physical decoder or Keychain behavior.

| Acceptance | Concrete regression or evidence |
|---|---|
| FND-14 | `staged::evicted_stage_cannot_be_applied_and_never_adds_a_live_feed`; `changed_transport_or_disabled_policy_or_new_block_refuses_a_prepared_edit`; store `replaced_feed_identity_and_deleted_channels_are_rejected` |
| FND-15 | `bulk_500_preview_paginates_and_one_affected_conflict_prevents_every_change`; `staged::one_stage_repairs_500_channels_with_one_bounded_evidence_batch` |
| FND-16 | `no_op_refresh_apply_and_unrelated_edit_undo_preserve_identity`; store `no_op_import_and_unrelated_edit_do_not_invalidate_apply_or_undo` |
| FND-17 | `foundation_repair_fences` holds an actual old refresh report and an actual old HTTP guide download across a repair, then checks stored and effective programmes |
| FND-18 | Saved outcomes remain pending until observed publication/verification; actual failed-refresh/recovery regression belongs to `foundation_repair_fences` |
| FND-19 | `late_verification_cannot_certify_a_later_edit_and_retry_keeps_original_revision`; `held_verification_cannot_certify_later_programmes_with_unchanged_mapping_and_configuration`; no-op inverse and late-cancellation regressions |
| FND-20 | Store `conflicting_undo_never_reverts_a_subset`, `no_op_import_and_unrelated_edit_do_not_invalidate_apply_or_undo`, `undo_restores_removed_feed_positions_and_preserves_unrelated_new_feed` |
| FND-21 | Runtime `closing_owner_expires_previews_but_committed_inverse_survives_restart`, `durable_inverse_receipt_reconciles_a_stale_original_operation_cache`; store `durable_inverse_survives_restart_but_restore_invalidates_authority`, `expired_inverse_is_explicit_and_duplicate_commit_does_not_reapply`; provenance restore regression |
| FND-22 | Runtime `headless_preview_apply_verify_undo_uses_real_domain_services`; app `foundation_repair` performs two-channel comparison, shared probe reuse, preview, apply, actual binding refresh, verification and undo |
| FND-23 | `repair_analysis_on_100k_channels_and_1k_groups_keeps_display_lane_responsive`; held-analysis and queued/inflight-writer deadline tests; preview and operation capacity regressions; existing unchanged guide-input/store-latency gates |
| FND-24 | App `foundation_repair` observes the persistent primary surface, player commands and selected content while repair operates on another playlist; native Mac/iPad simulator validation is a separate platform gate |
| FND-25 | `foundation_contract` names only the declared runtime facade and portable values; the app builds against it |
| FND-26 | `foundation_contract` exact signature constructions and exhaustive error/status matches; `foundation_capabilities` distinguishes implementation from provider/host readiness and unimplemented future work |

The generated 100k-channel/1k-group repair case also ran alone in a native test
process: 5.408 seconds wall time and 163,968 KiB peak process RSS on the Linux
development host. This includes fixture/catalogue construction, database lanes
and analysis; it is not an estimate of incremental analysis allocation. Display
reads retain the existing 50 ms assertion. Timing is machine-dependent, while
row/text/staging/admission limits are enforced explicitly.

## Recovery semantics

A received durable receipt identifies the exact committed revisions. A lost
reply after the writer begins can return `CompletionUnknown` with the operation
identity; querying history/status or retrying the same plan identity reconciles
it without applying a second mutation. Status compares the exact durable
commit and inverse direction before using a cached observation, including
when an undo committed after a lost reply. Read-only recovery does not promise to
restart publication work that never ran. The effective result remains pending;
request the existing source refresh and verify again. That existing native
refresh entry point remains experimental outside the declared v1 method set.

Undo records survive ordinary restart for seven days and compare all affected
rows before making any change. Unrelated edits survive. Restore rotates source
and feed authority and removes old inverse authority. Only cache provenance
that was authoritative before restore is retagged; matching a URL/shift alone
cannot certify data fetched with superseded transport credentials or headers.

Programme coverage never establishes that a provider actually broadcasts the
intended channel. Verification reports identity as unresolved, the actual
programme window, exact observed revisions and evidence digests. Capabilities
remain honest about catch-up provider verification and deferred product work.
