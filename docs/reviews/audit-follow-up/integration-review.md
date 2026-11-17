# Integration and delivery peer review

Root agent review of delivery-agent changes, November 16–17, 2026.

## Reviewed boundaries

- CI selector and aggregate include iOS and production broker checks. Standalone
  jobs have distinct names; legacy protected aliases still mirror the aggregate.
  No hosted branch protection settings were changed.
- Publication verifies each final file's digest-bound build provenance before any
  release mutation. Signed versus preview intent is a reviewed policy; partial or
  mismatched signing configuration fails rather than silently downgrading.
- Android smoke requires an explicit emulator serial, matching disposable AVD
  name and separate validation application ID before destructive test setup.
- Disposable broker tests remove inherited session/display addresses and run with
  private HOME/XDG roots and an owned foreground daemon. No user keyring is used.

## Findings resolved

1. A complete AVD file copy can still retain absolute qcow2 backing references.
   Reported before any emulator run. Tool now inspects every copied qcow2 chain,
   rejects unknown external backing, converts to an independent copied image and
   verifies the resulting chain before boot. Originals are read only.
2. Credential compiler errors were captured but discarded by the wrapper. The
   runner now emits rendered compiler diagnostics and stderr on failure.
3. New helper tests must participate in the required helper lane and local checks,
   not just exist as files. Delivery reconciled their execution and parity tests.

No remaining blocking findings in the reviewed delivery boundaries. This is source
review plus helper/production-broker execution, not a claim that a shipping
release has been published or every hosted workflow has completed.

## Validation observations

- Production Linux broker regression passed key creation/reopen and wrong,
  malformed, missing, locked and unavailable broker failures. Existing ciphertext
  remained unchanged on refusal. The intentionally locked session could not open
  a graphical unlock prompt, then failed closed.
- Initial full workspace Clippy passed after simplifying duplicate hidden-setting
  branches. First app integration run: 168 passed, 7 failed, 4 intentionally ignored.
  Failures identified missing recovery-message translations/extraction, stale
  expectations for hidden settings/updated backup wording, ambiguous multiview
  semantics selection, and numeric playback assertion using truncated display text.
  Corrections and final rerun are recorded in the implementation tracker.
- Workspace rustdoc passed without warnings; catalog process-death regressions
  passed. Further required checks are recorded separately after completion.

## Device instruction

The user explicitly requested no physical Android TV testing tonight and muted
emulators. This run uses emulator validation only, with `-no-audio`; physical-device
and spoken-screen-reader acceptance are not inferred from it.

## Final acceptance pass

The architecture reviewer checked the completed P11/P12 work against the accepted
plan at `34f214e` and found no additional feasible coding gap or critical
regression. Queue observations/canceled reads and the explicit per-window
constructor now meet those requirements; native experiments and manual platform
acceptance remain qualified separately.

Root validation of that implementation passed 658 affected app/runtime tests,
workspace Clippy, warning-free rustdoc, 16 doctests and the production app build.
All four strict shared-plan responsiveness gates and the GTK/PiP lifecycle test
passed before emulator workloads started. The corrected Windows fixture's four
completion tests passed locally after `bd8158e`; Windows execution remains a
separate hosted check. The fixture now syncs a writable descriptor, matching the
production writer's existing access mode.

New Android lifecycle diagnostics exposed a helper-test coupling: its mock
consumed diagnostic snapshots as polling responses. The UI reviewer separated
snapshot output from poll observations without changing the 1/20/40 observation
assertions; all six lifecycle and 13 ADB-timeout tests passed. This correction
does not change either emulator's two-player resume acceptance.
