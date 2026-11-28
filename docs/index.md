# Documentation map

Start with the [README](../README.md) for installation, supported platforms and
synthetic development setup. The following documents own their respective contracts;
link to them instead of duplicating release or validation instructions.

| Need | Source of truth |
|---|---|
| Architecture, boundaries and portable state | [Architecture](architecture.md), [runtime API](runtime-api-v1.md) |
| Contributor subsystem map and checks | [Contributing](contributing.md) |
| CI selection, critical path and required-check migration | [Validation tiers](ci.md), [CI critical path](ci-critical-path.md) |
| Publishing, signing, updates, provenance and recovery | [Release runbook](release.md#release-contract-and-recovery), [app updates](app-updates.md), [macOS updates](macos-updates.md) |
| First mobile submissions and privacy disclosures | [Readiness record](release-readiness-222.md), [privacy worksheet](store-privacy.md), [Android Data safety (#221)](store-privacy-221.md) |
| Disposable emulator operation and evidence | [Android validation](android-validation.md) |
| Optimized Android release artifact checks | [Android release evidence](android-release-220.md) |
| iOS simulator build, entitlements and synthetic run | [iOS simulator validation](ios-validation.md) |
| Platform capability/support limits, casting and stream sharing | [Capabilities](capabilities.md), [casting](casting.md), [live stream sharing](live-stream-sharing.md) |
| Platform feature, operation and settings review | [Platform feature audit](platform-feature-audit.md), [interaction parity audit](interaction-parity-audit.md) |
| Native framework revision and update policy | [NSH pin](nsh.md) |
| Encryption, backups and trust boundaries | [Security model](security.md), [vulnerability reporting](../SECURITY.md) |
| Choosing, watching and returning on every input | [Interaction contract](interaction.md), [desktop viewing](desktop.md) |
| Platform Back/exit acceptance and the pending Samsung policy choice | [Back and exit validation](platform-back-validation.md) |
| Archived journey evidence for the interaction contract (#86) | [Issue #86 journey validation](https://github.com/teng-lin/patina-iptv/blob/cb6b863b4635f40367ef5dce804d8fefe6ff0819/docs/validation/issue-86-journey.md) |
| Remote interaction, accessibility and translations | [TV controls](tv-controls.md), [accessibility](accessibility.md), [localization](localization.md) |
| Performance and recovery measurements | [Performance journeys](performance-journeys.md), [recovery validation](runtime-recovery-validation.md) |

Subsystem ownership means responsibility for its contract and regression evidence,
not an invented roster. The contributor map identifies the code boundary; the
repository's [CODEOWNERS](../.github/CODEOWNERS) identifies actual review ownership.
Before handing off, record the candidate commit, targeted checks, unresolved device
limitations and any data migration/rollback constraints in the change description.
