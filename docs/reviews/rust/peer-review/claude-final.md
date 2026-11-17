# Final plan review

**Verdict: APPROVE** for future implementation planning. No fix, regression gate or platform gate is complete, and no defect is closed.

## Blocking omissions

None.

### Consistency checks performed (packet-internal only)

- **Finding coverage:** the disposition table has exactly 39 IDs across 13 crates with findings. The fourteenth crate, `iptv-sqlcipher-init`, is explicitly retained with no defect. Each ID appears in exactly one package, and each package's "Findings:" line matches the table. `iptv-guide-002` and `iptv-runtime-003` are merged consistently into W5, with runtime-003 as the integration owner.
- **Unsafe initial suggestions resolved:**
  - Single-process-writer premise: rejected with the per-Service writer evidence (`lib.rs:168`, `recordings/host.rs:61`). Write intent is limited to actual read-then-write mutations (W4).
  - Blanket reuse of the `native_allowed` epoch denial: replaced by effective-permission rechecks. Unprotected access during zapping is a stated gate (W7).
  - Shared refusal decrement covering the hook failure: replaced by phase-aware, exactly-once, identity-bound completion (W8).
  - Predetermined render/worker destructor reorder: downgraded to a bounded investigation with explicit prohibitions (W14).
  - Reply-wins query replacement: rejected. The raw append contract, duplicates, encoding, fragments and path prefixes are preserved (W2).
  - Tolerant seconds parsing: rejected under the documented `time.rs:32` contract. Seconds 61–99 are invalid and 60 keeps its folding (W2).
  - Saturating intermediate multiplication: replaced by complete widening before clamping (W2).
  - Whole-chunk write timeout presented as an idle policy: replaced by a progress-reset policy with a test that keeps progressing past 15 seconds (W12).
  - Rejecting userinfo to avoid redaction: prohibited (W10).
  - Hiding fields before migrating constructors: ordering is fixed (W11).
- **Behavior and ownership preserved:**
  - Persistent player ownership: native views stay mounted and surface 0 is preserved (W9). W14 forbids detaching ownership or adding unbounded reapers.
  - Durable feed IDs and manual mappings: never silently renumbered (W3).
  - Cross-thread store ownership: unchanged (W4).
  - Stale preview resources: released through the accepting runtime owner (W9).
  - Existing accepted recording jobs: not reauthorized after relock, and stop/cancel remains available (W7).
  - Relay/native guards: not weakened to make baseline tests pass (W8, Wave 0).
- **Dependencies and waves:**
  - W5 and W6 depend on W4: same wave, which is acceptable with intra-wave ordering (see comment 2).
  - W7 depends on W1 and W4: placed in a later wave.
  - W14 depends on W8 and the restored baseline: last wave.
  - `iptv-nav` is included in the final combined validation.
- **Regression gates:** every package has concrete gates and requires a regression that fails against the baseline before the fix. Gates assert identity or outcome rather than row position. Concurrency gates use barriers and existing harnesses, and timing gates run isolated through `tools/ci/check-plan.py`.
- **Baseline honesty:** the NSH mismatch, missing wasm standard library and the 10 system-libmpv failures are kept as limitations. Wave 0 has to resolve them without downgrading guards.

## Nonblocking comments

1. **Table wording for providers-003.** "define reply-key precedence" could be read as an open decision, but W2 and the decisions settle it as the existing raw append. Suggested table text: "preserve raw signed query bytes and the existing append/duplicate-order contract."
2. **Intra-wave ordering.** Wave 2 lists W4, W5 and W6 together even though W5 and W6 depend on W4. Add "W4 lands first within Wave 2" to the exit or ordering text.
3. **P1 items in Wave 4.** Only app-005 (Stop/sleep) is called out as eligible to move earlier. app-003 (repair callbacks, P1) is also in Wave 4. Consider naming it as pull-forward eligible too, or state why it can wait.
4. **W4 gate phrasing.** "two writers causing SQLITE_BUSY_SNAPSHOT" describes the reproduction, not the pass condition. Suggested text: "two Service writers on a demonstrated read-then-write path complete without SQLITE_BUSY_SNAPSHOT, or retry within the stated bound."
5. **W12 Windows reparse handling.** The fix text includes it, but the gates list only synthetic symlink/FIFO cases. Add an explicit Windows reparse-point gate, or an explicit deferral to the affected-platform checks.
6. **W3 persisted-repair choice.** The plan allows "fail-closed validation or transactional repair." When implementation starts, record which one was chosen per path so restore and migration tests target that behavior.
7. **W7 split.** Change 1 has a large scope: inspection, pagination, cursors, repair admission and final recheck. Reviewers may want it split again by query family if the diff gets large; the plan allows that.
8. **W9 app-005 and W8.** Stop/sleep fencing of pending playback may interact with W8's load-completion accounting. If Stop/sleep is pulled forward ahead of W8, note that dependency or require a re-run after W8.
9. **Review-record placeholder.** Per your instruction this is treated as clerical completion and not blocking.

## Evidence limitations

- This review read only the supplied packet. It did not check source, reports, line references, reproductions or counts.
- Line citations (`store_service.rs:97`, `lib.rs:168`, `recordings/host.rs:61`, `time.rs:32`) and agent verdicts are accepted as stated.
- agy's review was packet-only. The plan states this correctly and does not present it as source verification.
- No claim is made of a remote exploit, a release crash, a production native deadlock, other-platform execution or exhaustive test coverage. The plan's matching disclaimers are appropriate.
- As instructed, I used no tools and wrote no files, including the plan-mode plan file.
