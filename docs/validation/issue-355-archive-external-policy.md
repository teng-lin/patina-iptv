# Archive entry-point external-player policy (#355)

The policy is consistent across explicit archive choices: selecting a past
program and choosing **Play from the beginning** honor the same group's
external-player preference. The menu action chooses a starting position,
rather than overriding the player's group preference. Unsupported hosts,
groups without that preference and a missing external handler retain the
internal player path. A changed or removed captured target is rejected.
The full-screen player's captured channel can restart outside the last
browsed group; that unrelated group does not authorize external playback.
Missing-handler guidance appears after authorized internal playback starts,
so canceling a PIN leaves neither a reset menu nor a playback claim.

## Measured baseline

Synthetic data only. The clean baseline source was
`c637b9344ad3422aa2365fea3c2a99f9cf7c939b`, with NSH
`413cfb65bf87a7bd9937f223fdc56896d8a718c8`. Its validation APK was captured
before implementation edits on 2026-11-25 at 19:54:35 UTC:

```text
SHA-256 68f342e6424d3fa8140bc73dccacbc01c3238e1f90f436cb989a3a67fdad0770
Bytes   86560224
```

On an owned Android TV API 36 ARM64 emulator at 1920×1080, density 320,
the generated **News** group's external-player preference was verified as
On. Both inputs selected **Synth News 0000** (`ch0.synthetic.example`),
**Summer Workshop**, 2026-11-25 19:25–19:40 UTC, from the local mock server.
The archive request was identical:

```text
/archive/ch0/index.m3u8?utc=1795634700&duration=900
```

Normal Replay opened the synthetic external TestPlayer once, prepared it
and advanced playback for 68 seconds before returning. It opened no
internal player. Returning restored the same News/program selection.
The menu's **Play from the beginning** then opened internal player 0 once,
with native 1280×720 ready, and opened no additional external player.
This directly confirms the policy difference on the baseline; it is not
evidence of the corrected behavior.

The retained baseline evidence is in the host's out-of-repository
issue-355 evidence directory: `baseline-apk-proof.json`,
`baseline-comparison-proof.json` and `tv-baseline/manual` phases 07–13.
The APK, native trees, screenshots and runtime logs are retained there.

## Corrected native acceptance

The installed Android TV artifact was built from
`d19edd45e38e6b02f286cbeac3849e2f58f53f36`, using the same NSH pin:

```text
SHA-256 1d564ebb4d33e24d82561e7a2dcdfaf42a47182ca199fc8a84e3d2e399fa4b09
Bytes   86546528
```

A fresh owned Android TV API 36 ARM64 emulator used the same generated
News/channel/program occurrence and exact archive request as the baseline.
The News preference remained enabled. Each case had independent activity,
runtime-log and mock-server before/after windows:

| Ordinary input | External handoffs | Internal opens | Observed media | Manifest fetches |
|---|---:|---:|---|---:|
| Replay | 1 | 0 | TestPlayer prepared, video rendering started, position 8→18 seconds while playing | 2 |
| More options → Play from the beginning | 1 | 0 | TestPlayer prepared, video rendering started, position 8→18 seconds while playing | 2 |
| Same menu action after removing only the synthetic TestPlayer | 0 | 1 | Native 1280×720, tracks and ready; visible timecode 00:00:03.440→00:00:31.200 | 1 |

Every manifest request matched the channel/UTC/duration above. Repeated
HLS fetches are counted separately from app handoffs. Each external return
restored the exact News/channel/occurrence selection. The no-handler case
also displayed the existing internal-playback guidance and reset offer.
No playback retries were used.

The retained evidence includes `candidate-d19-apk-proof.json`,
`candidate-d19-native-comparison-proof.json` and its Markdown report,
`tv-candidate-d19/manual` phases 00–22, and the independent per-case raw
windows in `tv-candidate-d19/cases`.

The later `879acd93cad8f9bd57f5054f8c4281a42f211d55` commit changes only the
existing app test fixture: it serves supported generated HLS content and
adds bounded HTTP failure diagnostics. Every other tracked file and mode
matches the installed `d19edd45` source. The final validation-document
update changes only this report. `fixture-hls-test-only-equivalence.json`
records the comparison. The installed APK retains its original commit
identity; these later commits are not relabeled as installed artifacts.

## Regression and acceptance boundaries

The existing app harness contains generated catch-up scenarios for both
entry points, no-handler internal fallback and PIN cancellation/approval.
A Linux scenario uses a generated player process to request the real
handoff relay, checking one upstream archive request per accepted choice
and rejecting a superseded group's pending handoff. Another Linux scenario
covers a same-group full-screen transfer across native source retirement.
Private app tests check channel/UTC remapping and rejection after occurrence-end, source,
channel authority, guide-offset or archive-window changes.

After rebasing onto `eca70e68`, the final local macOS checks passed all
three private archive-choice tests and all three app scenarios: both
entry-point fallbacks, PIN cancellation/approval, and full-screen restart
outside the browse group with both groups' external preferences enabled.
The existing TV replay journey also passed. These checks used Rust
`1.99.1` and the exact NSH revision above.

The final local checks also passed:

- All 211 private app tests and all six existing parental app scenarios.
- `cargo test -p iptv-core -p iptv-presentation`: 569 native tests and
  two compile-fail doctests.
- Wasm builds of both portable crates, native `cargo build -p iptv-app`,
  and the app's `wasm32-unknown-unknown` check.
- Affected app/presentation Clippy with `test-support`, `--all-targets`
  and `-D warnings`; scoped formatting and `git diff --check`.
- The 32-test CI-plan suite, including the test-executable budget.

After the supported-HLS fixture correction, all three Mac archive
scenarios and affected app Clippy passed again. The generated Linux
handler probes relay HTTP admission; it makes no decoder claim.

On `879acd93`, hosted Linux app validation passed all five archive
scenarios, including both entry-point handoffs, the superseded-group
rejection and same-group full-screen transfer. The full ordinary app suite
passed 359 tests with four existing ignores. Validation run
[45703804221](https://github.com/teng-lin/patina-iptv/actions/runs/45703804221),
Linux app job
[139520592081](https://github.com/teng-lin/patina-iptv/actions/runs/45703804221/job/139520592081),
also passed its performance journeys and native lifecycle check. The
retained raw log is `draft-368-879-linux-app-139520592081.log`.

All selected draft jobs passed. The draft gate intentionally deferred
coverage and smoke checks; complete validation of the final PR head is
still required before merge.
