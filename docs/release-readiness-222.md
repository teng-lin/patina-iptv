# First-submission readiness evidence (#222)

This pass began on November 24, 2026 from `origin/master` `0c76709d`, with
NSH pinned to `413cfb65bf87a7bd9937f223fdc56896d8a718c8`. It records partial
progress on [#222](https://github.com/teng-lin/patina-iptv/issues/222), not
authorization to submit a first release. The [privacy evidence](store-privacy.md)
and [optimized Android validation](android-release-220.md) have
separate owners. All fixture journeys use generated playlists and EPG, local
mock servers and authorized demo media. No store account information, signing
identities, provisioning profiles or provider data belongs in public evidence.

## Android source and merged-release evidence

The handheld launcher remains `.TvActivity`, with `LAUNCHER` and
`android:supportsPictureInPicture="true"`. The independent `.LeanbackActivity`
declaration has `LEANBACK_LAUNCHER` and **no** `supportsPictureInPicture`
attribute. It subclasses the same host to preserve lifecycle, input and the
persistent player surface. Both declarations retain `singleTask`, resize and
configuration-change handling. Recording and external-player notifications
return to the matching device launcher. A stale shortcut or explicit launch
of the other component redirects before loading Rust or mounting a player.
The redirected instance skips host callbacks and teardown, so it never calls
uninitialized host objects or native methods. A Leanback component launched
on handheld likewise redirects to the PiP-capable handheld component.
The runtime's existing TV PiP exclusion remains in `PipHost`.

Local release host checks passed: `:app:testReleaseUnitTest` (54 tests) and
`:app:processReleaseMainManifest`, using JDK 17 and Android SDK 36. The generated
merged release manifest independently confirmed:

| Activity | Launcher category | PiP attribute |
| --- | --- | --- |
| `com.hantekllc.iptvrs.host.TvActivity` | `LAUNCHER` | `true` |
| `com.hantekllc.iptvrs.host.LeanbackActivity` | `LEANBACK_LAUNCHER` | Absent |

The existing `HostPolicyTest` harness covers the two declarations, matching
lifecycle attributes, device launcher selection and stale-shortcut redirect
selection. Source and merged-manifest checks do not establish the manifest in
a signed AAB or prove playback PiP on the changed host.

### Android artifact snapshot — November 25, 2026

This artifact snapshot was recorded before the refresh onto master `39d0a09c`
(#325). The #220 lane keeps these distinct artifacts and results. Later device
results belong in the PR evidence, with the artifact identity, rather than being
inferred from this snapshot.

| Artifact | Verified observations and limits |
| --- | --- |
| Historical signed v0.1.8 ARM64 APK, source `469f3683505abce9ab50d8aaa651448510a29865`, NSH `fd847c95917edbac95368ae28b7c19fe3ee01470` | APK SHA-256 `8d23cad513d9a4cf68f070eddefcc66d40c3ee9b0b6b1babb24d35681c91428a`. Signature and corresponding R8 mapping were verified by #220. Normal tablet and TV resolved launches passed. Tablet normal file import of a generated one-channel playlist and decoded, advancing authorized demo video passed. Tablet PiP entered through both the native window key and visible control; its visible playback counter advanced, the task was pinned, and normal return retained the process. The emulator had no audio output, so this does not establish audible playback. Remaining optimized checklist items stay in #220's evidence. |
| Changed host at `ea183cac5b14ee2d23816227383741feb2d65fe1`, NSH `413cfb65bf87a7bd9937f223fdc56896d8a718c8` | The combined validation tree was still building the exact-pin Android native runtime at this snapshot. No finished changed-host APK/AAB or emulator pass is claimed. It must verify normal tablet/TV launch, stale/alternate component redirects, one canonical resumed activity, no extra Rust root, handheld playback PiP and notification return. A local debug-signed optimized test artifact must remain distinct from a distribution-signed AAB. |

The historical AAB SHA-256 is
`59d127ccc8570a84c606eef1d6f9b9876cbf2d1fcbf13e3845fe813a727400cb`.
It predates the Leanback change. The authorized synthetic video comes from
[Apple's streaming examples](https://developer.apple.com/streaming/examples/);
no provider data is used. Emulator observations do not replace physical-device
acceptance.

## iPad build and artifact audit

The local unsigned device-target bundle was built on November 25, 2026 through
`tools/ci/prepare-ios.sh`, from source
`ea183cac5b14ee2d23816227383741feb2d65fe1`, exact NSH
`413cfb65bf87a7bd9937f223fdc56896d8a718c8`, Rust 1.99.1 and checksum-verified
MPVKit inputs. The app and iOS runner builds passed. Its executable SHA-256 is
`bb593b151088f51ff77c931e6ee9e87f0deee4db5cc1860ed2af8ce15860c052`.
The coordinator independently reran the redacted inspector and confirmed it.
These results apply to that prior source. The later refresh onto master
`39d0a09c` changes app/device behavior; a submission candidate must be rebuilt
and audited from its exact final source before signed distribution validation.
No pre-existing Patina iPad bundle/IPA was available locally or among the public
v0.1.8 release assets.

The installed toolchain is Xcode 27.2 (27B52). The bundle's `DTSDKName` is
`iphoneos27.2`, its `DTXcode` is `2720`, and Mach-O build metadata reports iOS,
minimum 15.0, SDK 27.2. Apple's October 30, 2026
[App Store Connect release notes](https://developer.apple.com/help/app-store-connect/release-notes/)
confirm acceptance of Xcode 27.2 and iOS/iPadOS 27.2 SDK uploads. This verifies
public toolchain acceptance; actual signed upload/account validation remains
unperformed.

| Finished local bundle inventory | Observed result |
| --- | --- |
| Identity/version | `com.hantekllc.iptvrs`, version `0.1.8`, build `1`, `iPhoneOS` |
| Device/layout | iPad only (`UIDeviceFamily: [2]`), four orientations, minimum iOS 15.0 |
| Signing | Signature absent; strict verification false; no embedded provisioning profile |
| Export compliance | `ITSAppUsesNonExemptEncryption` absent |
| Framework/privacy resources | No embedded framework bundles and no `PrivacyInfo.xcprivacy` files; this does not mean statically linked libraries are absent |
| Privacy link | Exact public privacy-policy URL found in executable bytes; no iPad UI navigation or browser opening was exercised |

The initial normal preparation built the bundle but exited 2 in notices:
local cargo-deny 0.19.4 rejected the repository's expected argument layout.
The repository-pinned cargo-deny 0.20.2 was fetched into the ignored tool
directory with its committed SHA-256 verified. Notices generation, all native
runner dependency-policy audits, MPVKit notices and bundle notices validation
then passed. A cached complete normal preparation with the pinned tool also
passed (exit 0). This preserves the initial failure rather than describing it
as a straight first-attempt pass.

Run the redacted inventory against a finished bundle:

```sh
python3 tools/ci/inspect-ios-bundle.py target/ios/Patina.app
```

It reports executable digest, bundle/platform keys, embedded framework bundles,
bundled privacy-manifest declarations and signature verification. It omits
certificate identities and profile/entitlement contents. Run it again on the
**signed final** bundle; a local unsigned/ad-hoc result cannot prove distribution
signing, provisioning, device installation or App Store validation.

The finished plist supplies a local-network usage message and allows arbitrary
HTTP and media loads for viewer-configured sources. Do not infer a `false`
export-compliance answer from HTTPS. The actual executable contains SQLCipher
key API and Rustls symbols; MPVKit build inputs also include OpenSSL/GnuTLS
libraries. The input inventory does not establish which entire libraries
survived linking. The owner must classify the app and linked cryptography and
record the matching plist/console answer and any required documents. Apple's
[encryption guidance](https://developer.apple.com/documentation/security/complying-with-encryption-export-regulations)
includes linked third-party libraries in that decision.

The finished executable imports `stat`, `fstat`, `statvfs`, `statfs` and
`mach_absolute_time`. It contains `NSUserDefaults`, `standardUserDefaults` and
`AppleTemperatureUnit` strings. These are binary audit leads, not evidence
that each path executed in a device journey. The pinned source provides
specific follow-up work:

- The NSH iOS adapter reads the system-written `AppleTemperatureUnit` through
  `NSUserDefaults` in `nsh-platform-ios/src/surface.rs`. Classify this use under
  an applicable approved reason, or remove/replace that dependency behavior.
  Apple's app-private-defaults reason `CA92.1` does not cover system-written
  information; do not add a blanket app-container reason.
- NSH's installed Apple logger uses `fstat(stderr)` and `stat("/dev/null")` in
  `nsh-platform-apple/src/unified_log.rs` to inspect file mode/device identity
  for log routing. Review those exact purposes rather than assuming they read
  file timestamps. The recording runtime uses `statvfs` for available space;
  storage also uses Rust file metadata. Classify each applicable file/disk
  API use and dependency declaration against its actual purpose.
- Trace the finished binary's boot-time API import to its actual callers and
  purpose. The similarly named iOS synthetic-input code is development-only;
  it does not establish the shipping call path.

No downloaded MPVKit XCFramework input contains `PrivacyInfo.xcprivacy`, and
none was found in the finished app. Complete the app/dependency audit and bundle
accurate privacy declarations before claiming submission readiness. Missing
manifests are a concrete remediation item; no Apple validation or rejection was
observed. Use Apple's
[privacy manifest guidance](https://developer.apple.com/documentation/bundleresources/privacy-manifest-files),
[required-reason rules](https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api)
and [API category/reason list](https://developer.apple.com/documentation/bundleresources/app-privacy-configuration/nsprivacyaccessedapitypes/nsprivacyaccessedapitype).
App Privacy console answers tracked by #221 do not replace these manifests.

Read-only local discovery found zero valid signing identities, zero profiles in
the standard provisioning directories and zero connected Apple devices. This
records unavailable local signing/device resources, without inferring Developer
Program membership from them. The public privacy policy was deployed from
merged #221 and independently verified HTTP 200 with bytes matching the source;
the finished iPad executable includes its URL. Actual in-app opening and store
listing fields remain unverified.

The focused About → Privacy policy row now shows the public URL before
activation. The browser action and failed-open notice remain available. This
keeps the address readable on TV images that accept the URL intent without
opening a usable browser. The existing isolated app scenario verifies the
focused address before any request, its presence after accepted activation, and
the distinct notice after a failed request. Actual TV readability and browser
page rendering remain separate device observations in the PR evidence.

## Emulator and physical-device evidence

| Evidence class | Result in this lane |
| --- | --- |
| Android tablet/TV emulator | Historical signed baseline observations are separated above. The changed-host combined-artifact run remains pending; no pass inferred from host tests. |
| Physical Android tablet/TV | No connected physical device was available in the team inventory. Required changed-host acceptance remains pending before merge. |
| iPad simulator | No available simulator devices. `simctl` attempted runtime installation and reported authorization required; no playback or privacy-link journey verified. |
| Signed physical iPad | No signed candidate installation or playback performed. Owner must install the distribution-appropriate candidate and verify normal launch, source import, playback, recording where available, background/return and Settings → About → Privacy policy. |

The Android physical-device requirement comes from
[CONTRIBUTING.md](../CONTRIBUTING.md#when-a-device-run-is-needed). This record
does not waive it. Keep emulator observations separate from physical evidence.

## Console and owner handoff

The Chrome browser integration could not attach: its network-policy prerequisite requires
a desktop-app update. Consequently neither store console was reached, no
account facts were verified and no fields were changed. Account-dependent
items below remain **unverified**, not implicitly complete.

| Store | Owner-dependent verification or action |
| --- | --- |
| Apple | Active Developer Program membership, current agreements/attachments and app record for `com.hantekllc.iptvrs`; final distribution profile and signing; accepted Xcode/SDK upload and validation. |
| Apple | Export classification for the actual app and linked cryptography, any required documents and matching plist/console declaration; final required-reason/SDK privacy-manifest audit. |
| Apple | Age-rating questionnaire and accurate review notes with a publicly reachable synthetic playlist/guide and authorized demo media; verified iPad installation and feature instructions. Do not offer a review-only bypass or local/private fixture URL. |
| Google Play | Developer identity/verification, production package registration for `com.hantekllc.iptvrs`, Play App Signing and IARC content rating. |
| Google Play | Determine account type and eligibility. Where the personal-account closed-testing rule applies, verify at least 12 opted-in testers for 14 consecutive days and approved production access. No tester or account data is public evidence. |
| Both | Owner selects launch territories and completes applicable storefront declarations, including Play trader status. Console status must be recorded without private account details. |

Review notes should describe normal entry: launch, add the provided synthetic
source through playlist setup, open its guide and play authorized demo media.
Describe Record now only where the signed iPad build actually supports it;
scheduled recording is unavailable. Record platform-specific omissions rather
than promising unverified background behavior. The owner must host and verify
the review fixture externally before submission.

## Local checks

The exact pinned Rust 1.99.1 passed `cargo test -p iptv-core -p
iptv-presentation` (37 core tests, 465 presentation unit tests, 53 presentation
integration tests and 2 doctests), and both portable crates built for
`wasm32-unknown-unknown`. The redacted bundle-inventory and provisioning helper
suite passed all 8 tests. The pinned native `cargo build -p iptv-app`, Android
host tests after the final Kotlin formatting changes, pinned ktlint 1.3.1 and
targeted Ruff 0.11.13 checks also passed. The local unsigned bundle audit and
normal preparation passed for the prior source as described above. After the
refresh onto master `39d0a09c`, the required Rust/wasm/native builds, 54 Android
host tests and merged-manifest checks passed again. An independent native
integration review confirmed live device reporting, orientation and redirect
guards remain intact. Native Codex and Antigravity reviews were reconciled,
including launcher lifecycle and inventory redaction/error cases. Claude Code CLI review could not
run because authentication was unavailable. Final-head CI is tracked in the PR;
unchecked store/device criteria keep this issue open (`Refs #222`).
