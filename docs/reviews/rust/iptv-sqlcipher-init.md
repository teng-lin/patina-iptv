# Rust review: iptv-sqlcipher-init

This native library completes SQLCipher initialization before owned database
connections open. No confirmed defect was found in its narrow initialization
contract. Keep the success-only readiness cache and export registration probe.
The coordinated workspace Clippy pass succeeded; test execution is coordinated
by the parent review. No implementation changes or commits are part of this report.

## Baseline and evidence

- Application revision: `0e197073a272eceb0631e4d53b08d2823950c416`.
- Edition 2024; declared Rust version 1.99.1; internal native library.
- Reviewed the pinned `sqlcipher-src` 420.0.0 / SQLCipher 4.20.0 C implementation
  of `sqlcipher_extra_init`: the null argument and integer return match the
  declaration; initialization uses SQLite's static master mutex; export
  auto-extension registration occurs after that mutex is released.
- Workspace Clippy passed with `--all-targets` and runtime/app test-support
  features, using `-D warnings` (parent-coordinated evidence).
- The full host wrapper stops at the existing NSH revision mismatch:
  actual `32769658ad34ad364b960a134cc73c754d50f2d9`, pinned
  `9ed782cbf362ab63ba18c47dc657fe944c405e50`.
- Non-player workspace tests passed with `--tests` and runtime test-support;
  app build and workspace doctests passed. The full workspace invocation
  exited 101 in the player crate (50 passed, 10 failed there). The differing
  NSH commits have identical tracked trees; the wrapper checks commit identity.
- Cold-process integration evidence belongs to the existing store harness,
  especially `tests/it/cipher_initialization.rs`,
  `encryption::tests::concurrent_first_connections_wait_for_cipher_initialization`,
  and `inspection_stage::tests::probe_stage_initialization_precedes_its_first_encrypted_connection`.
  This report does not claim an additional standalone test target was run.

## Must fix

None established by this review.

## Should fix

None established by this review.

## Consider

The five-second export polling loop intentionally waits for registration by a
foreign first opener. Retain its deadline and failure behavior. A future change
to the native dependency must recheck the private C symbol and registration
ordering, then rerun the cold-process tests. Do not infer readiness from the
SQLite core-ready flag alone.

## What's already good

- Unsafe calls have specific invariants and the ABI matches the pinned source.
- Only successful process readiness is cached; failed initialization can retry.
- A fresh connection checks actual export availability instead of assuming the
  provider hook has finished registration.
- Keys, filesystem paths and runtime owners remain outside this process cache.

## File coverage

- [x] `Cargo.toml`: native features and Apple versus other crypto backend selection.
- [x] `src/lib.rs`: complete source, failure propagation, concurrency and readiness cache.
- [x] Existing cross-crate cold-process harnesses named above inspected for their
  actual provider/export assertions and child-process timeout/reaping behavior.

The crate has no build script, examples, fixtures or private test module of its
own. Native execution on Apple, Windows and Android remains outside this Linux
source review; no platform validation claim is made for those targets.

## Disposition

No finding IDs assigned because no actionable defect was confirmed. Keep this
report in the consolidation inventory, with coordinated validation recorded
separately from source-review conclusions.
