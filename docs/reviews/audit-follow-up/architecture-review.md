# Architecture peer review of the audit follow-up

Reviewed the shared implementation on 2026-11-16. This is an independent review
of the UX agent's P4/P5/P8/P9 changes; the recording implementation listed below
is this reviewer's own work and receives separate peer review.

## Reviewed areas

- P4 picture preferences now use source ID, source incarnation and content ID;
  catalog row changes and source replacement cannot inherit a different channel's
  picture setting. On-demand playback uses the global preference.
- P5 foreground reopening uses an explicit event with a publication token.
  Stale events are fenced before a new health session is created. Reopening
  repeats parental authorization and provider reservation, preserves pause and
  retry state, and retains pre-ready VOD/recorded resume position. Native retained
  URLs do not themselves authorize reopening.
- P8 numeric input retains a resolved content identity until commit and checks
  current membership/visibility again. Clear-watch-history commands pass through
  the ordered writer. Delayed history reads are fenced against later clearing.
- P9 Android recording notifications obtain strings from a configuration context
  for the selected UI language; settings changes refresh the notification and
  channel name. Unimplemented settings are hidden through the shared offer policy.

## Finding and correction

P8 initially constructed lazy search-history save/load futures. An older save
polled after a clear could run after that clear and resurrect history. An old
legacy file could also be imported by a later startup load. Reported this to the
UX author; the correction enqueues save/load when called and adds an ordered
clear command that removes both database history and the unconsumed legacy file.
Regression scenarios cover reversed future polling and clear-before-startup.
The corrected source was reviewed. No further blocking findings in these areas.

## Recording implementation evidence

The recording changes add a byte-bounded isolated writer with acknowledged drain
before finalization; bounded database retries that preserve compare-and-swap
failure; a durable completion receipt and content verification around publication;
and quota accounting that subtracts already-written active bytes from reservations.
Receipt-less interrupted recordings retain conservative Partial recovery.
Same-executable child processes exit abruptly after spool, intent, rename and
catalog-commit boundaries; restart verifies exact retained data and terminal state.

Scoped results before final shared validation:

- Runtime recording tests: 67 passed, including process-death boundaries,
  mismatched final content, bounded retry and existing stop/recovery regressions.
- Network recording tests: 14 unit and 10 integration tests passed, including
  delayed disk drain, synthetic disk-full failure and retained-prefix limits.
- Store unit run: 93 passed, one failed because schema 20's new data fixture was
  missing, two ignored. Added the fixture, historical schema entry and persistence
  assertions; the root agent's shared run validates the correction.
- Earlier runtime/network/store all-target check and schema snapshot generation
  passed. Later small edits, including spool-directory sync before receipt commit,
  are covered by the shared validation rather than claimed as a repeated scoped run.

## Limits

Native foreground and notification paths were reviewed from source here; this
review does not claim physical-device or GUI execution. Android validation is
owned by the root agent and uses emulators with audio disabled. An OS file write
cannot be forcibly interrupted safely: its worker retains bounded capacity until
termination, and finalization waits for the close acknowledgement. Native Unix
publication syncs file-directory metadata; portable non-Unix directory-sync
semantics remain platform dependent.
