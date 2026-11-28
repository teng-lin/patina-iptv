# Documentation map

Start with the [project README](../README.md) for installation, supported
platforms and synthetic development setup. Each document below owns its topic;
link to it instead of repeating it.

| Need | Source of truth |
|---|---|
| Architecture, boundaries, runtime API and shared live inputs | [Architecture](architecture.md) |
| Subsystem owners, checks and the NSH pin | [Development](development.md), [contribution policy](../CONTRIBUTING.md) |
| Choosing, watching and returning on every input | [Interaction contract](interaction.md) |
| Platform capabilities, limits and casting | [Platform support](platform-support.md) |
| Android, iOS, accessibility and performance verification | [Testing](testing.md) |
| CI selection and required checks | [Validation tiers](ci.md) |
| Publishing, signing, updates, store submission and privacy disclosures | [Release runbook](release.md) |
| Encryption, backups and trust boundaries | [Security model](security.md), [vulnerability reporting](../SECURITY.md) |
| Interface languages and translation | [Localization](localization.md) |

Subsystem ownership means responsibility for its contract and regression
evidence. The [development map](development.md) identifies the code boundary;
[CODEOWNERS](../.github/CODEOWNERS) identifies review ownership. Before handing
off, record the candidate commit, targeted checks, unresolved device limitations
and any data migration or rollback constraints in the change description.
