# Implementation CI validation

CI evidence is tied to the checked-out commit. A failed or superseded run does
not establish that the current head passed. No local Cargo commands are run by
the CI observer.

## Superseded initial run

Head `5ffa9a5ff8461a9a930034e468e7bc6856bab27a`,
[Validation 44006130353](https://github.com/teng-lin/patina-iptv/actions/runs/44006130353):

- Helpers and stress/memory jobs passed.
- macOS and Windows native compilation failed because two new worker regression
  functions referenced fault hooks compiled only for Linux. The functions lacked
  the matching Linux configuration condition. The correction changes test
  configuration only; production native behavior is unchanged.
- Aggregate failures merely propagated those failed selected jobs. They were
  not additional source failures.
- The run was superseded by later commits; other intermediate job results are
  not final-head evidence.

## CI checkpoint before synchronization corrections

Head `0c81937e5861fc9a6490434dbd304a1466effda5` includes the Linux test-condition
correction and final application fixture fixes.

- [Validation 44006759498](https://github.com/teng-lin/patina-iptv/actions/runs/44006759498):
  failed overall because of the Linux host test described below. Helpers,
  stress/memory and Android typecheck passed.
  macOS native validation passed all-target compilation, 46 player tests
  (zero failures or ignored tests) and the production app build. Windows native,
  synthetic fuzz and APK build passed. Linux host failed in
  `lifecycle_recovery::restart_cleans_interrupted_catalogue_work_without_touching_committed_content`
  because its first startup-future poll was already ready instead of pending
  (`lifecycle_recovery.rs:261`); runtime unit tests had passed 245 tests and the
  integration target reported 88 passed, one failed, one ignored. The failure
  is reported for correction. Both tablet and TV emulator smoke jobs passed.
- [Windows full 44006791942](https://github.com/teng-lin/patina-iptv/actions/runs/44006791942):
  failed in an application scenario (169 passed, one failed, four ignored).
  `library_management::organization_playlist_policies_and_remote_bindings_are_editable_and_persisted`
  attempted to press an organization row while the Display name modal was
  still present. The failure is reported for correction; no complete Windows
  pass or network regression execution is claimed. This was explicitly
  dispatched because the PR tier's Windows native
  plan compiles network tests but does not execute them. The full run must
  execute the private-file reparse-point replacement regression; source review
  or cross-compilation alone cannot close that platform gate.

Pending jobs and deliberately skipped package/coverage lanes are not claimed
as passes. The root agent owns final-head checks, review disposition and merge.

## Prepared synchronization corrections

The root agent retained the Linux startup test's pending-future assertion and
placed an acknowledged writer hold before starting data. The test now observes
the worker at that hold before polling, then releases the writer and drains
without polling the UI future again. The original abandoned-stage cleanup,
unrelated-file retention, source-incarnation and committed-data assertions remain.

The Windows scenario now waits for the Display name modal to close after each
of its three rename submissions before attempting another organization action.
Database assertions remain intact. Both corrections affect tests only; both
scoped Linux checks passed (startup cleanup 0.05 seconds, organization journey
35.53 seconds). The subsequent committed head requires new CI runs; the PR
checks report current-head status without treating these older runs as final.
