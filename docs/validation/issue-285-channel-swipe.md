# Issue 285 channel swipe validation

Validation started from `0c76709d45fb6a1b8c194023c8ff2ce362a3ed1a`, with NSH
pinned at `413cfb65bf87a7bd9937f223fdc56896d8a718c8`. All playlists, guide data
and media in these checks are synthetic. Swipe up follows Ch+ to the row above,
and swipe down follows Ch− to the row below. [Issue #285][issue]

## Merged evidence

- #296 added swipe zapping, channel buttons, the two named accessibility
  actions, iPad adjustable actions, and the swipe setting. Its x86_64 `tab36`
  journey with TalkBack off exercised both directions, preview tracking, burst
  coalescing, top and bottom edges, cancellation, and both buttons. With TalkBack
  on, a one-finger swipe left the channel unchanged. Double-tap, the TalkBack
  actions menu, and tactile haptics were unverified. [PR #296][pr296]
- #317 added cancellation for a second finger on a sibling control and recovery
  from interrupted contact sequences. Its API 36.1 tablet emulator ran both
  swipe directions and used Android `AccessibilityNodeInfo` with TalkBack
  bound and touch exploration enabled to invoke both named video actions and
  both channel buttons. Each selected stream reached ready. This establishes
  native node activation with TalkBack running. [PR #317][pr317]
- Full ready-PR validation completed successfully at
  `29800b42455dcde11072ade40d9363c0dae13947`. The run includes both Android
  tablet and TV smoke jobs, host tests, coverage, Windows, Linux/macOS packages,
  Android checks and the required gates. Its iOS job was skipped. The smoke
  jobs exercise the synthetic native journey through remote keys and touch.
  [Validation run 45531781646][run]

## Physical devices

A physical Android phone with TalkBack off and on, the tactile light tick on
swipe commit, and a physical iPad with VoiceOver custom actions and adjustable
swipes up and down remain unverified. The host had no attached physical devices
in `adb devices -l` or `lsusb`. `adb mdns services` returned no records, but a
passive `avahi-browse -rt _adb-tls-connect._tcp` found a CPH2583 phone and an
SM-X210 tablet. The endpoints were handed to the lead for authorization and
coordination with an operator. Nothing connected to or changed those devices.
Bounded Avahi checks for `_apple-mobdev2._tcp`, `_apple-mobdev._tcp`,
`_remotepairing._tcp` and `_remotepairing-manual-pairing._tcp` found no Apple
mobile-device or pairing records. No paired Apple host was supplied.

## Local Android emulator

The x86_64 validation APK came from `android-smoke-input` in run 45531781646
at `29800b42`. Its SHA-256 was
`b726c836b633b0f52a3ebe533c64bee58ead2f14d69fd5628358c1329304cc69`.
The later base has identical swipe, player and Android host input code and
the same NSH revision. Local builds and headless checks use the later base.
[Validation run 45531781646][run]

The emulator used SDK Emulator 37.3.10 with KVM and SwiftShader, the Pixel
Tablet profile, and the API 36 Google APIs x86_64 image revision 7
(`BE2A.250530.026.F3`). The [disposable profile runner](../android-validation.md)
made owned copies of a newly created `tab36` seed. The stock cold boot exceeded
180 seconds. A retry with a private 600-second boot deadline imported the
300-channel fixture and stopped at the generic favorites menu focus check
while the initial guide was being published. The full tablet and TV smoke
results from the ready PR are recorded above.

## Local headless checks

- `cargo test -p iptv-core -p iptv-presentation` passed, including the portable
  gesture tests for edges, cancellation, second contacts, short/slow gestures
  and flings.
- `cargo build --target wasm32-unknown-unknown -p iptv-core -p iptv-presentation`
  passed.

[issue]: https://github.com/teng-lin/patina-iptv/issues/285
[pr296]: https://github.com/teng-lin/patina-iptv/pull/296
[pr317]: https://github.com/teng-lin/patina-iptv/pull/317
[run]: https://github.com/teng-lin/patina-iptv/actions/runs/45531781646
