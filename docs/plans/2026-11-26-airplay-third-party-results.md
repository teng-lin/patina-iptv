# Third-party AirPlay video feasibility · 26 November 2026

Neither upstream sender started URL playback on the available Mac receiver. **M8a did not pass, so M8b stays closed.** Patina should keep third-party AirPlay video unavailable on Android, Windows and Linux. Native AirPlay from Apple apps uses a separate AVFoundation route and has its own acceptance tests.

## Receiver and media

A Linux workstation targeted one Apple silicon Mac on the same LAN. Its `_airplay._tcp.local.` service resolved on port 7000 and advertised the model identifier `Mac16,12`. Each sender received a fresh HTTP URL for the same generated 12-second MP4 with H.264 Baseline video and AAC audio. A temporary origin counted requests. The test kept the receiver's existing access settings.

| Unmodified sender | Action | Observation |
| --- | --- | --- |
| [Castor Android source at `2d81b038`](https://github.com/kdpkke/castor-android/tree/2d81b038293d0b039d73bb2c6d708900931e1e36) | Built its desktop `castor` CLI with Go 1.27.4 and the pinned Whisper submodule. Ran `castor --config <temporary-config> cast url <synthetic-url>` with `device.type` set to `airplay` and `device.host` set to the Mac. | The CLI exited 1 during receiver connection and reported `the receiver plays no video over AirPlay`. The origin received zero requests. |
| [pyatv PR #2846 at `8848ad3f`](https://github.com/postlund/pyatv/tree/8848ad3fd9ae46b8eb733bfc667b536a28f04c5a) | Installed the PR checkout in a Python 3.13.17 virtual environment. Used `pyatv.scan` with the Mac as the sole host and `Protocol.AirPlay` as the sole protocol, then `pyatv.connect` and `stream.play_url` with in-memory storage. | Discovery found the service and reported pairing `Unsupported`. The Mac answered two `GET /info` requests with 200 and returned 403 to `SETUP` and both `RECORD` attempts. The sender raised `PlaybackError` after its retries. No `POST /command` was observed. The origin received zero requests. |

Castor excludes models matching `^Mac\d+,\d+$` when it processes discovery records and when it connects directly. Its error therefore cannot establish how the receiver would handle Castor's play queue. pyatv independently classifies the same model family as unsupported for pairing. The test used no stored credentials, and neither sender requested a PIN from the operator. The 403 responses establish where that attempt stopped. They do not identify the receiver's configured access policy. [Castor model filter](https://github.com/kdpkke/castor-android/blob/2d81b038293d0b039d73bb2c6d708900931e1e36/services/apiserver/internal/device/airplay/features.go) · [Castor connection and discovery](https://github.com/kdpkke/castor-android/blob/2d81b038293d0b039d73bb2c6d708900931e1e36/services/apiserver/internal/device/airplay/family.go) · [pyatv pairing classification](https://github.com/postlund/pyatv/blob/8848ad3fd9ae46b8eb733bfc667b536a28f04c5a/pyatv/protocols/airplay/utils.py)

## What would reopen M8b

Repeat M8a on an available Apple TV or AirPlay-capable television using the receiver's ordinary pairing flow. Record the receiver model and software, whether `insertPlayQueueItem` succeeded, whether the receiver fetched the generated URL, and whether the video played. No Apple TV or AirPlay-capable television was available for this run. The results cover one Mac and the Linux builds of the two clients. There is no Android or Windows hardware playback result.
