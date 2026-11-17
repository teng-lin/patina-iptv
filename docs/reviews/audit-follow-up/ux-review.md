# Audit follow-up: UI/native implementation and recording peer review

Reviewed the follow-up working tree based on `323c18eb` on November 16–17,
2026. Scope is application behavior and the changed recording library boundaries;
this is a source review, not a claim of device, speech-accessibility, or shipping
release acceptance. Rust review and Rust best-practices guidance were applied.
No blocking recording defect was identified in this scoped review. Shared full
Rust validation and emulator evidence belong to the implementation record.

## Recording review

Inspected `iptv-net/src/recording/writer.rs`, runtime
`recordings/completion.rs` and its callers, and the SQLite completion methods.

- The file worker owns its descriptor, bounds chunks to 256 KiB and queue depth
  to one, and holds a global worker permit through actual descriptor closure.
  Cancelling the network future does not acknowledge completion before accepted
  writes drain, the file syncs, and the descriptor closes. The runtime retains
  its active reservation across this acknowledgement.
- Terminal receipts precede filename publication. Their compare-and-set guards,
  digest/length verification, no-overwrite publication, and idempotent catalog
  acknowledgement preserve the distinction between superseded intent and
  storage failure. Bounded retry applies only to retryable store errors.
- Recovery consults pending receipts before treating older receipt-less files
  as interrupted partial media. Failed recovery is subsequently surfaced by
  the scheduler's `finalization_errors` path, rather than reported as success.
- Synthetic regressions cover blocked writes, disk-full behavior, byte limits,
  and child-process exits at spool/intent/rename/commit boundaries. These tests
  were read here; their execution results are maintained by the runtime owner
  and full-plan runner, not independently asserted by this review.

## UI and native implementation evidence

Implemented source/incarnation/content picture keys with a global fallback for
on-demand playback; typed token-fenced foreground reauthorization through the
existing parental/reservation/source checks; core numeric entry with timeout,
correction and identity retention; durable history clearing; hidden unsupported
settings; and multiview button names and selected state. Foreground authorization
preserves paused state, retry accounting, and a pre-ready saved position. Stop
continues to reject stale native events. Android and iOS use their existing
foreground hooks; iOS reaches the shared mpv `resume` implementation.

The recording notification uses Android localized resources and the selected
application language. The backup export note explicitly identifies the file as
unencrypted in all catalog languages. Auxiliary player windows now receive
independent presentation state through the exhaustive `Ui::new` constructor.
Application-wide settings, catalog signals and picture choices live in named
`SharedUi`; native stream handles and their task owner cross the window boundary
explicitly through `Player::for_window`. This removes parent-copy/manual-reset
inheritance, including the previously missed `Ui.vod.open` signal that let main
Movies/Shows capture auxiliary input. The post-root shell runtime thread contract now asserts in
release builds too; JNI recording callbacks use explicit host ownership.

The architecture peer review identified a real ordering defect in the first
history-clear draft: lazy save/load futures could enqueue older data after a
clear. Save/load now submit eagerly; a dedicated clear command also removes the
legacy file on the writer queue. The regression deliberately polls older replies
after the clear and covers clear before legacy startup load. Numeric input also
starts a new number after an expired deadline and cancels when a modal opens.

Direct validation:

- Workspace all-target `cargo check` with app/runtime test-support passed before
  the final ordering and small UI refinements. Final-head Rust checks are delegated
  to the shared full-plan runner and must not be inferred from this earlier pass.
- `:app:testDebugUnitTest` passed on November 17 at approximately 04:13 UTC:
  25 tests, zero failures/errors/skips, including the shared typed player-event
  fixture. This compiled Android Kotlin and all localized resources. The command
  used Java 21, the installed SDK, and the adjacent NSH Java source explicitly.
- After the full app run exposed stale expectations and missing recovery-message
  rows, the rebuilt app harness passed all 25 targeted catalogue, interface-language,
  numeric-entry, and multiview tests (two catalogue tools intentionally ignored).
  Numeric entry asserts the exact synthetic native source, and tile semantics are
  scoped to the multiview presentation. The source catalogue has 1,479 unique rows
  with all ten locale columns populated.
- The later P12 app recompile and existing `guide_player_windows` journey passed
  after adding main Movies/Shows browsing while an auxiliary player stays open.
  Both pointer and keyboard pause controls remained local; the main pause state,
  both exact native source values and the auxiliary native node were unchanged.
  The complete journey also retained its existing track, volume, channel-change,
  multiple-window, PiP and close assertions (one scenario, approximately 30 seconds).
- Gradle reported SDK metadata, existing manifest extraction, and deprecated API
  warnings. No device, emulator, real provider, or credentials were used by this
  agent. A separate core test invocation lost its captured completion output;
  it is not claimed as a pass.

## Remaining limits

The parent runner owns required final Rust checks and muted disposable emulator
runs. No physical Android TV testing is authorized tonight. iOS execution,
notification language appearance during an actual recording, and spoken
accessibility remain native/manual validation limits; semantic tree assertions
and resource compilation do not prove spoken output. Native teardown was not
speculatively rewritten in this scope.

## Store queue peer review

The later P11 diff in `store_service.rs` adds numeric-only queue observations and
skips dropped read/analysis replies before opening SQLite. Reviewed the RAII
queue-ticket lifecycle, failed-send cleanup, execution paths and four synthetic
regressions; no blocking defect found. Writer work still executes in FIFO order
when its caller drops the reply. Metrics retain only pending timestamps and
aggregate maxima/counts, with no provider/query/path values or completed-request
history. The explicit accepted test blocker replaces the old intentionally
unpolled analysis request. The architecture owner reports 13 scoped store-service
tests passing; this review did not repeat those executions.
