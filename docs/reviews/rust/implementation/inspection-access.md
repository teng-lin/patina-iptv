# Inspection access review and corrections (W7)

Scope: the application runtime's v1 inspection consumer and the Store library adapter, especially pre-limit authorization and publication after an asynchronous read. This review applies `rust-review` correctness/error-handling rules and `rust-best-practices` guidance for explicit authority, coherent compound reads, and meaningful behavior tests. Root owns the policy implementation and central Cargo validation; this agent did not run Cargo.

## Findings corrected

1. **An empty programme window omitted its explicit channel from final authorization dependencies.** The SQL subject gate could accept an unlocked channel, return no programmes, then publish success after relock because only returned rows contributed dependencies. Runtime now includes the optional requested channel even when the page is empty. The final synchronous fence checks the accepting owner, policy revision, and the actual grants needed by that subject/results after the last await.
2. **Raw adapter calls had inconsistent default authority.** Without an explicit trusted scope, the adapter previously excluded legacy overlay blocks but could expose modern, adult-parent or custom-parent blocked content. The default is now restricted with no grants; explicit `InspectionAccess` carries policy-off/global authority or exact source/kind/content grants. SQL applies modern/legacy/direct/parent/adult restrictions before limit and lookahead. An exact live grant may authorize its blocked live subject; a parent grant does not erase a separate direct block.
3. **Some raw compound reads started their snapshot after scope or subject reads.** The runtime's scoped outer snapshot already covered these, but raw library callers could combine commits. Direct `inspection_channel` now uses `read_snapshot`; programme/candidate/coverage snapshots start before their first scope/subject read. They reuse an outer snapshot and retain typed scope/subject errors. The scoped access guard restores the caller's prior access on success and failure.

## Invariants checked

- The same restricted predicate gates channel pages, known channel subjects, programme rows and coverage subjects; provider and custom group memberships are qualified by source.
- Explicit allow overrides provider adult labels consistently with `parental_target_blocked`; direct legacy and modern blocks remain authoritative without the corresponding live grant.
- The runtime records actual target/blocked ancestor dependencies inside the same Store snapshot as the result. Its final fence has no await after validation.
- Cursor seals include accepting owner/service epoch and effective access/policy context. A grant/relock or policy change invalidates old permission cursors; unrelated zapping does not invalidate a cursor that needs no grant.
- Separate policy revision and grant epoch preserve unprotected access during ordinary zapping while rejecting a result authorized under an older policy or a revoked/expired needed grant.
- Policy-off is explicit runtime authority; raw library access remains fail-closed.

## Added regressions

Existing `foundation_inspection` harness:

- `inspection_filters_modern_and_legacy_blocks_before_limit_and_honors_policy_off`
- `inspection_adult_and_custom_group_access_matches_exact_source_grants`
- `inspection_cursors_bind_permissions_but_ignore_unprotected_zapping`
- `inspection_final_publication_rechecks_relock_even_for_an_empty_known_channel_window`
- `unprotected_inspection_survives_zapping_but_not_policy_revision_changes`

The publication tests acknowledge a later request on the same FIFO analysis lane before relocking/changing policy, so earlier SQL has completed while its public result remains unconsumed. They exercise the final publication fence deterministically without sleeps. The tests also cover known-ID candidate/programme denial, valid legacy unlock, full filtered pages, explicit adult allow, custom-parent grant versus direct block, and isolation of identical content/group IDs in another source.

Direct rustfmt and the owned-files whitespace check pass. Central compile/test results must be recorded by the coordinating agent before merge; no executed Cargo success is claimed here.

## Targeted refresh policy fence

The final review found that a targeted parental refresh could discover changed known blocked bits or ancestry without advancing the policy revision. `parental_refresh_targets` now reads target/ancestor metadata in one Store snapshot, distinguishes a first partial-cache fill from changed previously known metadata, and advances policy_revision once when known target/parent blocked bits or known ancestry changes. It does not treat ordinary unblocked zapping, an unchanged refresh, or an unknown new target as a policy change.

Private deterministic regression `parental::source_tests::targeted_refresh_invalidates_changed_known_policy_but_not_new_cache_entries` exercises actual named targeted reads and Store edits. It proves first cache-fill acceptance, a known parent's changed block discovered through a new child, unrelated cache-fill and ordinary-zapping acceptance, a known subject's changed direct block, changed custom-group ancestry, and unchanged refresh acceptance by checking captured publication fences.

The integration fixture now installs an explicit task host for settings completions. The older hidden-versus-blocked test explicitly enables protected live access, matching the policy-off contract; its source predicate was not weakened. Root's first combined test run identified these fixture assumptions; they are corrected and require the coordinating agent's rerun.
