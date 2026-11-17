# Final local implementation validation

The full shared command `bash tools/check.sh full` completed with exit 0 and
`all checks passed` on the rebased implementation tree. Pinned Rust 1.99.1,
exact NSH revision `9ed782cbf362ab63ba18c47dc657fe944c405e50`, and the guarded
Linux libmpv runtime were used. The runner reused ignored build artifacts; it
did not bypass native guards or alter the NSH checkout.

## Executed checks

| Check | Result |
|---|---|
| Formatting, Rust line lengths and available repository linters | Passed |
| Dependency advisories, bans, licenses and source audit | Passed |
| Workspace all-target Clippy, runtime/app test support, warnings as errors | Passed |
| Ordinary feature-unified workspace tests | 1,949 passed, zero failed, 13 ignored across 27 harnesses |
| Application scenarios within that run | 173 passed, zero failed, four ignored |
| Core tests within that run | 379 unit and three integration scenarios passed |
| Four isolated repair/inspection/display-reader/input budget gates | Passed, one at a time |
| Portable wasm builds | All nine portable crates passed, including merged nav fixes |
| Separate workspace doctests | 16 passed, zero failed, two ignored |
| Private-Xvfb GTK player/PiP/context-loss scenario | Passed with actual software GL and guarded libmpv |
| Production `iptv-app` build | Passed |
| Locked package, test-target budget, parity and release/build helper regressions | Passed |
| `cargo doc --locked --workspace --no-deps` | Completed; 25 warnings in existing documentation links |

Ordinary ignored cases are not claimed as passes. The shared plan separately
ran the required four timing gates and native GTK scenario. Large opt-in fixture
tools and other ignored manual scenarios retain their existing limitations.
The documentation warning lines were checked against `origin/master`; all 25
already exist in the base. They were recorded rather than broadening this
correctness patch into unrelated documentation cleanup.

The shared plan parent was held after ordinary tests while Android emulators
finished, then resumed after both VMs stopped and fixture preparation completed.
The ordinary worker continued normally; the isolated gates ran without competing
Android emulator, encode, build or other test workloads.

## Follow-up CI synchronization fixes

Platform CI found two new Linux-only native-hook tests lacking matching target
conditions. Two test attributes corrected macOS/Windows compilation; macOS then
passed 46 native tests and the production build, and Windows native also passed.

Two broader CI scenarios exposed existing scheduling assumptions: a Linux startup
future could already be ready at its first poll, and a Windows rename could be
durable before its dialog finished refreshing. The tests now use an acknowledged
writer hold and actual modal disappearance, respectively. Both scoped checks
passed on Linux with all cleanup, committed-data and subsequent-action assertions
retained. These subsequent changes affect tests only. Final-head CI remains
a separate gate; older or superseded results are not final-head evidence.

## Platform and manual limits

[Android evidence](android-validation.md) records the final three-ABI APK,
25 Java tests, typecheck, both API 36 emulator journeys and a physical 32-bit TV
journey, including native Stop. The physical tablet is unavailable behind its
closed hardware cover. Actual spoken accessibility output and remaining manual
subscreen focus checks are not verified. The emulator setup incident is disclosed
in that record.

[CI evidence](ci-validation.md) records immutable prior runs and the explicit
Windows full workflow needed to execute the reparse-point regression. Current PR
checks and review state govern merge; a local Linux pass cannot establish another
host's behavior.

## Reproducibility records

Coordinated commands/logs: `/tmp/patina-rust-fixes-20261116/`.

- `full-check-final2.log`: complete shared run.
- `ordinary-suite-results.json`: one final result per executable; nested child
  stdout is not double-counted.
- `player-final-tests.log`, `recordings-final-tests.log`,
  `parental-final-tests.log`, `runtime-foundation-final.log`: targeted evidence.
- `startup-cleanup-regression.log`, `organization-rename-regression.log`: scoped
  checks after CI exposed scheduling assumptions.
- `workspace-docs.log`, `rustdoc-warning-baseline.json`: documentation check.

Final Android APK, hash metadata, inspected synthetic captures, safe runners and
retained failed/positive fixture evidence are under ignored
`target/rust-review-android-validation` in the original checkout. No real provider
material was used. These summaries preserve the results even if temporary files
are later removed.
