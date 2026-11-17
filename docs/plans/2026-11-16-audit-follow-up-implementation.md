# Audit follow-up implementation and validation

Status: implementation and independent review in progress. User authorized one
PR, review/CI/conflict repair and merge when ready. No release or protection-rule
change is included.

## Accepted base and isolation

- #98 final branch head: `6d5ba65b9fd645af3eed1a483f68908e569028a8`.
- Accepted merge: `323c18ebac4718e81fb2c679669c081ccf1cf06b`.
- Worktree: `patina-audit-followup/patina-iptv`, branch `fix/audit-follow-up`.
- Rust 1.99.1; NSH `9ed782cbf362ab63ba18c47dc657fe944c405e50`.
- Separate Cargo target directory. Synthetic fixtures only. Original device and
  keyring profiles must not be modified by validation.

## Package disposition

| Package | Implementation / evidence to verify |
| --- | --- |
| P0 | Rebased onto accepted #98; previous PR test reports are historical evidence only. |
| P1 | Release/changelog/provenance documentation and publishing contract. |
| P2 | Encrypted recording completion intents, bounded retry, incarnation and file verification, real process-death recovery. |
| P3 | Bounded blocking capture writer with async drain; active quota uses remaining reservation. |
| P4 | Source-incarnation-qualified picture preferences, global preference remains separate. |
| P5 | Typed foreground reauthorization, stale-token/Stop/paused-VOD regressions. |
| P6 | CI dependency selection, required iOS and production broker lanes, coverage inventory and unique check names. |
| P7 | Disposable production Linux broker: reopen, wrong/malformed/missing/locked/unavailable failures. Shipping and physical-device acceptance recorded separately. |
| P8 | Numeric entry, clear viewing/search history, unsupported controls hidden. |
| P9 | Multiview semantics and localized native notification/export text; spoken AT and fluent-human acceptance remain manual. |
| P10 | Raster header allocation bounds, Xtream origin-scoped LAN trust, release-only fixture exclusion, documented plaintext/authority/native assumptions; M3U chunk-invariance fuzz oracle. |
| P11 | Real process-death restore/catalog tests, bounded recording retirement and durable retry tests. Performance/backend choices need representative hardware measurements. |
| P12 | Search state consolidated behind runtime facade; rustdoc repairs and contributor/runbook documentation. |
| P13 | Disposable AVD and broker tools, explicit ownership and cleanup bounds, durable validation record. |
| P14 | Existing teardown ordering retained; no new native proof supports a safe asynchronous ownership redesign. |

## Validation and review

Initial required `cargo test -p iptv-core` and workspace all-target compilation
passed on the accepted base. These are baseline checks, not final-change evidence.
Final checks and independent agent findings will be recorded here before merge.

## Explicit limits

Automated emulator tests cannot establish physical tablet cover behavior, spoken
TalkBack/VoiceOver output, fluent translation review or representative Windows
software-render/multiview performance. Debug validation is not shipping-release
artifact acceptance. No such result will be inferred from compilation or source
inspection. Locale initialization and native teardown need a separate native
experiment before a behavior change; see [security model](../security.md).
