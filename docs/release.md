# Releases and CI

PRs and pushes to master run tools/check.sh on Linux and macOS, including
the headless app scenarios and portable crates' wasm builds.

The Android workflow (`.github/workflows/android.yml`) has two jobs behind one
lightweight change check, `tools/ci/android-changes.py`:

- `android` builds the arm64 native library and the Gradle host. On PRs it runs
  only when Android host files, Cargo manifests/lockfile, app build or package
  metadata/assets, packaging tooling, the NSH or Rust pin, or its workflow/setup
  actions change.
- `android-check` runs `tools/ci/check-android.sh`, a `cargo check` of the app
  library for `aarch64-linux-android`. On PRs it runs when Rust sources or
  manifests change and `android` does not already build them. The host checks
  never compile `cfg(target_os = "android")` code, so this is the check that
  stops a Rust-only change from breaking the Android build.

Both jobs run on every master push (which also keeps their build caches warm
for PRs) and can be started manually. The change check runs on every PR, so
both jobs always report: skipped when not needed, failed when change detection
itself fails. Either can therefore be a required check. Desktop packaging is
path-filtered at the workflow level and must not be required, since skipped
workflows can remain pending.

## Android smoke test

`.github/workflows/android-smoke.yml` builds an x86_64 debug APK whose wizard
starts with the mock server's playlist (`IPTV_RS_DEV_PLAYLIST`, with debug
assertions so it may reach the emulator's host address), generates the
synthetic fixtures, and runs `tools/ci/android-smoke.sh` on a tablet
(`pixel_tablet`, Google APIs) and a TV (`tv_1080p`, Android TV) emulator,
API 36. The script drives the app with remote keys and checks its log: first
run, adding the playlist, a channel in the guide, full screen, multiview with
a second player, background (the activity stops) and resume (both players
reopen), and back to the guide; it fails on a crash or a player load refused
outside its relay. Logs and a screenshot per step are uploaded as
`android-smoke-<form>`. It runs on PRs that touch the Android host, the app or
network crates, or its own inputs, and nightly. It is new and path-filtered:
do not make it a required check. The script's header says how to run it on a
local emulator.

## Coverage and fuzzing

Neither of these gates a merge. Both workflows are path-filtered on PRs, so do
not make them required checks.

- **Coverage** (`.github/workflows/coverage.yml`) runs `tools/ci/coverage.sh` on
  Linux for PRs that touch `crates/**` and on every master push. It measures the
  library crates' tests with `cargo llvm-cov` 0.9.1 (everything except
  `iptv-app`, whose scenario tests run the whole app), prints the per-file table
  in the job summary, and uploads `lcov.info` and an HTML report as the
  `coverage` artifact. The job is allowed to fail. Run it locally with
  `rustup component add llvm-tools` and `cargo install cargo-llvm-cov --version
  0.9.1 --locked`.
- **Fuzzing** (`.github/workflows/fuzz.yml`) runs `tools/ci/fuzz-imports.sh` once
  per fuzz target, in parallel. The targets are the `[[bin]]` entries of
  `fuzz/Cargo.toml` (what `cargo fuzz list` prints), so a new target is picked up
  without touching the workflow. PRs that change the playlist, guide or provider
  parsers, the import pipeline, the playback manifest rewriters or the harness
  get 60 seconds per target. The nightly Import stress workflow calls the same
  workflow with 300 seconds per target and saves each target's corpus to the
  Actions cache; every later run, including PR passes, starts from the newest
  saved corpus plus the committed seeds. Crashing inputs are uploaded as the
  `fuzz-<target>` artifact. The harness has its own `fuzz/Cargo.lock`, built
  with `--locked`; `tools/bump-version.py` keeps its workspace crate versions in
  step.

## Packaging and releases

Desktop Package checks likewise run only for package/build inputs on PRs and
support manual dispatch. Signed release packaging still runs on tags or manual
requests.

Android packages builds the arm64 native library and the Gradle host's debug APK/AAB,
verifies the APK signature, and uploads both as the android-debug artifact.
It uses a generated debug key and does not receive release secrets.
NSH is checked
out beside the repository at the commit in `nsh.rev` (from the fork's tv-keys
branch). That file is the only place the commit is named: both setup actions
read it through `tools/ci/pins.py`, and `tools/check.sh` refuses a `../nsh`
checkout at any other commit. Update `nsh.rev` when adopting fork changes.

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

Release runs repeat the host checks while packaging runs in parallel; the
signed Android build takes the place of the PR's debug build. Publication
waits for both the checks and packaging to pass.
Tags and manual runs default to platforms=android-desktop: Android arm64,
Linux x86_64 and macOS Apple silicon. Android produces one distribution-signed
APK and AAB for tablets and TVs. Both signing certificates are checked against
the configured keystore before upload. Manual runs also offer platforms=android
or platforms=desktop independently. platforms=all adds an iPad device IPA;
only that option requires Apple signing secrets and still needs iPad validation.

Linux outputs are .deb, .rpm, .pkg.tar.zst (a pacman package), and .AppImage.
Native packages depend on system GTK4, mpv and GStreamer. The AppImage bundles
linked libraries and GTK resources, and uses X11 (XWayland on Wayland desktops).
Its libmpv, with FFmpeg linked in, is the pinned Linux runtime
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

The macOS disk image and the Windows installer are platform-signed only when
their credentials exist. Without them the release still ships, and the
release notes start with a "Preview builds" section naming each unsigned
package (`tools/ci/publish-release.py notes`, from a record each signing job
writes). The README's download table and its "Opening the macOS and Windows
builds" section describe the previews. File names do not change.

- **macOS.** `tools/ci/package-macos.sh` checks for `MACOS_CERTIFICATE`: when
  it is set, the sign job imports the certificate into a temporary keychain,
  signs every Mach-O file in the app, the pinned libmpv runtime included, with
  the hardened runtime and a secure timestamp, checks that each one carries
  both, notarizes and staples the app (`tools/ci/notarize-macos.sh`), then
  signs, notarizes and staples the disk image. A partly configured set of
  Apple secrets fails the job. When the certificate is not set, the script
  says so and produces an ad hoc signed preview.
- **Windows.** The `windows-sign` job takes the installer that
  `windows-package` built and tested, on a fresh runner in the `release`
  environment. With all six `AZURE_*` secrets set it signs the installer
  through Azure Trusted Signing (now named Artifact Signing) with
  `Azure/artifact-signing-action` v2.0.0, pinned by commit, SHA-256 digest and
  an RFC 3161 timestamp, then requires `Get-AuthenticodeSignature` to report a
  valid, timestamped signature. With none set the installer is an unsigned
  preview; with some set the job fails. Only the installer is signed: the
  `iptv-app.exe` and `libmpv-2.dll` it installs are not (a follow-up). SmartScreen
  reputation attaches to the signing identity, so expect warnings to fade only
  after a signed release has been downloaded for a while.

As of v0.1.1 neither set of secrets is configured: every DMG and installer
released so far is unsigned, and Gatekeeper and SmartScreen warn about them.

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

The Android version code is the release workflow run number. Preserve its
monotonicity when migrating workflows or publishing to Play.
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
workflow_dispatch builds artifacts
without publishing; use platforms=desktop to validate desktop packages alone.
A manual run on master is also the dry run for the release-only steps: it
downloads the native source archives and writes the SBOM, and stops before the
checksum, attestation and publish steps. It signs with the release
credentials, so it is refused on any other ref (`tools/ci/release-plan.py`).
A tag publishes Android and desktop artifacts and SHA256SUMS to GitHub Releases only
after every job succeeds.

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

So a failed or cancelled publish job can simply be rerun: the draft is completed
and published. Nothing can be re-uploaded to a public release, because users may
have downloaded its files and the checksums name those bytes. To fix a
published release, bump the version and tag again; do not delete and recreate
the release or move the tag. An error while reading the release state also
fails the job rather than being read as "no release yet".

## Checksums, provenance and SBOM

Every release carries, next to the packages:

- `SHA256SUMS`: the SHA-256 of every other file in the release, in the format
  `sha256sum --check` reads.
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
# One file: built by this repository's release workflow from the release tag.
gh attestation verify Patina-<version>-linux-x86_64.AppImage \
  --repo teng-lin/patina-iptv \
  --signer-workflow teng-lin/patina-iptv/.github/workflows/release.yml \
  --source-ref refs/tags/v<version>

# Or verify the checksum file once, then check every file you downloaded.
gh attestation verify SHA256SUMS --repo teng-lin/patina-iptv \
  --signer-workflow teng-lin/patina-iptv/.github/workflows/release.yml
sha256sum --check --ignore-missing SHA256SUMS
```

Releases up to v0.1.1 predate this and have no attestations; their
`SHA256SUMS` is unsigned. An attestation proves which workflow run built a
file from which commit. It does not replace platform code signing: the macOS
and Windows packages are still not trusted by Gatekeeper or SmartScreen (see
below).

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

Each package carries its runtime's notices, licences, build configuration and
`SOURCE.txt` (the source archive's address and SHA-256) in
`notices/mpv-runtime` (`notices/libmpv` on Windows). The source archives hold
the downloaded upstream archives byte for byte, the licences, the lock and
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
   was published after the runner image was built) is fetched from Launchpad,
   which keeps every published version, and checked against its `.dsc`.
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

This does not submit to Play or App Store Connect. An IPA's distribution
profile determines where it can be installed; an App Store profile does not
make a GitHub download sideloadable. Without the Apple secrets above the macOS
image is ad hoc signed and NOT notarized, and without the Azure secrets the
Windows installer is unsigned: both are then released as previews (see
[Platform signing](#platform-signing)). Before calling a release stable,
configure Developer ID notarization and Windows code signing.

Before a stable release, also validate the RPM, pacman package, AppImage and
DMG on real desktops with a GPU and sound (CI covers installation and start-up
only), and signed iPad installation when enabling iOS. Android releases need tablet/TV smoke
tests on tab36 and tv36 with synthetic playlists. CI build and headless tests
do not replace those device checks.

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
There are no fallback keys: a changed tool input builds a new artifact. Release
packaging still disables caches, and signing jobs remain isolated. Cache sharing
is subject to GitHub branch scope; PRs can read a default-branch cache but cannot
warm caches for unrelated PRs. The default-branch Android workflow warms the
Linux CLI and Android runtime caches used by subsequent PRs. macOS CLI caches
are reused within a PR; dispatch Package checks on master to seed a macOS cache
that other PRs can restore, since desktop packaging has no master push trigger.

Only master writes the bulk Rust dependency caches. PRs restore those caches
and may save the much smaller finished-tool artifacts. This avoids duplicating
gigabytes of Rust dependencies per PR and evicting the shared tool caches.
Dispatching Package checks on master also seeds desktop packaging dependencies.

After changing this pipeline, compare a cold run with a second run at the same
NSH pin: the latter must skip both NSH build steps while still building the app,
generating notices, linting, packaging and verifying signatures. The expected
saving is the skipped compilation time minus the artifact restore overhead;
measure this in CI before claiming an improvement. Upstream Java-only runtime
preparation is a separate follow-up; this pipeline retains the complete runtime.

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
5. Run package CI for all platforms. Run the stale-lock regression with
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

Windows PR checks run `tools/build-windows.ps1 -Check`: the same lints, library
crate tests, app tests and portable wasm builds as `tools/check.sh`
(`tools/ci/test-check-parity.py` fails when the crate lists of the two scripts
differ). The Windows setup action uses the same Rust build cache as the other
hosts; only master saves it. Formatting and the workspace-wide `cargo deny` run
once, on Linux and macOS: neither depends on the host, and `cargo deny` without
a target already covers the Windows-only dependencies. `audit-runners.sh`
additionally audits the graph the Windows executable ships, for its own target.

Installer generation runs only in the
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
