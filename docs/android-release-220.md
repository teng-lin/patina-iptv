# Android optimized release evidence (#220)

Checked November 24, 2026. This is a partial acceptance record; issue #220
remains open. The public 0.1.8 packages establish a signed optimized baseline,
not verification of a future candidate containing the separate #222 manifest fix.

## Package evidence

The existing release configuration enables R8 optimization, obfuscation and
resource shrinking with targeted JNI keep rules. `prepare-android.sh` compares
the generated mapping byte for byte with the bundle's embedded map before
staging. `check-android-artifacts.py` now also requires an R8 map containing
class mappings in each AAB, including after the isolated signing step.

Downloaded only the public Android assets, checksums and candidate manifest
from [v0.1.8](https://github.com/teng-lin/patina-iptv/releases/tag/v0.1.8).
The compiled source is `469f3683505abce9ab50d8aaa651448510a29865`, NSH
`fd847c95917edbac95368ae28b7c19fe3ee01470`; release workflow run
[45330350566](https://github.com/teng-lin/patina-iptv/actions/runs/45330350566).
These differ from this validation harness's checkout and NSH pin.

| Public artifact | SHA-256 |
| --- | --- |
| `Patina-0.1.8-android-tablet-tv.apk` | `8d23cad513d9a4cf68f070eddefcc66d40c3ee9b0b6b1babb24d35681c91428a` |
| `Patina-0.1.8-android-tablet-tv.aab` | `59d127ccc8570a84c606eef1d6f9b9876cbf2d1fcbf13e3845fe813a727400cb` |

Both packages pass required ARM ABI and notice checks; APK `apksigner verify`
and AAB `jarsigner -verify` pass. Jarsigner reports a self-signed upload
certificate and no timestamp; this is a cryptographic signature check, not
verification against an owner-confirmed production certificate or Play signing.
`gh attestation verify` passes with the repository, release workflow and
`refs/tags/v0.1.8` restricted explicitly. No signing secrets were read.

The AAB embeds R8 8.9.35 mapping, 14,934,549 bytes, SHA-256
`a79c7b1afd6dfac0c59824499a5d41c0cd2717963fa26bf230e32492a5af7795`.
Keep the exact AAB to retain its map for symbolication. The single packaged
DEX is 1,625,404 uncompressed bytes; this is a local package measurement,
not a Play Console optimization percentage or threshold determination.

## Emulator acceptance

`disposable-avd.py --release` admits only a non-debuggable production-ID APK,
after verifying an owned private AVD copy. It resets only that copy, installs
the unchanged APK, resolves the appropriate Android launcher category and
waits for normal app startup. It does not access private app storage, inject
settings or prefill a development playlist. Its result explicitly leaves the
remaining journeys pending. The existing fixture smoke remains separate and
also resolves the appropriate launcher for tablet/TV.

Record the artifact source independently from the harness source:

```sh
python3 tools/ci/disposable-avd.py "$HOME/.android/avd/tab36.avd" \
  target/release-220/Patina-0.1.8-android-tablet-tv.apk --release --port 5580 \
  --artifact-commit 469f3683505abce9ab50d8aaa651448510a29865 \
  --artifact-nsh fd847c95917edbac95368ae28b7c19fe3ee01470
```

Repeat with `tv36.avd` and a free even port. `IPTV_SMOKE_HOLD=600` leaves
the owned emulator available for visible UI journeys before cleanup. Evidence
is retained beneath `target/android-validation/run-*`; failed or incomplete
operations are not passes. Never run against the original profiles or viewer
installs. Screen captures and logs use fresh synthetic profiles only.

| Journey | Tablet emulator | TV emulator |
| --- | --- | --- |
| Normal launcher and first run | Pending | Pending |
| Playlist import / playback | Pending: public synthetic endpoint required | Pending: public synthetic endpoint required |
| PiP | Pending active playback; handheld PiP retained by #222 | Pending: verify Leanback exclusion with #222 candidate |
| Backup/restore | Pending visible document picker journey | Pending visible document picker journey |
| Recording | Pending synthetic playback and device capability checks | Pending synthetic playback and device capability checks |

The production network policy excludes private upstream addresses. The local
fixture server used by the debug smoke cannot establish signed release playback.
No tester-controlled public endpoint was available in this pass. Provide a
public endpoint serving only generated playlists, XMLTV and authorized demo
media, then follow [signed release acceptance](android-validation.md#signed-release-acceptance).
Do not weaken production transport restrictions to obtain a pass.

## Physical-device acceptance

`adb devices -l` showed no connected targets. Tablet/TV physical playback,
PiP, document pickers, recording, background/resume and install-over-previous
remain owner-dependent. Record each against the final signed artifact hash,
model, OS, ABI and configuration. Emulator observations do not satisfy the
physical-device gate in CONTRIBUTING.md.

## Play Console acceptance

Console access was unavailable: the browser connection required a network
policy and a desktop app update before it could open a store page. No console
fields or metrics were inspected or changed, and no bundle was uploaded.

Google's [technical quality requirements](https://support.google.com/googleplay/android-developer/answer/17492799)
were checked directly. From February 2027, apps above 10 MB DEX must reach
25% each for optimization, obfuscation and shrinking across form factors.
Memory thresholds use 28-day P90 data and apply to phones/tablets; Android
vitals separates anonymous RSS plus swap from bitmap memory by application
state and RAM tier. Local R8 configuration and a single memory snapshot do
not establish compliance with these metrics.

An authorized owner must inspect the final uploaded version in App bundle
explorer for DEX size and all three optimization percentages, and inspect
Android vitals memory across available RAM tiers and states. Record unavailable
or insufficient data explicitly. Confirm the signing certificate against the
owner-approved upload certificate, preserve the exact map, and rerun the full
journey matrix for the final candidate including #222 before closing #220.
