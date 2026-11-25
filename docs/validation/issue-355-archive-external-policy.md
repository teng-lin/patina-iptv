# Archive entry-point external-player policy (#355)

The policy is consistent across explicit archive choices: selecting a past
program and choosing **Play from the beginning** honor the same group's
external-player preference. The menu action chooses a starting position,
rather than overriding the player's group preference. Unsupported hosts,
groups without that preference and a missing external handler retain the
internal player path. A changed or removed captured target is rejected.

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

The retained baseline evidence is outside the repository in
`/Users/tenglin/src/patina-355-evidence/`: `baseline-apk-proof.json`,
`baseline-comparison-proof.json` and `tv-baseline/manual` phases 07–13.
The APK, native trees, screenshots and runtime logs are retained there.

## Regression and acceptance boundaries

The existing app harness contains generated catch-up scenarios for both
entry points, no-handler internal fallback and PIN cancellation/approval.
A Linux scenario uses a generated player process to request the real
handoff relay, checking one upstream archive request per accepted choice
and rejecting a superseded group's pending handoff. Private app tests
check channel/UTC remapping and rejection after occurrence-end, source,
channel authority, guide-offset or archive-window changes.

At this documentation update the implementation has not been compiled or
run: native device work owns the available build capacity. Corrected
Android behavior, Linux execution, standard native/Wasm checks and
independent review remain required before completion is claimed.
