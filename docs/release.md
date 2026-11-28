# Releases and CI

[The validation guide](ci.md) is the authoritative matrix for PR, master, nightly
and release-candidate checks, lane selection, required-check migration and retained
evidence. The shared tested plan drives host checks; do not infer coverage from
an old workflow name or a skipped unselected lane. Documentation consumed by tests
is a build input. Coverage includes the application; its floors are enforced when
the coverage lane is selected.

Android emulator smoke uses a dedicated validation application ID and explicit
serials. For local execution use [disposable AVD validation](testing.md#disposable-android-validation),
which copies the complete profile and never resets original images. x86_64 debug
smoke is not proof of signed ARM release playback or physical accessibility.

Publishing a GitHub release does not submit to a store. The
[mobile store submission checklist](#mobile-store-submission) lists the
acceptance that stays with the owner, and the
[mobile store privacy disclosures](#mobile-store-privacy-disclosures) cover the
store forms.

## Contents

- Build and publish: [packaging and releases](#packaging-and-releases),
  [platform signing](#platform-signing),
  [promoting a tag](#promote-an-existing-tag-manually),
  [rehearsal](#release-rehearsal),
  [immutability](#published-releases-are-immutable),
  [provenance and SBOM](#checksums-provenance-and-sbom),
  [corresponding source](#corresponding-source-for-bundled-media-libraries)
- Build inputs: [NSH tool artifacts](#pinned-nsh-tool-artifacts),
  [locked package builds](#locked-package-builds),
  [Windows installer](#windows-installer-experimental)
- Recovery and updates: [release contract](#release-contract-and-recovery),
  [update feed](#authenticated-update-feed),
  [update build settings](#native-update-checks),
  [app updates](#app-updates),
  [upgrade acceptance](#upgrade-acceptance),
  [signed macOS updates](#signed-macos-updates)
- Stores: [submission checklist](#mobile-store-submission),
  [privacy disclosures](#mobile-store-privacy-disclosures)

## Packaging and releases

Desktop installer checks run on PRs that change packaging recipes, shipped
resources or bundled runtime inputs. The [validation guide](ci.md) describes lane
selection and manual dispatch. Signed release packaging runs on tags or manual requests.

Routine PRs compile both ARM targets and build one x86_64 debug APK when emulator
smoke is selected. `Android packages` can manually build an ARM debug APK for
investigation; debug AABs are not distribution evidence. Trusted master candidates
build unsigned release APK/AAB once, checking both ABIs, notices, Gradle unit tests
and release lint before promotion to isolated signing jobs.
The [Patina NSH fork](development.md#nsh-pin) at `../nsh-patina` uses the commit in `nsh.rev`.
GitHub Actions, the Windows build and `tools/check.sh` read the pin through
`tools/ci/pins.py`. Update `nsh.rev` when adopting changes from the fork.

Local Android builds: `tools/sync-host.sh` first (it builds the Rust library
and copies it and the app's assets into the Gradle host; Gradle does not run
it, so skipping it packages a missing or stale library), then
`./gradlew assembleDebug` in `hosts/android` for development, and `assembleProfile` for measuring (`tools/perf-android.sh`): a
non-debuggable release build that is also profileable from the shell and
signed with the debug key. The `release` build type itself is never
profileable and never falls back to the debug key: a release task fails unless
`KEYSTORE_PATH` (with `STORE_PASSWORD`, `KEY_ALIAS` and `KEY_PASSWORD`) names
the distribution key, `IPTV_UNSIGNED_RELEASE=1` asks for an unsigned build (as
`tools/ci/prepare-android.sh` does before the separate signing job), or
`IPTV_DEBUG_SIGNED_RELEASE=1` explicitly asks for a debug-signed one.

Release runs require full candidate validation before signing or publication.
Unsigned compilation and package checks run alongside candidate validation; only
the sealed successful result can be promoted.
Wait for the complete master Validation run, including `seal-candidate`, before
tagging that same commit. A successful exact-SHA candidate is verified and promoted
without repeating tests or compilation. If none is retained, the tag runs the full
validation and unsigned build path. Never promote PR or source-tree-equivalent
artifacts: the compiled application embeds its commit SHA. Distribution Android
version codes are stable across these workflows: `1,000,000 + major*1,000,000 +
minor*1,000 + patch` (minor/patch at most 999; Android maximum 2,100,000,000).
This starts above historical workflow-run codes; local development retains its
existing default unless CI stamps distribution metadata.
Publication waits for package verification and candidate-bound build provenance.
Tags and manual runs default to platforms=android-desktop: Android ARM64 and ARMv7,
Linux x86_64, macOS Apple silicon and Windows x64. Android produces one distribution-signed
APK and AAB for tablets and TVs, each containing both ARM architectures.
Packaging fails if either native library or the notices are missing. Both
signing certificates are checked against the configured keystore before upload.
The signed AAB for each release retains both R8 mapping files. For a direct APK
crash, extract `BUNDLE-METADATA/com.hantekllc.patina.direct-apk/proguard.map`.
For a store install, extract the bundle's own map from the same release.

```sh
unzip -p Patina-0.1.7-android-tablet-tv.aab \
  BUNDLE-METADATA/com.android.tools.build.obfuscation/proguard.map > mapping.txt
```

Manual runs also offer platforms=android
or platforms=desktop independently. platforms=all adds an iPad device IPA;
only that option requires Apple signing secrets and still needs iPad validation.

Linux outputs are .deb, .rpm, .pkg.tar.zst (a pacman package), and .AppImage.
The DEB, RPM and pacman packages use system GTK 4 and GStreamer and bundle
Patina's pinned libmpv in a private directory. The AppImage bundles the linked
libraries and GTK resources and uses X11 (XWayland on Wayland desktops).
All formats use the pinned Linux libmpv runtime with FFmpeg linked in
(`tools/mpv-runtime-unix`, fetched and verified by hash); packaging checks that
the bundled `libmpv.so.2` carries that runtime's build ID and that no system
FFmpeg or libmpv library came along. The other bundled libraries are Ubuntu
24.04's. It uses the host's OpenGL/EGL/GLES drivers and audio servers.
Linux compatibility starts at glibc 2.39 and GTK 4.14 from Ubuntu 24.04, not
every older distribution. `tools/ci/check-linux-packages.sh` inspects all three
native package formats and then installs and launches every package, each with
empty temporary app state under Xvfb:

- the DEB on the Ubuntu 24.04 runner itself;
- the AppImage on the runner, and again in a minimal Ubuntu container that has
  no system GTK, mpv or GStreamer (`Dockerfile.appimage-smoke`);
- the RPM in a Fedora 43 container (`Dockerfile.rpm-smoke`) and the pacman
  package in an Arch Linux container (`Dockerfile.pacman-smoke`). Each installs
  the package with the distribution's package manager, which resolves its
  dependencies, checks that the notices were installed, and launches the app.

These containers have no GPU and no audio device, so they prove installation
and start-up, not playback on real desktops.

macOS produces a self-contained .dmg with Patina.app and an Applications
shortcut. Packaging verifies the disk image, mounts it read-only, verifies the
app's signature and arm64 architecture, and launches it with empty temporary
app state. Bundled dependencies are checked for unresolved external references.
libmpv is the pinned macOS runtime (`tools/mpv-runtime-unix`): one
`libmpv.2.dylib` with FFmpeg linked in, referencing only `/usr/lib` and system
frameworks, built for macOS 14.0, the app's declared minimum. The bundler
checks that the copy in `Contents/Frameworks` has the runtime's Mach-O UUID.

### Platform signing

The reviewed `tools/ci/signing-policy.json` declares `signed` or `preview` for
each desktop platform. Signed mode requires every credential; preview mode
requires none. A partial or mismatched configuration fails before signing, even
when the certificate itself is missing. Rehearsals always require preview mode
and receive no credentials. Change the policy and provision the complete signing
configuration together; credential presence never silently downgrades a release.
For intentional previews, the
release notes start with a "Preview builds" section naming each unsigned
package (`tools/ci/publish-release.py notes`, from a record each signing job
writes). The README's download table and its "Opening the macOS and Windows
builds" section describe the previews. File names do not change.

- **macOS.** `tools/ci/package-macos.sh` checks for `MACOS_CERTIFICATE`: when
  signed mode is configured, the sign job imports the certificate into a temporary keychain,
  signs every Mach-O file in the app, the pinned libmpv runtime included, with
  the hardened runtime and a secure timestamp, checks that each one carries
  both, notarizes and staples the app (`tools/ci/notarize-macos.sh`), then
  signs, notarizes and staples the disk image. A partly configured set of
  Apple secrets fails the job. Explicit preview mode requires all Apple signing inputs to be absent and
  produces an ad hoc signed preview.
- **Windows.** The `windows-sign` job takes the installer that
  `windows-package` built and tested, on a fresh runner in the `release`
  environment. With all six `AZURE_*` secrets set it signs the installer
  through Azure Trusted Signing (now named Artifact Signing) with
  `Azure/artifact-signing-action` v2.0.0, pinned by commit, SHA-256 digest and
  an RFC 3161 timestamp, then requires `Get-AuthenticodeSignature` to report a
  valid, timestamped signature. Explicit preview mode requires none set; signed mode requires all six.
  A mismatch fails rather than falling back. Only the installer is signed: the
  `iptv-app.exe` and `libmpv-2.dll` it installs are not (a follow-up). SmartScreen
  reputation attaches to the signing identity, so expect warnings to fade only
  after a signed release has been downloaded for a while.

The checked-in policy intentionally selects previews for macOS and Windows.
Historical releases through v0.1.4 were documented as previews; this does not
assert current hosted secret configuration.

Create a GitHub environment named release and set these secrets for Android:
- ANDROID_KEYSTORE_BASE64, ANDROID_STORE_PASSWORD, ANDROID_KEY_PASSWORD,
  ANDROID_KEY_ALIAS: a persistent release signing key.

Keep every signing secret in that environment, not at repository or
organization level, and keep its protection rules (deployment branches and
tags, required reviewers): they decide which refs may sign with the release
credentials. A release without the Android secrets fails in the signing job
(`tools/ci/sign-android.sh`); it never falls back to a debug or throwaway key.
To sign and notarize the macOS disk image, also set (all or none):
- MACOS_CERTIFICATE_BASE64, MACOS_CERTIFICATE_PASSWORD, MACOS_SIGN_IDENTITY: a
  Developer ID Application certificate (base64 of its .p12), the .p12's
  password, and its identity name ("Developer ID Application: NAME (TEAM)").
- APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD: the account and app-specific
  password `notarytool` submits with.
To sign the Windows installer with Azure Trusted Signing, also set (all or none):
- AZURE_TENANT_ID, AZURE_CLIENT_ID, AZURE_CLIENT_SECRET: an Entra ID app
  registration with the "Trusted Signing Certificate Profile Signer" role on
  the certificate profile, and a client secret for it.
- AZURE_SIGNING_ENDPOINT: the signing account's regional endpoint, for
  example `https://eus.codesigning.azure.net/`.
- AZURE_SIGNING_ACCOUNT, AZURE_CERTIFICATE_PROFILE: the signing account and
  its public-trust certificate profile.

A conventional code-signing certificate (a PFX file) would work instead of
Trusted Signing but is not wired: it would need `signtool sign /fd SHA256 /tr
<timestamp URL> /td SHA256 /f cert.pfx /p <password>` in `windows-sign`, with
the PFX in a secret such as WINDOWS_CERTIFICATE_PFX_BASE64 and its password in
WINDOWS_CERTIFICATE_PASSWORD, written to `RUNNER_TEMP` with a restrictive
umask and deleted in an `always()` step, as the macOS keychain is. Most
certificate authorities now issue code-signing keys only on hardware tokens or
in cloud HSMs, which is why Trusted Signing was chosen.
For the optional all-platform run, also set:
- IOS_CERTIFICATE_BASE64, IOS_CERTIFICATE_PASSWORD, IOS_SIGN_IDENTITY,
  IOS_PROFILE_BASE64: a certificate and matching distribution profile for
  com.hantekllc.iptvrs. Ad hoc profiles install only on registered devices.

The Android distribution version code follows the version-derived formula
above. Preserve its monotonicity when migrating workflows or publishing to Play.
Run `python3 tools/bump-version.py patch` (or `minor`, `major`, or an explicit
version such as `0.2.1`) from the repository root using Python 3.11 or newer.
Add `--dry-run` to preview. The script also accepts an absolute path when
invoked from another directory.
The command updates `Cargo.toml`, every inherited workspace package version in
`Cargo.lock`, `tools/ci/package.Cargo.lock` and the fuzz harness's `fuzz/Cargo.lock`,
and the NSH packaging version in `crates/iptv-app/nsh.json` without
updating dependencies. It accepts increasing MAJOR.MINOR.PATCH versions.
Review and commit those changes, then tag that commit `v<version>` when ready.
The command does not commit, tag, or publish. Tag/version mismatch fails.
An ordinary workflow dispatch builds artifacts
without publishing; use platforms=desktop to validate desktop packages alone.
A manual run on master is also the dry run for the release-only steps: it
downloads the native source archives and writes the SBOM, and stops before the
checksum, attestation and publish steps. It signs with the release
credentials, so it is refused on any other ref (`tools/ci/release-plan.py`).
A tag publishes Android and desktop artifacts and SHA256SUMS to GitHub Releases only
after every job succeeds.

## Promote an existing tag manually

Use the Release workflow's `promote_tag` and `validation_run` inputs on
`master` when an existing tag has a successful exact-commit master Validation
run and its unsigned artifacts are still available. This also works if the
post-validation seal was skipped after a job retry.

```sh
gh workflow run release.yml --ref master \
  -f 'promote_tag=v<version>' -f 'validation_run=<master-validation-run-id>'
```

Keep `platforms=android-desktop` and leave rehearsal disabled. The plan
requires the aggregate validation and every unsigned packaging job to have
passed. It checks each retained archive against the digest reported by GitHub
before staging its files. The existing release approval, signing, installer
smoke tests, checksum and publication jobs then run for the tag. Promotion
shares a concurrency group with the tag-triggered workflow and refuses to
replace a public release.

For a manually promoted release, `candidate-manifest.json` records the tag,
the tagged commit, the validation run and the input digests. The final build
provenance identifies the `master` run that signed and published those
packages. Verify the manifest and checksum file with the release workflow
identity and `--source-ref refs/heads/master`. Check the manifest's `tag` and
`sha` against the peeled release tag, then use `SHA256SUMS` to check the
downloaded files.

## Release rehearsal

A manual run with `rehearsal` checked runs the whole release pipeline,
publication included, without the release credentials and without leaving
anything behind. It can start from any branch:

```sh
gh workflow run release.yml --ref <branch> -f rehearsal=true
```

What it does differently from a release:

- **Environment.** The two signing jobs (`sign`, `windows-sign`) run in the
  `release-rehearsal` environment instead of `release`. That environment must
  hold no secrets and needs no protection rules; create it once (Settings,
  Environments), or let GitHub create it empty on the first rehearsal. The
  `release` environment and its rules are not changed and are not used.
- **No release credentials.** Environment secrets of `release` are not sent to
  a job in another environment. As a second barrier, every `secrets.`
  expression in the two signing jobs is forced empty when
  `github.event_name == 'workflow_dispatch' && inputs.rehearsal`, the "is it
  configured" flags included, so a secret stored at repository level would not
  reach a rehearsal step either. `tools/ci/test-release-workflow.py` evaluates
  the workflow's expressions for each kind of run and fails if a rehearsal
  would receive a secret, or if any other run would sign outside `release`.
- **Android.** The APK and AAB still go through the real signing and
  verification (`tools/ci/package-android.sh`), with a key pair that
  `tools/ci/sign-android.sh` makes for that run alone: a PKCS12 store in
  `RUNNER_TEMP`, outside the checkout, behind a random password that is masked
  in the log, deleted when the script ends and again by the job's `always()`
  step. The script refuses to make it for anything but a manual rehearsal, and
  refuses to run a rehearsal that received any release credential. The job
  log names the throwaway certificate's SHA-256.
- **macOS and Windows.** Unsigned previews, as in any run without their
  credentials: the disk image is ad hoc signed and not notarized, the
  installer has no Authenticode signature, and the signing records and the
  draft's release notes say so.
- **Publication.** The publish job creates the lightweight tag
  `rehearsal-<run id>` at the run's commit, attests the files, uploads them to
  a draft release, never makes it public, then deletes the draft and the tag
  (`tools/ci/rehearsal.py`, which only removes a tag and drafts it can show to
  be its own). Checksums, SBOMs, source archives and provenance attestations
  are produced as in a release.

When rehearsing workflow changes before they reach the default branch, GitHub
may refuse creation of the throwaway tag with `403 Resource not accessible by
integration`. The publish job already requests `contents: write`; a commit with
changed workflow files also needs workflow-write authority, which the built-in
`GITHUB_TOKEN` cannot request. See [GitHub's token permission rules for release
creation](https://docs.github.com/en/rest/releases/releases#create-a-release).
Do not broaden repository permissions or relax release-tag protection.

A maintainer whose GitHub CLI authentication can write workflows can seed the
exact tag with the existing ownership-checking helper, then rerun only the failed
publish job. Read the run's full `headSha` and the publish job's `databaseId` from
`gh run view <run-id> --json headSha,jobs`, then run from that source checkout:

```sh
GH_REPO=teng-lin/patina-iptv python3 tools/ci/rehearsal.py create \
  rehearsal-<run-id> <full-run-commit-sha>
gh run rerun <run-id> --repo teng-lin/patina-iptv --job <publish-job-id>
```

This keeps the original run's package and signing artifacts. The helper refuses
a tag at another commit; publication accepts the existing owned tag and performs
the usual draft upload, verification and cleanup. The target must remain the
original run's commit. Do not substitute the default-branch commit or rerun the
packaging jobs just to recover publication.

Limits:

- A rehearsal covers `platforms=android-desktop` only (with the Windows
  installer). `platforms=all` is refused at the start of the run: an IPA
  cannot be signed without the distribution certificate and provisioning
  profile, and a rehearsal never receives them. iOS signing is validated with
  a manual `platforms=all` run on master. A partial selection is refused too.
- It proves the pipeline, not the credentials: whether the production
  keystore, the Apple certificate or the Azure profile are valid is only known
  from a run in the `release` environment.
- The Android packages of a rehearsal are in the run's `packages-android`
  artifact until it expires. They are signed with a key that no longer
  exists: they install, but cannot update a released build or be updated by
  one. Do not distribute them.
- Rehearsal attestations remain in GitHub's attestation store after the draft
  and the tag are gone. They name the rehearsal run and the branch it ran on,
  not a release tag, so `gh attestation verify --source-ref refs/tags/v<version>`
  does not accept them.

## Published releases are immutable

`tools/ci/publish-release.py` does the publishing, and it only ever writes to a
release that does not exist yet or is still a draft:

1. It reads the release's state. If the release for the tag is already public,
   the job fails before changing anything.
2. It writes `SHA256SUMS` for the files to release.
3. It creates a draft (or reuses the draft an interrupted run left), uploads
   every file to it, checks that it is still a draft, and makes it public once.

So a failed or canceled publish job can simply be rerun: the draft is completed
and published. Nothing can be re-uploaded to a public release, because users may
have downloaded its files and the checksums name those bytes. To fix a
published release, bump the version and tag again; do not delete and recreate
the release or move the tag. An error while reading the release state also
fails the job rather than being read as "no release yet".

## Checksums, provenance and SBOM

Every release carries, next to the packages:

- `SHA256SUMS`: the SHA-256 of every other file in the release, in the format
  `sha256sum --check` reads.
- One `*-media-runtime-sbom.cdx.json` per desktop media runtime and an
  `-android-classpath-sbom.cdx.json` for the Android release classpath
  (`tools/ci/sbom.py`).
- `Patina-<version>-sbom.cdx.json`: a CycloneDX 1.5 SBOM written by
  `tools/ci/sbom.py` (`cargo cyclonedx` 0.5.10). It lists the app crate's
  resolved `Cargo.lock` graph for all targets (normal and build dependencies,
  with registry checksums) and records the NSH commit. It does not list the
  platform host crates of the generated NSH runners (those are in
  `runner-components.json` inside each package) or the native media libraries
  (see "Corresponding source for bundled media libraries").
- Build provenance attestations, stored by GitHub rather than as release files.
  The publish job attests every released file and `SHA256SUMS` itself with
  `actions/attest-build-provenance`. The signature is keyless: Sigstore issues a
  short-lived certificate for that workflow run, so there is no signing key to
  keep or to leak. `SHA256SUMS` is not signed separately; its attestation is the
  signature.

To check a download, with the [GitHub CLI](https://cli.github.com/) installed:

```sh
# One file attested by a tag-triggered release.
gh attestation verify Patina-<version>-linux-x86_64.AppImage \
  --repo teng-lin/patina-iptv \
  --signer-workflow teng-lin/patina-iptv/.github/workflows/release.yml \
  --source-ref refs/tags/v<version>

# Or verify the checksum file once, then check every file you downloaded.
gh attestation verify SHA256SUMS --repo teng-lin/patina-iptv \
  --signer-workflow teng-lin/patina-iptv/.github/workflows/release.yml
sha256sum --check --ignore-missing SHA256SUMS
```

Releases up to v0.1.1 predate build provenance; their `SHA256SUMS` is unsigned.
v0.1.4 is a second historical exception: an audit on November 17, 2026 found
release-membership attestations (`release/v0.2`) rather than SLSA build
provenance on its assets, and the separate macos-v0.1.4 release is outside the
release workflow. Do not claim those downloads pass the documented build-origin
verification. Preserve their bytes and record the exception.

An attestation proves which workflow run built a file from which commit. It does
not replace platform code signing: preview macOS and Windows packages are still
not trusted by Gatekeeper or SmartScreen (see below).

## Corresponding source for bundled media libraries

Three packages bundle libmpv, FFmpeg and the libraries they use, which are
LGPL software. All three take them from Patina's own builds from pinned
sources, with FFmpeg configured `--disable-gpl --disable-version3
--disable-nonfree` and mpv `-Dgpl=false`; each build fails unless FFmpeg's
and mpv's generated configuration headers confirm it. Each runtime's
dependency release retains the exact sources it was built from:

| Package | Bundled libmpv/FFmpeg | Its source | Other bundled native libraries |
| --- | --- | --- | --- |
| Windows installer | `tools/mpv-runtime` (one `libmpv-2.dll`) | `mpv-runtime-sources.tar.gz` of release `mpv-runtime-<digest>`; see [Windows installer](#windows-installer-experimental) | the VC runtime |
| macOS disk image | `tools/mpv-runtime-unix` (one `libmpv.2.dylib`) | `mpv-runtime-macos-sources.tar.gz` of release `mpv-runtime-macos-<digest>` | normally none; any Homebrew library: `Patina-<version>-macos-native-sources.tar` |
| Linux AppImage | `tools/mpv-runtime-unix` (one `libmpv.so.2`) | `mpv-runtime-linux-sources.tar.gz` of release `mpv-runtime-linux-<digest>` | GTK, GStreamer and the host libraries libmpv uses, from Ubuntu 24.04: `Patina-<version>-linux-native-sources.tar` |

Each package carries its runtime's notices, licenses, build configuration and
`SOURCE.txt` (the source archive's address and SHA-256) in
`notices/mpv-runtime` (`notices/libmpv` on Windows). The source archives hold
the downloaded upstream archives byte for byte, the licenses, the lock and
the build scripts, and restore offline with `prepare.py prepare
--source-bundle`. See [`tools/mpv-runtime-unix/README.md`](../tools/mpv-runtime-unix/README.md)
for what each runtime contains and how to rebuild it.

The `.deb`, `.rpm` and pacman packages bundle no media libraries; they depend
on the distribution's.

How the per-release archives of the remaining system libraries are produced:

1. **Packaging records what was bundled.** `tools/ci/linux-native-notices.py`
   writes, into `usr/bin/notices/native/` of the AppImage, the inventory
   (`components.json`) and `sources.json`, naming the Ubuntu source package
   and exact version behind every bundled library except the pinned runtime
   (`dpkg-query`). `tools/bundle-macos.py` does the same in
   `Contents/Resources/notices/native/` for any Homebrew keg the app still
   links, recording each installed formula and the upstream archive, patches
   and resources it names with their SHA-256 checksums or Git commits; when
   only the pinned runtime is bundled it writes no such directory. Both also
   write `SOURCE.txt`, which tells a user where the source archive is.
   On macOS, packaging fails when a bundled library has no checksummed
   source; on Linux the check happens in step 2, when the sources are
   downloaded.
2. **The release job downloads exactly that list** with
   `tools/ci/native-sources.py`, reading the manifest back out of the finished
   package. On macOS it downloads each address (mirrors are tried too) and
   compares the SHA-256; Git sources are archived at the pinned commit. On
   Linux it runs `apt-get source --download-only <source>=<version>` after
   `tools/ci/enable-deb-src.sh` and checks every file against the checksums of
   the signed archive index. A version that has left the index (a newer update
   was published after the runner image was built) uses the historical `.dsc`
   from the official Ubuntu HTTPS archive or Launchpad. Each source file must
   match the size and checksum in that exact version’s description.
3. **The archive and a `.json` manifest are released** next to the packages and
   listed in `SHA256SUMS`. The manifest names every file with its address and
   SHA-256; the archive contains the same manifest, `SOURCE.txt` and, on macOS,
   the formulae.

A source that cannot be fetched, or whose checksum differs, fails the packaging
job, so no release is published without it. The Package checks workflow runs
the same collection on pull requests that change packaging and uploads only the
manifest.

Limits:

- The system-library archives cover the libraries that are copied into the
  package. Code that Homebrew or Ubuntu linked statically into those libraries
  from build-only dependencies is not traced.
- On macOS, formula sources that are computed while installing cannot be read
  from the formula text; packaging fails rather than guess. Sample data that a
  formula downloads only for its own test is left out.
- Never delete these release assets or the runtime dependency releases. Every
  distributed DMG and AppImage points at the archives of its own release and
  of its runtime.
- Releases up to v0.1.1 bundled Homebrew's and Ubuntu's FFmpeg, whose GPL
  configuration was never determined; their per-release source archives stay
  the corresponding source for them. A legal review of the distribution terms
  is still worthwhile.

An IPA's distribution profile determines where it can be installed; an App Store
profile does not make a GitHub download sideloadable. Without the Apple secrets
above the macOS image is ad hoc signed and NOT notarized, and without the Azure
secrets the Windows installer is unsigned: both are then released as previews
(see [Platform signing](#platform-signing)). Before calling a release stable,
configure Developer ID notarization and Windows code signing.

Before a stable release, also validate the RPM, pacman package, AppImage and
DMG on real desktops with a GPU and sound (CI covers installation and start-up
only), and signed iPad installation when enabling iOS. Android releases need tablet/TV smoke
tests on tab36 and tv36 with synthetic playlists. CI build and headless tests
do not replace those device checks.

Casting also needs physical receivers before a receiver or sender platform is
claimed as supported: rolling live HLS, channel change, stop, return to local
playback, the parental PIN prompt and audible output, recorded with receiver and
sender software versions.

The About row displays the compiled workspace version and the first eight hex
characters of the source commit, for example `v0.2.1(a3d4123f)`. Cargo tracks
Git HEAD and branch refs, including linked worktrees and detached CI builds.
Uncommitted edits retain the HEAD hash; builds from source archives without Git
show `v0.2.1(unknown)`. Package metadata keeps the plain numeric version required
by platform installers.

## Pinned NSH tool artifacts

Nonsigning CI caches the finished NSH CLI and Android runtime separately from
Cargo dependency caches. Exact identities include the NSH commit, Rust compiler,
host OS/architecture, build recipe, compiler flags and inherited Cargo configuration. The Android recipe pins
its ABI, NDK and SDK versions; its identity also includes the JDK and installed
build-tools inventory/revisions, since NSH selects the newest installed tools.
IPTV source and lockfile changes do not invalidate these tool artifacts.

The shared CLI action uses the same bootstrap profile for desktop and Android
(no LTO, optimization level 1). Caches restore after the general Rust cache and
save immediately after successful tool compilation, before app/package checks.
There are no fallback keys: a changed tool input builds a new artifact. Unsigned shipping
builds disable bulk Cargo dependency caches but reuse exact-key pinned finished
tools; signing jobs restore no caches and remain isolated. Cache sharing
is subject to GitHub branch scope; PRs can read a default-branch cache but cannot
warm caches for unrelated PRs. Default-branch candidate packaging warms the
Linux CLI and Android runtime caches used by subsequent PRs. Master candidate packaging also seeds
the macOS CLI cache that subsequent PRs can restore.

Only master writes the bulk Rust dependency caches. PRs restore those caches
and may save the much smaller finished-tool artifacts. This avoids duplicating
gigabytes of Rust dependencies per PR and evicting the shared tool caches.
Manual Package checks on master can still seed desktop preview packaging dependencies.

After changing this pipeline, compare a cold run with a second run at the same
NSH pin: the latter must skip both NSH build steps while still building the app,
generating notices, linting, packaging and verifying signatures. Measure the
saving in CI before claiming an improvement.

## Locked package builds

Windows installer distribution prerequisites are listed in the Windows section below.

Rust is pinned in `rust-toolchain.toml`. Source builds use `Cargo.lock`.
NSH generates a workspace containing additional platform runners, so its
resolved superset lives in `tools/ci/package.Cargo.lock`. Package scripts
source `locked-build-env.sh`: NSH honors `CARGO`, and that entry point copies
the committed runner lock and supplies `--locked`. Stale dependencies fail;
package jobs never update either lock.

To update after changing dependencies or the NSH revision:

1. Update the source lock intentionally with Cargo.
2. Run NSH once locally to generate `crates/iptv-app/.nsh/rust/Cargo.toml`.
   A stale-lock failure after generation is expected.
3. Copy the source `Cargo.lock` into that generated directory. Run plain
   `cargo metadata --manifest-path crates/iptv-app/.nsh/rust/Cargo.toml
   --format-version 1` to resolve its platform runners.
4. Review the dependency delta, then copy the resulting lock to
   `tools/ci/package.Cargo.lock` and commit both locks together.
5. Nightly validation runs the desktop package previews and master validates
   the unsigned shipping packages. Dispatch full Validation to run the
   previews before merging a dependency-only PR. Run the stale-lock regression with
   `IPTV_REAL_CARGO="$(command -v cargo)" python3 tools/ci/test-locked-cargo.py`.

`cargo fmt` has no `--locked` option and does not resolve dependencies;
all build and test calls in `tools/check.sh` require the lock.

`check-package-lock.py` requires shared source-lock dependency identities and
checksums to remain present in the runner lock, so lock-only updates cannot
silently leave packaged versions behind. `audit-runners.sh` audits each shipped
Rust runner with its actual target and features, and the app crate's own graph
for `x86_64-pc-windows-msvc`: Windows has no generated runner and ships that
executable, built from the source `Cargo.lock`. The generated virtual workspace
also contains JavaScript, web and Windows development hosts that are not shipped
by these package scripts; auditing that virtual root would enable their unrelated
features. Generated notices and inventories conservatively include that superset.

The build environment also exports the repository toolchain to commands that
change directory into NSH. Its patched xtask adds `--locked` to nested runtime
builds and metadata calls: Cargo resets `CARGO` inside `cargo run`, so relying
only on a wrapper environment variable would not protect those nested builds.

## Windows installer (experimental)

Windows validation follows [the shared tier matrix](ci.md#windows-and-linux-only-checks).

Installer generation runs in selected package checks and the
release workflow for tags and manual desktop selections, using
`tools/package-windows.ps1`. It produces one x64 `.exe` installer, with no portable
ZIP. The per-user install bundles libmpv and the VC runtime, adds a Start menu
shortcut, and preserves application data on uninstall. The `windows-sign` job
signs it when the Azure secrets exist ([Platform signing](#platform-signing)).
The package smoke check uses synthetic data and verifies the executable and window
icons, launch, clean exit, and uninstall.

The native runtime comes from the separate, manually dispatched
`.github/workflows/mpv-runtime.yml` dependency workflow (platform `windows`). It
prepares checksum-pinned source and toolchain archives, compiles in a pinned
Linux Docker environment with network access disabled, and tests the packaged DLL
on Windows with generated MPEG-TS and local HLS media. Publication depends on that
smoke test. Normal app PRs and releases download the selected runtime; they do not
rebuild the native graph.

The same workflow builds the Linux and macOS runtimes (platforms `linux` and
`macos`, from `tools/mpv-runtime-unix`). Linux compiles in the pinned Ubuntu
24.04 builder with networking disabled, plays synthetic MPEG-TS/HLS in the
builder, then again on a stock runner and runs the app's real-player tests
against the candidate with the system libmpv removed. macOS compiles on
`macos-15` with the Xcode named in `tools/mpv-runtime-unix/macos/xcode-version`,
isolated from Homebrew and the network, and plays the same media at build time
and on a fresh runner. Each publishes `mpv-runtime-<platform>-<digest>`; commit
its `runtime-<platform>.json` to `tools/mpv-runtime-unix/`. Until a platform's
manifest is committed, its app packaging fails and names the missing pin.

The dependency release tag includes a digest of the source lock and build scripts.
It contains the runtime ZIP, exact corresponding-source archive, `runtime.json`
and checksums. This ZIP is a build dependency, not a portable Patina distribution.
After publishing a new dependency release, review and commit its `runtime.json`
as `tools/mpv-runtime/runtime.json`. See the [native build instructions](../tools/mpv-runtime/README.md)
for local commands and the update procedure.

Installer generation verifies the bundled DLL against that committed pin and the
native component inventory. It copies all native licenses, the source lock,
inventory, build configuration and `SOURCE.txt` into `notices/libmpv`.
`SOURCE.txt` identifies the exact retained source asset for the installed DLL.

Never prune, replace or delete these dependency releases or their source assets.
Every distributed installer must retain access to its matching sources, including
after a newer runtime is selected. Keep notices and the same source availability
when redistributing installers. If a dependency release is incomplete or invalid,
correct the inputs and publish a new digest; do not overwrite published bytes.


## Release contract and recovery

Before a non-rehearsal build, `release-plan.py` requires the workspace version's
nonempty changelog section; a release tag must match that version. Record platform
preview/signing state and known compatibility changes there. Do not move a public
tag or publish manually while a job is still running.

`publish-release.py` verifies checksums and **each final artifact's** SLSA predicate,
repository release workflow, candidate commit and source ref before any draft
creation or upload. Missing provenance or a release-membership predicate fails.
Rehearsals verify the actual branch ref and remain drafts; they do not prove a
later tag or signing configuration. Post-publication verification downloads and
checks the public bytes again. Never bypass a failure by replacing public files.

The `verify-published` job then retains `update-release.json` in the Actions
artifact `verified-update-record-<run attempt>`. This is the per-release input
for the [authenticated update feed](#authenticated-update-feed). It identifies
the available viewer packages, their immutable URLs, exact sizes and hashes,
and each desktop package's signing status from the signing job. Clients require
signed update metadata before offering an installer.

### Authenticated update feed

The release workflow does not publish a feed. For a staging signing event, run
the following at the Patina checkout with a verified release record and a
checkout of the TUF repository. Start from its latest accepted target, serialize
signing events, and inspect the target diff before signing. The command creates
the first index when the file is absent.

```sh
python3 tools/ci/update-index.py ../update-feed/targets/index.json update-release.json
```

A retry or an older record preserves newer package entries. A conflict for
the same package and version, or different commits for one release across
platforms, aborts before replacing the file. The resulting
target is ready for the repository's signing event. Publish the signed target
and versioned metadata before the timestamp.

Before production signing:

- Host production metadata on its own static deployment, so a feed refresh
  cannot overwrite the privacy policy site, and record its stable URL.
- Record the offline root custodians and their signature threshold, and rehearse
  root rotation and recovery. Keep rehearsal keys and URLs separate.
- Schedule metadata refreshes between releases and verify the public result.
- Withdraw a bad target in newer metadata, and issue a higher-version repair for
  devices that already updated.

Unsigned candidate packages and their checked evidence are retained for 30 days
in Actions. A promoted release publishes `candidate-manifest.json` with checksums
and final-workflow provenance, retaining the unsigned input/check record after
the Actions bundle expires.
Use **Re-run failed jobs** to retry a failed validation, package, signing or
publication stage while preserving successful package outputs at the same source
SHA. If validation or packaging failed, the master seal job also reports a
failure before pulling any artifacts. The failed-job retry runs the seal again
once its prerequisites pass. GitHub increments the run attempt automatically.
A changed source requires new validation. If a failed job already uploaded part
of its output, remove only that job's unpublished Actions artifacts before
retrying it. Keep successful package jobs' artifacts.

If the run gains a new attempt after `seal-candidate` succeeded, for example by
retrying only another job, the manifest still names the previous attempt.
Selection deliberately rejects that mismatch. Before tagging, delete only the unpublished Actions artifact
named `release-candidate`, then rerun the `seal-candidate` job (using its job ID).
It checks and seals the retained unsigned packages under GitHub's new attempt;
it does not rerun the successful package builds. Never delete their signing inputs,
SBOMs, source archives, or any public release asset for this recovery. A full-job
rerun requires deliberate cleanup of unpublished Actions artifacts first.

If a build fails before publication, fix the cause and rerun the same candidate
only while its release is absent or a draft. A public release is immutable. For a
harmful release, stop recommending it in documentation, publish an advisory with
exact affected versions/artifact hashes, and ship a corrected version. Do not
silently delete or rewrite artifacts/checksums or retarget the tag. A deliberate
asset withdrawal requires a separately reviewed incident decision and a retained
public explanation; this runbook does not automate deletion.

Before upgrading, retain an application backup and recordings separately using
synthetic data for validation. Catalog migrations can be irreversible: installing
an older binary over a newer encrypted catalog is not a supported rollback. To
recover, stop the app and recording host, keep the newer profile intact, and restore
a backup compatible with the older version into a separate profile with its
corresponding platform key. If key or schema compatibility is unknown, remain on
the newer version and seek a fixed release. Do not promise downgrade support from
fresh-install package smoke. Validate install-over-previous, catalog reopen,
recording retention and backup restore before claiming an upgrade path passed
([upgrade acceptance](#upgrade-acceptance)).


Desktop release artifacts also retain a `*-build-environment.json` beside their
SBOMs: candidate commit, lock/pin file hashes, compiler/tool versions and installed
apt/Homebrew package versions. These inventories aid reconstruction and diagnosis;
they neither freeze the host repositories nor promise byte-identical rebuilds.


### Native update checks

For a rehearsal build, set `PATINA_UPDATE_FEED` to the HTTPS directory serving
the staging repository and `PATINA_UPDATE_ROOT` to its out-of-band public TUF
root file before compiling Patina. Set both or neither; the feed address is an
HTTPS directory ending in `/` that serves `metadata/` and `targets/`.
Disclose the selected host in the privacy notice before distributing a
configured build. GitHub discovery, inline release notes and manual package
links work without that pair. The feed authorizes installation inside Patina.
[App updates](#app-updates) describes viewer behavior, failure handling and
platform limits. Existing installations without the checker need one manual
upgrade to acquire it.

To rehearse direct APK installation, compile the staging build with
`PATINA_APK_HANDOFF=1` and the staging feed and root above. Gradle's
`directRelease` variant carries the system installer permission and private
handoff activity. The normal release bundle goes through the store. Use the
About action to download the selected build and confirm installation. An
active recording defers the handoff. Keep production activation gated on the
signed [install-over-previous exercises](#upgrade-acceptance).

Set `PATINA_APPIMAGE_HANDOFF=1` for AppImage rehearsals with the staging feed.
AppRun records its image path’s device and inode at startup. About can replace
that file after confirmation or save the verified image to a new name on a
writable filesystem. Patina keeps playing until the viewer restarts it. Each
replacement preserves the old image beside the installation as
`<file>.<previous-version>.previous`, adding its inode to the name if an earlier
backup already occupies it. Keep that file until the new version opens the
existing library, then remove the backup when it is no longer needed. Restoring
the executable alone cannot reverse a database migration. A hard interruption
can also leave a hidden `.patina-update-*` directory beside the chosen file.
After restarting, inspect its contents before removing it.

For a signed Windows rehearsal, set `PATINA_WINDOWS_PUBLISHER` at compile
time to the exact subject distinguished name of the approved Authenticode
publisher. Obtain it independently from the signing administrator. About offers
setup only when the verified release record also reports a signed installer.
The verifier checks Authenticode, the timestamp and the authorized SHA-256
while holding the EXE open against replacement. After confirmation the helper
waits for Patina to exit before starting the per-user wizard. Setup cannot ask
Restart Manager to close another Patina session. Preview releases continue to
show their release notes.

## App updates

Settings → About → Check for updates compares the installed version with the
highest stable semantic version published on GitHub. Drafts and prereleases do
not count. A newer local build is never offered a downgrade. The panel shows
installed/latest versions, the release date, the last successful check and
scrollable release notes. Failed checks remain failures, with the previous
successful result available; a missing package is reported separately.

Checks run in the foreground. The frequency choices are Never, On launch,
Daily, **Weekly (default)** and Monthly. Monthly means the same day of the next
calendar month, clamped to that month's last day. Manual checks work at every
frequency. A failed automatic check uses bounded retry backoff. Skip this version
suppresses its automatic notice; a manual check can still review it. Later
dismisses the notice without losing the result. A notice waits until fullscreen
playback and other panels have closed and can be reviewed with a TV remote.

GitHub requests use a separate client: update requests carry no provider
headers, playlist addresses, credentials, history or device identifiers. Release
notes render as text. Package links must match this repository's release and the
installation's platform, architecture and package format. Store Android builds
do not offer a direct APK. iOS/iPadOS, Tizen and terminal builds do not use this
checker. See the [privacy policy](privacy-policy/index.html).

### Download and installation

Every supported matching package has a manual download route. Installation
inside Patina requires additional authentication and platform capabilities:

| Installation | Behavior |
| --- | --- |
| Direct Android APK | Opt-in builds download a TUF-authorized APK, verify size and digest, then ask Android's installer to confirm its package, signing identity and higher version code. Store builds stay on their distribution channel. |
| Windows per-user setup | Opt-in builds verify TUF authorization, Authenticode publisher and timestamp before asking to close Patina and start the installer. Unsigned previews use manual download. |
| macOS app | Opt-in Developer ID builds use pinned Sparkle and its signed appcast for download, installation and relaunch. Current unsigned previews download the DMG manually. See [macOS configuration](#signed-macos-updates). |
| Linux AppImage | Opt-in builds verify the full image and ask before replacing a writable installation or saving elsewhere. The previous image remains available for recovery. Restart manually to use the replacement. |
| Linux DEB, RPM, pacman | Download the matching package and apply it using the system package tool. GitHub availability does not promise availability from a configured package repository. |

Authentication failure never enables installation. An explicit **Download
package** browser action remains available when an installer feed or appcast is
unavailable. Downloading through that action requires the viewer to install the
package using the platform's normal tools.

Android, Windows and AppImage in-app downloads show progress and can be
canceled. Closing the update panel lets an accepted download finish; the runtime
retains the verified file and offers it again without downloading twice. Explicit
cancellation removes their partial and retained stages. These runtime stages are
private temporary files and do not survive app exit. Recording activity is checked
again before installer handoff. Deferring installation preserves the staged file;
it does not silently stop a recording.

### Production activation

Production activation still requires a published [authenticated
feed](#authenticated-update-feed), trusted keys, [platform
signing](#platform-signing) and [install-over-previous
acceptance](#upgrade-acceptance). The unsigned macOS and Windows previews do not
become signed merely by enabling this UI. Enable each platform's installation
route only after its own acceptance passes. Build settings are under [native
update checks](#native-update-checks).

### Update verification

Existing test executables cover actual loopback GitHub HTTP discovery, a signed
synthetic TUF repository, streamed download and byte/digest verification,
truncated/corrupt/oversized refusal, progress, cancellation, retained stages and
cleanup. Schedule tests cover frequency boundaries, persistence, skipped and
unseen versions, retry backoff and clock rollback. App journeys exercise About,
inline notes, close/reopen, failed-check recovery, delayed Android channel
reports, physical remote notice navigation, compact large-text layout and saving
Monthly. Native macOS fixture tests exercise the AppKit bridge's
generation fencing, restart approval and cancellation.

```sh
cargo test --locked -p iptv-runtime --features test-support --lib app_updates::
cargo test --locked -p iptv-app --features test-support --test app app_updates::
cargo test --locked -p iptv-app --features test-support --lib screens::settings_view::updates::view::
python3 tools/ci/test-macos-dmg.py
```

`python3 tools/ci/test-macos-dmg.py` checks absent and partial Sparkle
configuration, transport and key validation, the pinned archive boundary, version
preservation and disabled automatic installation. On macOS it also compiles the
bridge against AppKit/Security and exercises preview refusal, version agreement,
off-main-thread requests, shell-authorized relaunch, root disposal, cancellation
and extraction races, prepared Skip replies, retry reauthorization, deferred
recovery, and a child process that cancels Cocoa termination and exits normally.

### Upgrade acceptance

Fixtures prove discovery, verified download and native contracts. They do not
replace this release gate: use two packaged releases, entered through the normal
About action, and record the signing status, install location, package format,
and for Android the signer, package ID and both version codes. Verify that the
synthetic library, favorites, settings and encrypted catalog survive, that
cancellation and failure leave a launchable app, and that an active recording
survives a deferred update. Include a database migration in the upgrade pair
before treating restoration of an older executable as recovery.

- **Android:** a phone in portrait and landscape, a tablet, and a TV using only
  the remote; two production-signed APKs on representative ARM hardware. Cover
  permission denial, return from Settings, confirm, cancel, retry and relaunch.
- **Windows:** as a standard user, cover cancel, locked files and reopening the
  library.
- **AppImage:** cover interrupted downloads, insufficient space, read-only paths
  and a failed launch.
- **DEB, RPM, pacman:** install over the previous version.
- **macOS:** two Developer ID builds with the same updater key. Cancel one
  download, reject a tampered archive, complete the upgrade and verify the new
  version after relaunch, then reopen the library, keep recordings and exercise
  backup restore. Start a recording while the install dialog is open and verify
  that relaunch waits. Cancel at the download/extraction boundary and dispose the
  root before approving a prepared update. Decline an immediate quit and confirm
  that the recording host resumes, the deferred notice appears and a later check
  resumes only the selected version. Exercise an unwritable installation and an
  offline appcast.

## Signed macOS updates

Patina includes an opt-in bridge to Sparkle **2.10.1**, pinned to the upstream
archive SHA256 in `tools/ci/macos-sparkle.py`. Before enabling production
updates, provision an Ed25519 update-signing key in the protected release
publisher, establish the HTTPS appcast URL, and complete the Developer ID
signing/notarization setup. The update key is separate from the Apple signing
identity and the TUF key used by other installer adapters. No production updater
key, appcast URL or Developer ID credential is supplied by the repository. Do
not generate a throwaway key for production or alter the preview signing policy
to make capability checks pass.

Set repository **variables** `PATINA_SPARKLE_FEED` and
`PATINA_SPARKLE_PUBLIC_KEY` together. The unsigned macOS candidate job downloads
the pinned upstream archive; bundling verifies its hash and framework signature,
copies the framework and its license, and seals the public configuration inside
the app. Partial configuration fails packaging. Neither variable is a secret.

For a local candidate build, the equivalent command is:

```sh
PATINA_SPARKLE_ARCHIVE=/outside-checkout/Sparkle-2.10.1.tar.xz \
PATINA_SPARKLE_FEED=https://updates.example.test/patina/appcast.xml \
PATINA_SPARKLE_PUBLIC_KEY="$PATINA_PUBLIC_UPDATE_KEY" \
bash tools/package-macos.sh
```

The protected signing stage must sign the nested Sparkle helper bundles inside
out, preserving their entitlements, then sign and notarize the enclosing app and
DMG. Patina enables the action only for a running Developer ID application with
complete public configuration and the pinned bundled framework. An unsigned
candidate remains manual even when it contains Sparkle.

After the final DMG is signed, notarized and verified, publish that immutable DMG
to its GitHub release. Generate an appcast from those exact bytes using the
**pinned** upstream `bin/generate_appcast`; use its `--ed-key-file` option with a
protected file outside the checkout. Match `--download-url-prefix` to the exact
GitHub release asset directory and include release notes beside the DMG. Publish
the resulting appcast at `PATINA_SPARKLE_FEED` after the referenced assets exist.
Retain the appcast and artifact hashes as release evidence. Never place the
private key in repository variables, candidate artifacts, app resources or logs.

The release workflow does not publish an appcast. Missing or mismatched appcast
entries fail installation; GitHub notes and the manual package route remain
usable.

Upstream references: [programmatic setup](https://sparkle-project.org/documentation/programmatic-setup/),
[publishing](https://sparkle-project.org/documentation/publishing/), and
[code signing](https://sparkle-project.org/documentation/#6-code-sign-your-app).

### macOS lifecycle

GitHub release discovery and release notes work independently of Sparkle.
Ad hoc signed previews offer their matching DMG for manual installation; they
cannot safely replace themselves through the signed channel.

Sparkle supplies download progress, cancellation, Ed25519 verification,
app replacement and relaunch. Patina owns check frequency and requires an
explicit installation action. Sparkle's own automatic checks, automatic
downloading and installing and JavaScript release notes are disabled. The
appcast version must match the GitHub version the viewer chose, including for a
resumed update. A forwarding public `SPUUserDriver` keeps Sparkle's standard UI
and captures its check and download cancellation blocks; Patina uses no private
Sparkle driver API or AppKit delegate replacement.

Relaunch waits until the recording host can close safely and flushes watch time.
AppKit publishes a token-scoped restart request; the owning NSH shell task checks
the runtime and approves it. Native callbacks never access shell state, and
disposing that root revokes queued approval.

Cancellation ends when extraction begins. Cancel and Skip become unavailable
during extraction, preparation and handoff; Later can still close the app panel.
The Ready reply is held until the recording guard approves installation. Root
disposal before approval answers that one-shot reply with Skip, which cancels the
prepared installation; disposal during extraction retains only native state until
Ready can be answered that way.

Once approved, Sparkle's external installer owns the handoff. A prepared update
can install on a later normal quit, even when immediate termination is declined:
the external agent observes process exit and does not require Cocoa's
WillTerminate notification. If the approved process remains alive after 60
seconds, Patina releases the native UI and controller, restores the recording
host and reports that installation was deferred. This does not remove the
prepared update; checking again can resume it through the same version and
recording guards.

References for the pinned version:
[SPUUserDriver](https://github.com/sparkle-project/Sparkle/blob/2.10.1/Sparkle/SPUUserDriver.h)
and [InstallerProgress](https://github.com/sparkle-project/Sparkle/blob/2.10.1/Sparkle/InstallerProgress/InstallerProgressAppController.m).

## Mobile store submission

These items are open until verified against the exact candidate and the owner's
store accounts ([#220](https://github.com/teng-lin/patina-iptv/issues/220),
[#222](https://github.com/teng-lin/patina-iptv/issues/222)). Privacy forms and
policy links ([#221](https://github.com/teng-lin/patina-iptv/issues/221)) are
under [mobile store privacy disclosures](#mobile-store-privacy-disclosures).
Record results against the artifact hash and source commit, keeping emulator and
physical-device evidence separate. No account details, signing identities or
provisioning profiles belong in public evidence.

Review notes describe normal entry: launch, add a synthetic playlist through
playlist setup, open its guide and play authorized demo media. The owner hosts
that fixture publicly before submission; do not offer a review-only bypass or a
private fixture address. Describe only what the submitted platform supports:
the iPad build does not record, and scheduled recording is unavailable.

### Android

- Run [signed release acceptance](testing.md#signed-release-acceptance)
  on a physical tablet and TV: normal launcher and first run, playlist import,
  playback and XMLTV refresh, picture in picture on handheld and its absence on
  TV, background and return, backup and restore, recording, document pickers and
  install-over-previous.
- Confirm that each form factor opens its own launcher activity, that a stale or
  alternate-launcher start redirects to it, and that notifications return to it.
- In Play Console: developer identity and verification, package registration for
  `com.hantekllc.iptvrs`, Play App Signing (confirm the owner-approved upload
  certificate), IARC content rating, launch territories and trader status. Where
  the personal-account closed-testing rule applies, verify at least 12 opted-in
  testers for 14 consecutive days and approved production access.
- Check the uploaded version's DEX size and optimization, obfuscation and
  shrinking percentages in App bundle explorer, and Android vitals memory across
  RAM tiers and app states, against Google's
  [technical quality requirements](https://support.google.com/googleplay/android-developer/answer/17492799).
  Record unavailable data explicitly; a local R8 build does not establish them.
  Keep the exact AAB, which retains the R8 maps.

### iPad

- Confirm program membership, agreements, the app record for
  `com.hantekllc.iptvrs`, the distribution profile, a signed upload and its
  validation, the age rating and launch territories.
- Inspect the **signed final** bundle:
  `python3 tools/ci/inspect-ios-bundle.py <path to Patina.app>`.
- Classify the cryptography of the app and its linked libraries (SQLCipher,
  Rustls, MPVKit inputs) and record the matching export-compliance answer. The
  plist does not set `ITSAppUsesNonExemptEncryption`, and HTTPS use alone does
  not justify answering `false`.
- Add the missing `PrivacyInfo.xcprivacy` declarations after auditing
  required-reason API use by the finished binary and its dependencies (file and
  disk metadata, boot time, `NSUserDefaults`, including NSH's read of the
  system-written `AppleTemperatureUnit`, which reason `CA92.1` does not cover).
  App Privacy answers do not replace these manifests.
- Install the signed candidate on a physical iPad and verify launch, source
  import, playback, background and return, and
  **Settings → About → Privacy policy**.

## Mobile store privacy disclosures

The public [Patina IPTV Privacy Policy](https://teng-lin.github.io/patina-iptv/privacy-policy/)
is published from [this repository](privacy-policy/index.html) by GitHub Pages.
Android and iPad expose that URL at **Settings → About → Privacy policy**. Use
the same URL in Play Console and App Store Connect.

### Release behavior

| Data or feature | Android tablet and TV | iPad |
| --- | --- | --- |
| Playlist and provider credentials, headers and guide sources | SQLCipher catalog, with key protected by Android Keystore. Provider authentication and requests go directly to the configured service. | SQLCipher catalog, with key protected by the Apple Keychain. Same direct requests. |
| Playlists, guide, artwork and playback | Imported documents and catalog data are local. Requests go to the configured hosts or content hosts for streams, guide data and artwork. Media3 receives media through an app-local relay. | Same content requests. MPVKit receives media through the app-local relay. |
| Preferences, favorites, search history, viewing history and playback progress | Stored locally. The watch history database and search terms are not sent to providers. A provider can infer viewing from stream requests. | Same. |
| Backups and restored data | User-requested exports are unencrypted and may contain provider credentials and history. Android uses the system's document picker. The selected storage provider may receive the file. Automatic app-data backup and device transfer are excluded. | In-app backup and restore are unavailable. Private app data is excluded from automatic system backup. |
| Recordings | Media on device, with optional user-selected exports and a foreground service while a recording runs. Recording starts only when the viewer chooses Record; nothing is scheduled. The app fetches media from the selected provider. | No recording ([capabilities](platform-support.md#record-now-and-the-recordings-library)). |
| Casting | Phones and tablets can send a live channel to a Chromecast or AirPlay receiver on the local network. The receiver gets the media over plain HTTP; provider addresses and credentials stay in the app's relay. Android TV has no cast sender. | AirPlay to a receiver chosen in Apple's picker, with the same relay boundary. |
| Diagnostics and accounts | Local app and platform logs. **Send a diagnostic log** in Settings exports a redacted log only when the viewer asks, to a destination they choose; TV images without a picker or share target do not offer it. No automatic Patina diagnostics upload, analytics, ads or Patina account. Google may collect diagnostics under its own OS or store settings. | Same app behavior. Apple may collect diagnostics under its own OS or store settings. |
| Bundled components | NSH, AndroidX/Media3, SQLCipher and Rust libraries. See the Android classpath and Rust SBOMs for the release graph. | NSH, MPVKit (including libmpv and FFmpeg), SQLCipher and Rust libraries. See the verified MPVKit inventory and Rust SBOM. |

Portal authentication also transmits viewer-supplied MAC, serial and device
identifiers when configured, with portal language/time zone and protocol
profile fields. These are provider identity settings, not a hardware MAC read
from Android or iPad. Include them when assessing identifier disclosures.

The [security model](security.md) documents the encryption boundaries, explicit
HTTP support and export handling. The policy covers transfers to configured
providers, provider-selected hosts, external players and storage destinations.

### Listing decisions to verify on the submitted builds

These are proposals from a source inspection, not saved or published store
declarations. [#221](https://github.com/teng-lin/patina-iptv/issues/221) stays
open until the candidate builds, both listing fields and both completed forms
have been checked.

- **Google Play Data safety.** Local-only history, preferences and cached data
  are outside data collection. Off-device provider requests can carry user IDs,
  account credentials, connection headers and signals about which content
  the user requested, so do not select a blanket no-data answer. Review the
  applicable categories for every track and
  feature. Google's exception for an expected user-initiated transfer applies
  to sharing and does not remove collection from the form. Review automatic
  guide refresh and a recording that runs on in the background against that
  distinction. Provider
  retention is unknown, so do not assume ephemeral processing. Explicit HTTP
  is supported, so the app cannot declare that all data is encrypted in
  transit. Patina has no account creation or deletion flow.
  [Google Play Data safety guidance](https://support.google.com/googleplay/android-developer/answer/10787469)

  | Play field or category | Proposed answer or open question |
  | --- | --- |
  | Personal info, User IDs | Provider account names and IDs, for app functionality. Credentials can also occur in addresses, headers and exported archives; resolve any further authentication category in the form. |
  | Device or other IDs | Configured portal MAC, serial and device identifiers. User entry does not make their transmission local-only. |
  | App interactions | Provider-visible content requests. Local history, favorites and progress leave only in an explicit backup export; assess that transfer separately. |
  | Search history, files, videos | No normal upload found. Resolve how the form treats backup and recording exports and external-player transfers. |
  | Location | No location use found. IP visibility alone does not prove location inference. |
  | Crash logs, diagnostics, ads, analytics | No automatic upload or advertising purpose found. Confirm against the packaged SDK graph and the user-initiated log export. |
  | Deletion | Local clear, remove and uninstall only; nothing deletes provider-held data. Verify the form's deletion choices and support process. |

- **Apple App Privacy.** Source review found no Patina-operated data collection
  or tracking by bundled components, so the proposal is **Data Not Collected**
  with **no tracking**, conditional on the packaged iPad app, its SDKs and their
  privacy manifests, and on there being no developer or partner retention path.
  Apple asks about data accessible to the
  developer or integrated third-party partners beyond the time needed to
  service a request. The viewer's independently chosen provider can retain
  its own requests and is covered in the privacy policy.
  Do not copy the Google answers into Apple's form. Export compliance and
  privacy manifests are [separate submission items](#ipad).
  [Apple App Privacy guidance](https://developer.apple.com/app-store/app-privacy-details/)

### Owner follow-through

1. Reconcile these answers with the exact submitted builds, every active Play
   track, the runtime SBOMs, the iPad privacy manifests and observed synthetic
   traffic.
2. Enter the policy URL in Play **App content → Privacy policy** and App Store
   Connect **App Privacy → Privacy Policy URL**, including required
   localizations, and check that the policy's app name matches each listing.
3. Finish both forms, verify their previews and listing links, and record only
   sanitized field, status and date evidence.
4. After a policy change deploys, check that the URL is reachable anonymously and
   matches the source. On the submitted Android tablet, TV and iPad artifacts,
   open it from the app; on a TV without a browser, check that the address is
   shown instead.

Keep release artifact, SDK inventory and console evidence with the store
submission record, outside the repository.
