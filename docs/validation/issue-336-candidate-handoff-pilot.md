# Windows candidate handoff contract pilot

This standalone experiment implements a binary handoff contract, not a
production optimization. It follows the measured baseline in
[issue-336-master-candidates.md](issue-336-master-candidates.md). Validation,
its planner, preview packages, unsigned candidates and release promotion keep
their current recipes and selection. Refs #336; whole-Validation improvement
and broader adoption require later evidence.

## Compatibility decision

| Existing consumer | Actual Windows app build | Compatible inputs |
| --- | --- | --- |
| `windows-package-check.yml` installer preview | `package-windows.ps1` calls `build-windows.ps1 -Release` | Root locked workspace, default production features, release profile, x86_64 MSVC, exact NSH and Windows runtime pins |
| `unsigned-candidate.yml` Windows package | The same `package-windows.ps1` call and release build | The same recipe; additionally publishes media SBOM and build inventory |

The candidate plan already skips preview package lanes. These are two
compatible existing consumers, **not evidence of duplicate candidate-grade
compilation in one selected run**. The experiment measures transport,
verification and scheduling overhead. Host debug builds and test-support
executables cannot supply this artifact. Linux and macOS packaging use NSH's
generated shipping workspace and its lock; they are outside this pilot.

## Isolated workflow

`candidate-handoff-pilot.yml` accepts manual dispatch and an exact
`ci/candidate-artifact-handoff-336` branch push bootstrap. The push registers
the unmerged workflow for pre-merge hosted execution; GitHub documents that
push can run unmerged workflows, and that a workflow which has already run
can be dispatched against a branch through the API. The exact repository and
branch are checked in the jobs and proof commands. There are no PR triggers,
write permissions, secrets, environments, promotion outputs or production
callers. All artifacts start `pilot336-` and expire quickly. The bootstrap
can be removed after hosted validation; manual dispatch still remains
restricted to the pilot branch while this infrastructure is experimental.

Manual `inputs_only: true` is a diagnostic mode: two fresh Windows jobs run
the same setup and strict fingerprint, then upload input evidence without
building, sealing, consuming, packaging or measuring a candidate. Normal
push, cold and reuse recipes retain their checks. Rust diagnostics contain
relative file names, sizes and SHA-256 for `lib` and `bin`; bounded UTF-8 text
is captured only for an explicit allowlist of Rustup installer receipts.
These are diagnostic artifacts, not compatibility exceptions. The full
Rust library tree must still match, and a mismatch remains fail-closed until
file-level evidence establishes the affected files and their role.

The producer and independent cold control start on fresh Windows runners.
`setup-windows` restores no Rust cache. The producer requires an absent
`target/release`, fingerprints its inputs, stamps the usual package metadata,
then runs the existing locked `build-windows.ps1 -Release` recipe. The
consumer starts on another fresh runner and independently fetches and
verifies the pinned media runtime. It fingerprints its own inputs before
reading the handoff, stages only a proven executable, and calls the existing
`package-windows.ps1 -SkipBuild`. Both package paths retain icon checks,
locked Rust/native/runtime notices, MSVC redistribution checks, the original
install/launch/uninstall smoke test with a synthetic profile, media SBOM and
build inventory. No pilot installer is published for promotion.

## Proof and refusal contract

The independent fingerprints include the exact clean checkout and tracked
source hashes, root Cargo lock, shipping lock policy input, all recipe and
workflow sources, clean exact NSH commit/tree, Rust pin and actual compiler
version/bytes/standard libraries, target/profile/default features, workspace
version, runtime DLL/inventory bytes, image version, MSVC compiler/linker
bytes and native headers/libraries, discoverable SDK versions and input
trees, and resolved Perl/NASM/CMake/Ninja bytes. The shipping lock is recorded
as policy metadata; the root Cargo lock resolves this Windows app. The
usual deterministic version stamp is part of the recorded source recipe.
Hashing native input trees has a cost; it is timed as part of the pilot.

Ambient Cargo configuration and unsupported compiler, flags, target,
profile and native-library overrides are refused. The helper reads only
fixed nonsecret identity fields and never serializes or hashes the whole
environment. Runner path strings are not compared as input identities;
tool and native input bytes are. SDK/image/tool differences fail closed.
The pinned toolchain action's exact `CARGO_INCREMENTAL=0` setting is allowed
and explicitly bound; other nonempty incremental overrides are refused.

The consumer receives expected SHA-256 values through trusted `needs`
outputs, not from the downloaded manifest alone. It verifies this workflow
run, exact head/repository/branch/event, the successful producer job ID and
its original attempt, plus the artifact ID, isolated name, run/head and
archive digest through read-only REST calls. A consumer-only rerun may retain
the successful producer's original attempt and immutable artifact identity;
it does not substitute the current workflow attempt. The containing run may
still be executing, so the producer job's completed success is required.

The actual downloaded ZIP must match the trusted upload digest before it is
opened. Only `manifest.json` and `iptv-app.exe` are accepted, with bounded
sizes, regular file types, no duplicates, directories, links, encrypted
members, arbitrary paths or extraction into the checkout. Manifest bytes,
producer identity, independent inputs, executable size and executable
digest are checked separately before the executable is moved into an absent
release directory. Downloaded scripts are never executed.

| Observation | Result |
| --- | --- |
| Explicit `cold: true` dispatch | Do not read/download artifacts; fresh local release build, then all package checks |
| Successful, fully paginated artifact list proves expected artifact absent | Fresh local release build; record `absent` |
| Matching authoritative metadata says `expired: true` | Fresh local release build; record `expired` |
| Verified producer, inputs, metadata, archive, manifest and executable | Stage only executable; record `reuse` |
| Producer failure, wrong/missing proof, different inputs, ambiguous artifacts, corrupt bytes or unsafe archive | Fail visibly; no hidden build fallback |
| API, download or parsing error, including a download 404 | Fail visibly; no hidden build fallback |

An expired-artifact control is synthetic in helper tests. Real expiration,
consumer retry with a retained successful producer, and the token/API/archive
contract need hosted evidence. No API response is forged to force a green
fallback. A successful first hosted run proves only the exact tested recipe
and handoff, not a broader performance result.

## Measurement protocol

The timeline artifact records actual GitHub job creation/start/completion
times, per-step timestamps, queue/execution durations and trigger-to-job
completion. The independent cold control runs in parallel with the
producer-to-consumer path. Compare those actual path endpoints, and the
maximum endpoint of all three jobs for the whole pilot. Do not add parallel
durations or subtract compile time to claim a workflow win. Preparation,
fingerprinting, compilation, upload, acquisition/verification, packaging and
smoke steps remain visible separately. Runner acquisition/scheduling and
fallback decisions are evidence, not product failures or improvement.

For a later measurement cohort, collect at least 20 independent paired runs
at the same exact revision and image/tool identity. Keep reuse, explicit
cold, absent and expired decisions separate; preserve failed/canceled runs.
The first correctness probe is `n=1` and cannot establish stable p90 or a
speedup. Download timeline JSON and corresponding input/decision evidence
outside the repo, check image/input comparability with the cold control, and
combine timeline objects into a JSON array. Then run:

```sh
PILOT_SAMPLES=/outside/repo/timelines.json \
  python3 tools/ci/candidate-handoff.py summarize /outside/repo/summary.json
```

The helper reports n, successful n, failures, median p50 and nearest-rank p90
for each actual path and queue/execution segment. It refuses mixed heads,
duplicate run IDs and mixed fallback decisions. It also separates clean
and retried cohorts, preserves failed/canceled
earlier attempts, and reports all-attempt execution/queue costs per run and
job alongside final path endpoints. A successful retry does not erase a
failed attempt from reliability or runner-cost evidence.
The requested cold/reuse mode is recorded independently from consumer
outputs, so failed/canceled/skipped consumers with unavailable decisions
stay in the intended cohort and failure counts. A successful consumer must
have a concrete decision; distinct concrete fallback decisions stay separate.
Inherited-attempt or invalid queue intervals are unavailable: GitHub may
rewrite carried-forward job creation times. Raw timestamps remain, and
reports sum only known queue durations with an unknown-attempt count.
Reports must state cohort size, runner/tool identity, missing evidence and failures alongside
percentiles. There is no production integration or whole-Validation gain
claim in this PR. Any adoption requires a separate coordinated change and
measurements of its real integrated critical path.

Primary API references:
[workflow events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows),
[workflow jobs](https://docs.github.com/en/rest/actions/workflow-jobs), and
[artifacts](https://docs.github.com/en/rest/actions/artifacts).
