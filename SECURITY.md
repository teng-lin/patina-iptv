# Security policy

## Reporting a vulnerability

Please report security problems privately, not in a public issue.

Use GitHub's private vulnerability reporting for this repository: open the
[Security tab](https://github.com/teng-lin/patina-iptv/security) and choose
**Report a vulnerability**, or go straight to
<https://github.com/teng-lin/patina-iptv/security/advisories/new>. The report
is visible only to you and the maintainers.

A useful report says which version and platform you used, what an attacker
can do, and how to reproduce it. Reproduce with synthetic data: the mock
servers and fixtures under `tools/` stand in for a provider. Do not include
real provider addresses, account names, passwords, portal MAC addresses or
channel lists; if a real playlist is the only way to show the problem,
describe its shape instead.

Patina is maintained by a very small team. We will acknowledge your report
as soon as we can, tell you what we plan to do, and credit you in the
advisory if you wish.

## Supported versions

Only the latest release receives security fixes. There are no maintenance
branches for earlier versions; a fix ships as a new release.

## Scope

In scope: this repository's code and the packages published on its
[Releases page](https://github.com/teng-lin/patina-iptv/releases), for
example credential or stream address leaks, flaws in catalog encryption,
unsafe handling of playlists, guides or backups, and weaknesses in the build
and release pipeline.

Out of scope: the content or behavior of IPTV providers (Patina ships no
channels), and vulnerabilities in bundled third-party libraries that have an
upstream fix, though we do want to know when a release ships an affected
version.

## How Patina protects data

[docs/security.md](docs/security.md) describes the security model: the
SQLCipher-encrypted catalog and where its key lives on each platform, the
transport rules for provider and playback traffic, and what backups contain.

## Verifying a download

Releases carry `SHA256SUMS`; the automated pipeline additionally requires build
provenance before publication. Releases up to v0.1.1 predate this guarantee.
v0.1.4 (including its separate macOS release) is a documented exception: the
November 17, 2026 audit found release-membership attestations, not release-workflow
build provenance. Do not infer build origin from a release-membership predicate.
[docs/release.md](docs/release.md#checksums-provenance-and-sbom) shows how to
verify that a file was built by this repository's release workflow.
