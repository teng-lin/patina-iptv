# First-submission readiness evidence (#222)

This pass began on November 24, 2026 from `origin/master` `0c76709d`, with
NSH pinned to `413cfb65bf87a7bd9937f223fdc56896d8a718c8`. It records partial
progress on [#222](https://github.com/teng-lin/patina-iptv/issues/222), not
authorization to submit a first release. The [privacy evidence](store-privacy.md)
and [optimized Android validation](https://github.com/teng-lin/patina-iptv/issues/220) have
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

Local release host checks passed: `:app:testReleaseUnitTest` (51 tests) and
`:app:processReleaseMainManifest`, using JDK 17 and Android SDK 36. The generated
merged release manifest independently confirmed:

| Activity | Launcher category | PiP attribute |
| --- | --- | --- |
| `com.hantekllc.iptvrs.host.TvActivity` | `LAUNCHER` | `true` |
| `com.hantekllc.iptvrs.host.LeanbackActivity` | `LEANBACK_LAUNCHER` | Absent |

The existing `HostPolicyTest` harness covers the two declarations, matching
lifecycle attributes, device launcher selection and stale-shortcut redirect
selection. Notification wiring and lifecycle behavior still need device
validation. A source/merged
manifest check does not prove the manifest inside a signed AAB or handheld PiP
playback. The #220 lane owns those final-artifact and emulator checks against
the combined candidate. Record its artifact SHA and results here after that run.

## iPad build and artifact audit

The machine reports Xcode 27.2 (27B52), iPhoneOS SDK 27.2. Installed tooling alone
does not prove Apple currently accepts that toolchain for submission. No
pre-existing `Patina.app` or IPA was found in the repository's `target` or local
Xcode Archives. A device-target packaging attempt uses the exact NSH pin and
checksum-verified MPVKit inputs without signing credentials. Its outcome and
finished-bundle inventory will be recorded below.

Run the redacted inventory against a finished bundle:

```sh
python3 tools/ci/inspect-ios-bundle.py target/ios/Patina.app
```

It reports executable digest, bundle/platform keys, embedded framework bundles,
bundled privacy-manifest declarations and signature verification. It omits
certificate identities and profile/entitlement contents. Run it again on the
**signed final** bundle; a local unsigned/ad-hoc result cannot prove distribution
signing, provisioning, device installation or App Store validation.

The source config targets iPad only (`UIDeviceFamily: [2]`), permits all four
iPad orientations, and supplies a local-network usage message. It allows HTTP
loads, including media, for viewer-configured sources. It does not declare
`ITSAppUsesNonExemptEncryption`. Do not infer a `false` value from the use of
HTTPS: the shipped inputs include Rust TLS, SQLCipher and MPVKit's OpenSSL and
GnuTLS libraries. The owner must classify the app and linked dependencies and
record the matching console answer/documentation. Apple's
[encryption guidance](https://developer.apple.com/documentation/security/complying-with-encryption-export-regulations)
explicitly includes linked third-party libraries in that decision.

Review required-reason APIs in app and dependency code against the finished
executable and resource inventory. A missing manifest is a pending audit
finding, not proof that no declaration is required. Apple's
[privacy manifest guidance](https://developer.apple.com/documentation/bundleresources/privacy-manifest-files)
and [required-reason rules](https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api)
require accurate declarations for applicable APIs, including SDK use. App
Privacy console answers are tracked by #221; they do not replace manifests.

The pinned NSH iOS adapter reads the system's `AppleTemperatureUnit` through
`NSUserDefaults` in `nsh-platform-ios/src/surface.rs`. This needs an API or
approved-reason review: Apple's `CA92.1` reason for app-private defaults does
not authorize reading a system-written setting. The recording runtime checks
available space with `statvfs`, and file storage uses Rust file-metadata APIs.
These provide concrete disk-space and file-metadata audit leads. None of the
downloaded MPVKit XCFramework inputs contains `PrivacyInfo.xcprivacy`. Do not
substitute a blanket app-private-defaults declaration for this dependency
review. Consult Apple's current
[API category and reason list](https://developer.apple.com/documentation/bundleresources/app-privacy-configuration/nsprivacyaccessedapitypes/nsprivacyaccessedapitype).

## Emulator and physical-device evidence

| Evidence class | Result in this lane |
| --- | --- |
| Android tablet/TV emulator | Pending the optimized combined-artifact run owned by #220; no pass inferred from host tests. |
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
iptv-presentation` (37 core tests, 463 presentation unit tests, 53 presentation
integration tests and 2 doctests), and both portable crates built for
`wasm32-unknown-unknown`. The redacted bundle-inventory and provisioning helper
suite passed all 8 tests. The pinned native `cargo build -p iptv-app`, Android
host tests after the final Kotlin formatting changes, pinned ktlint 1.3.1 and
targeted Ruff 0.11.13 checks also passed. The final bundle audit and review/CI
results are recorded as they complete; unchecked store/device criteria keep
this issue open (`Refs #222`).
