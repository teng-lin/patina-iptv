# NSH fork operating ledger

Patina uses a sibling NSH checkout at the commit in [nsh.rev](../nsh.rev), from the
`tv-keys` branch. Cargo path dependencies do not pin that checkout; the local pin
check and CI setup do. Never substitute an ahead checkout's passing result for the
pinned dependency graph. Use an isolated target directory for each Patina worktree.

| Responsibility | Current contract / next action |
|---|---|
| Version adoption | Change nsh.rev in a reviewed PR, inspect the fork diff, run the shared full plan and affected native journeys. |
| Patch scope | Keep TV-key, lifecycle, rendering, accessibility and packaging changes small with framework-side regressions. Avoid unrelated framework edits in an application fix. |
| Upstream status | Upstreaming is intended, not a promise that every patch is accepted or temporary. Each adoption PR must list upstream PR/issue links or explicitly say unsubmitted/maintained in fork. |
| API changes | Record affected host adapters and the minimum usable revision; keep generated runner lockfiles and packaging tests in sync. |
| Security and dependencies | Review framework dependency advisories along with the application's shipping graphs; do not assume updating Patina's Cargo.lock updates every generated runner. |
| Succession | Keep build/run instructions, pin, patch rationale and native evidence in the repository. A new maintainer must not need another developer's mutable checkout or /tmp logs. |
| Recovery | Revert the pin only together with incompatible application/runner changes. Validate rebuilt packages; never retarget a public app tag. |

This is an operating ledger, not a claim that upstream has accepted the current
patch set. Use `git -C ../nsh show <pinned-sha>` and the adoption PR to establish the
actual patch inventory. Release candidate evidence must name both app and NSH SHAs.
