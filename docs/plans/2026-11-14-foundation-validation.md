# Foundation validation record — 2026-11-14

**Draft; final integration closure pending.** The earlier validated feature source is
[`6f5829c597de9f24e6b2fde7e7b134157ec828d6`](https://github.com/teng-lin/patina-iptv/commit/6f5829c597de9f24e6b2fde7e7b134157ec828d6).
This records executed evidence separately from pending gates. The internal API
remains a **candidate** until the integration owner closes the host, CI and
review rows below; this document does not declare Stage 1 complete.

The requirements are the [FND-01–26 acceptance inventory](2026-11-14-foundation-acceptance.md)
and [delivery plan](2026-11-14-mcp-plan.md). Semantics and compatibility are in
[the internal application service contract v1](../runtime-api-v1.md). Product
entry points and detailed checks are documented in
[source and guide inspection](../foundation-inspection.md) and
[guide repair foundation validation](../foundation-guide-repair.md).

All evidence uses generated playlists, XMLTV and media, local mock servers,
and synthetic credentials/keys. No real provider compatibility is inferred.

## Executed host evidence and pending gates

| Gate | Source | Recorded result and limits |
|---|---|---|
| Pinned Linux full repository gate | `6f5829c` | **PASS.** Complete `tools/check.sh`, required native-player path and strict linters enabled: core/native tests, nine portable wasm builds, app build, 113 app unit tests, 108 app scenarios, both unchanged latency gates and CI/helper regressions. NSH uses the repository pin. |
| Direct Mac native checks | `6f5829c` | **PASS, 27 checks.** Includes 13 probe tests, 15 inspection tests, default-feature `--tests` compilation, independent contract consumer, 25 repair tests, nine atomic-store repair tests, three fence/recovery tests, generated 100k scale cases, all portable wasm builds, app units, playback scenarios, latency checks and strict Clippy. This is native application/test evidence, not a universal decoder or provider certification. |
| iPad simulator repair workflow | `6f5829c` | **PASS, 51.8 seconds.** Fresh isolated simulator target; two-channel inspect/preview/apply/verify/undo, settled inverse verification and status. Synthetic injected key only. No Keychain, physical iPad or real decoder claim. Exact state oracle is below. |
| Android API 36 tablet and TV smoke plus live inspection | `9c36e46` | **PASS on both emulator targets.** Stable layer IDs/focus; seven observed opens were accounted for by generated fixture endings; zero unexplained opens/errors. This earlier-source evidence does not replace the final repair-workflow gate. |
| Android live repair UI | `6f5829c` | **PASS for the tablet repair workflow.** Exact CI APK: No guide preview/apply/verify, undo and restored coverage; focused channel and primary SurfaceView/BLAST identities remained unchanged. Eleven opens were accounted for by finite generated fixture endings; zero unexplained opens, errors or crashes. This run exposed a viewport reveal defect, fixed separately in `33e5710`; final visible keyboard-navigation evidence remains pending. |
| PR 55 CI and review | `386631f` | **PASS; merged as `d2da140`.** All 21 checks passed after retrying the tablet smoke. Review threads were fixed and resolved. The initial tablet failure was a smoke-driver race: it sent navigation before the asynchronous menu close and could mistake an EOF reconnect for a channel change. `33e5710` adds causal menu-close and guide-key barriers without increasing timeouts. |
| PR 57 CI and review | `6f5829c` | **PENDING closure.** Seventeen checks passed; Android tablet/TV smoke checks remain pending. Required CI and review of the final retargeted PR must be closed before declaring v1 stable. |

After merging PR 55 and the native initialization baseline (`d2da140`),
`0e5d94f` passed all 29 native Mac checks against the new checksum-verified
runtime, including the cold first-probe initialization test. Linux's first
full run stopped in the generated 100k-channel fixture: syscall tracing showed
`pwrite64` returning `EDQUOT` on `/tmp` (a tmpfs with user quotas). The unchanged
isolated scale test passed after assigning a private scratch directory under
`/home/blackmyth/tmp`. This is a host scratch-space correction, not a relaxed
assertion. Final checks must also cover the subsequent peer-review fixes for
automatic projection freshness and feed-ID allocation.

The simulator measured durable and effective state separately:

| Checkpoint | Saved overrides | Effective bindings | Additional observation |
|---|---:|---:|---|
| Before / preview | 0 | 0 | Preview did not mutate live state. |
| Apply / verify | 2 | 2 | Exact selected generated channel IDs; viewer identity unchanged. |
| Undo saved | 0 | 2 | The inverse persisted before asynchronous guide replacement completed. |
| Undo settled | 0 | 0 | Old effective bindings were removed after refresh. |
| Inverse verify / status | 0 | 0 | Coverage 0 of 2 with an `Undone` receipt; viewer identity unchanged. |

The earlier `994be71` simulator run proved saved choices, verification and saved
undo, but did not establish the settled-zero-binding checkpoint. The fresh
`6f5829c` run above supplies that oracle. It still does not test real Keychain
access, a physical iPad or hardware video decoding.

## Evidence map for FND-01–26

The original evidence map below records tests present at `6f5829c`; later integration checks also cover the `33e5710` viewport/harness fixes and native initialization after master `d2da140`. The complete pinned Linux gate executes
the relevant native/app suites; the Mac checks execute the stated subset above.
Host-specific acceptance remains subject to the explicit pending rows.

Test-file abbreviations keep the map readable:

- **I:** [native inspection consumer](../../crates/iptv-runtime/tests/foundation_inspection.rs).
- **P:** [native probe consumer](../../crates/iptv-runtime/tests/foundation_probe.rs).
- **R:** [runtime repair regressions](../../crates/iptv-runtime/src/guide_repair/tests.rs).
- **S:** [atomic store repair regressions](../../crates/iptv-store-sqlite/src/guide_repair.rs).
- **J:** [incarnation/revision/restore regressions](../../crates/iptv-store-sqlite/src/guide_revisions.rs).
- **F:** [native delayed-work/failure/recovery consumer](../../crates/iptv-runtime/tests/foundation_repair_fences.rs).
- **UI-I:** [existing inspection UI workflow](../../crates/iptv-app/tests/app/foundation_inspection.rs).
- **UI-R:** [existing repair UI during playback](../../crates/iptv-app/tests/app/foundation_repair.rs).
- **C:** [independent contract consumer](../../crates/iptv-runtime/tests/foundation_contract.rs).

| Scenario | Executed evidence and observable oracle |
|---|---|
| FND-01: explicit source scope while another source is shown | I `source_queries_leave_shown_schedule_browser_and_player_events_untouched`; UI-I and UI-R inspect/repair the other playlist while retaining the viewer identity and player commands. |
| FND-02: equal IDs and enabled-source scope | I `equal_content_ids_remain_scoped_and_disabled_sources_are_explicit`, `a_channel_filter_with_multiple_sources_uses_the_complete_identity`. |
| FND-03: empty versus missing/disabled/failed reads | I `empty_missing_failed_and_invalid_reads_are_distinct`; disabled-source and blocked-access cases remain typed failures. |
| FND-04: physical no-op reimport between pages | I `semantic_channel_pages_survive_physical_reimport_and_reordering`; semantic identities/cursors remain valid despite physical row replacement. |
| FND-05: cursor context, ordering and bounds | I `cursors_refuse_other_owner_scope_filter_window_and_changed_catalogue`, `audit_cursor_cannot_mix_different_guide_snapshots`; P checks staged cursor context; UI-I exercises more than one candidate page and return from samples. |
| FND-06: independent owners, storage and keys | [Ownership suite](../../crates/iptv-runtime/tests/ownership.rs), especially `a_request_keeps_its_accepting_directory_and_key_broker_on_another_thread` and `constructing_and_starting_another_runtime_does_not_change_an_open_owners_store`; P crash-cache cleanup preserves an active owner. |
| FND-07: held query, cancellation, retirement and restore | I `pre_cancelled_and_expired_queries_do_not_poison_the_reader`, `closing_an_owner_with_a_held_open_rejects_its_late_query`, `completing_one_query_does_not_cancel_a_shared_callers_control`; P shutdown cancellation; R owner restart; J restore authority rotation. |
| FND-08: hidden content versus blocked access | I `hidden_content_and_blocked_access_have_different_query_semantics`; R blocked-known-ID preview/apply and current disabled-source verification regressions. Full parental PIN/unlock remains deferred. |
| FND-09: diagnoses and interval coverage | I `audit_distinguishes_coverage_partial_stale_unmatched_disabled_and_input_failure`, `overlapping_stored_programmes_count_separately_from_union_coverage`; P missing metadata/feed failure and audit invalidation. Coverage does not certify station identity. |
| FND-10: alternatives, ambiguous names and shifted editions | I `candidates_keep_ambiguous_and_timeshift_alternatives_and_half_open_programmes`; UI-I compares unmatched alternatives while saved choices remain unchanged. |
| FND-11: listed but unmatched staged programme samples | P `unmatched_candidates_have_samples_from_the_exact_encrypted_snapshot_without_live_mutation`; `prewindow_successors_settle_missing_stops_before_filtering_in_any_document_order` includes 300 older rows and reordered input. |
| FND-12: network/parser/resource failures and cleanup | P cancellation/deadline, malformed XML/entities, three-redirect/download budgets, cross-origin private-address refusal, supplied credential isolation, crash cleanup, shared 16-read admission and two-feed admission tests; [strict XML tests](../../crates/iptv-epg/src/strict.rs) include compressed/chunk/expanded-record limits. |
| FND-13: UTC, offsets and time-shift provenance | [EPG time tests](../../crates/iptv-epg/src/time.rs) `full_form_with_offsets` and `missing_offset_uses_policy_and_shift_applies_on_top`; I half-open/shifted alternatives; P configured shift applied once to staged samples. Display formatting is separate from stored Unix instants. |
| FND-14: expired/evicted or superseded evidence | R `staged::evicted_stage_cannot_be_applied_and_never_adds_a_live_feed`, `bad_digest_expired_plan_and_blocked_known_id_make_no_changes`; P changed-config/forged-evidence/feed-reuse tests; S reused feed identity refusal. |
| FND-15: atomic bulk conflict and staged feed allocation | R `bulk_500_preview_paginates_and_one_affected_conflict_prevents_every_change`, `staged::one_stage_repairs_500_channels_with_one_bounded_evidence_batch`; S explicit-choice protection, addition allocation and conflict tests. |
| FND-16: no-op refresh preserves valid repair authority | R `no_op_refresh_apply_and_unrelated_edit_undo_preserve_identity`; S `no_op_import_and_unrelated_edit_do_not_invalidate_apply_or_undo`; J no-op semantic revision tests. |
| FND-17: stale import/download/publication fences | F holds an actual old refresh report and an actual old HTTP guide download across a committed repair, then checks saved choice, stored programmes, effective schedule and request count. Existing [guide-check recovery](../../crates/iptv-app/tests/app/guide_check_recovery.rs) and [combined guide handoff](../../crates/iptv-app/tests/app/combined_playlists.rs) retain deletion/late-report/download regressions. Writer-side stop checks and final install revision checks remain in place. |
| FND-18: saved/effective/verified distinction and real recovery | F `a_failed_postcommit_refresh_recovers_without_repeating_the_saved_mutation`: generated HTTP 404 produces `Failed` / `Effective::Failed`; a later explicit source refresh plus verification produces installed/verified state with exactly one durable repair record. |
| FND-19: stale verification cannot certify or replace newer work | R mapping-edit and same-configuration programme-refresh barriers; `verification_from_before_undo_cannot_overwrite_the_new_undo_operation`, `no_op_inverse_direction_also_fences_older_verification`, and cancellation-admission regression. Commit revisions, programme evidence, inverse direction and later cancellation markers are fenced. |
| FND-20: conditional inverse preserves unrelated edits | S `conflicting_undo_never_reverts_a_subset`, no-op/unrelated-edit inverse test, removed-feed position restoration and external use of a newly added feed; R unrelated edit survives undo. |
| FND-21: restart, restore, expiry and lost reply | R owner restart expires previews but retains inverse; `durable_inverse_receipt_reconciles_a_stale_original_operation_cache`; S restart/restore/expiry tests; J `restore_preserves_stale_transport_provenance_and_retags_only_authoritative_cache`. |
| FND-22: one facade for existing UI and headless workflow | R `headless_preview_apply_verify_undo_uses_real_domain_services`; UI-R two-channel shared-stage workflow; fresh iPad simulator checkpoint oracle above. The native runtime consumer links no NSH, view, player or daemon. The tablet live repair workflow passed; final keyboard-visibility evidence remains pending. |
| FND-23: real analysis overlap, bounds and responsiveness | I `a_hundred_thousand_channel_analysis_keeps_display_reads_responsive` holds an actual SQLite progress callback during the scan; R 100k-channel/1k-group repair with display reads, 500-row stage batch, held analysis/writer deadlines, preview/operation capacity tests; unchanged [store-latency gates](../../crates/iptv-app/tests/app/store_latency.rs) and full pinned gate. |
| FND-24: repair during playback without surface/focus disruption | UI-R `inspected_candidate_applies_verifies_and_undoes_during_uninterrupted_viewer_playback` captures native-protocol player commands, opens, primary surface/ancestor lifetime and focused viewer identity across preview/verify/undo. Android live inspection and repair retained primary surface and focus identity; final keyboard-visibility evidence and physical decoder claims remain limited as stated above. |
| FND-25: declared facade and independent consumer | C `independent_consumer_compiles_against_the_declared_service_types`; [runtime architecture checks](../../crates/iptv-runtime/tests/architecture.rs); core/library wasm and app builds. The declared facade exposes portable values, not store connections, NSH objects or implicit shown-source inputs. |
| FND-26: compatibility and honest capability states | C exact signature/field constructions and exhaustive error/status/capability matches; `Runtime::foundation_capabilities()` and the v1 compatibility policy. This verifies the candidate contract; final stable declaration awaits the remaining host/CI/review closure. |

## Bounds and latency interpretation

The [v1 contract](../runtime-api-v1.md#bounds-and-cancellation) records finite
query/page/deadline, XML, staging, matching-input, batch, preview/operation and
inverse-retention limits. A generated 100k-channel/1k-group repair test also ran
alone on Linux in 5.408 seconds with 163,968 KiB peak **whole-process** RSS,
including fixture/catalogue creation and database lanes. This is a measured
fixture process, not a claim about incremental allocation or all host workloads.

Two preliminary Linux wall-clock input runs measured 57.96 ms and 55.68 ms while
both owned Android emulator fixture apps were playing. After stopping only those
synthetic app instances, the isolated gate measured 35.14 ms. The original
50 ms assertion and workload were unchanged. The final complete pinned
`6f5829c` gate passed with competing fixture playback stopped. This records the
resource condition rather than weakening the input budget or treating a loaded
host result as an application fix.

## Reproduction and closure

The complete repository gate is `tools/check.sh` with the pinned sibling NSH
checkout and required native-player/linter prerequisites. Focused commands are
listed in the two foundation guides; the native consumer and app suites include:

```sh
cargo test -p iptv-runtime --features test-support --test foundation_inspection
cargo test -p iptv-runtime --features test-support --test foundation_probe
cargo test -p iptv-runtime --features test-support guide_repair --lib
cargo test -p iptv-runtime --features test-support --test foundation_repair_fences
cargo test -p iptv-runtime --test foundation_contract
cargo test -p iptv-store-sqlite guide_repair --lib
cargo test -p iptv-app --features test-support --test app foundation_inspection
cargo test -p iptv-app --features test-support --test app foundation_repair
```

Before committing a final acceptance record or changing the API status, fill
in the final Android repair result, remaining Android/package CI results and
review/merge outcomes for both PRs. Preserve the explicit simulator, synthetic
key, Keychain, physical-device and decoder limitations. Later test-only/harness
commits must identify whether production source remains equivalent to `6f5829c`.
