# Third-party AirPlay video feasibility · 26 November 2026

Neither upstream sender started URL playback on the available Mac receiver, including after the owner allowed everyone on the same network. **M8a did not pass, so M8b stays closed.** Patina should keep third-party AirPlay video unavailable on Android, Windows and Linux. Native AirPlay from Apple apps uses a separate AVFoundation route and has its own acceptance tests.

## Receiver and method

A Linux workstation targeted one Apple silicon Mac running macOS 26.6.2 (25G83). Its `_airplay._tcp.local.` service resolved on port 7000 and advertised model `Mac16,12`. Each run generated a 12-second H.264 Baseline and AAC MP4 and served it from a temporary LAN HTTP origin. Each client received a fresh URL. The origin counted requests, and the test closed it after each attempt.

- **Castor.** Built the unmodified desktop `castor` CLI with Go 1.27.4 and its pinned Whisper submodule. Ran `castor --config <temporary-config> cast url <synthetic-url>` with `device.type` set to `airplay` and `device.host` set to the Mac. [Castor Android source at `2d81b038`](https://github.com/kdpkke/castor-android/tree/2d81b038293d0b039d73bb2c6d708900931e1e36)
- **pyatv.** The handoff specifies open PR #2846. Both runs used its unmodified head `8848ad3f` and Python 3.13.17. Called `pyatv.scan` with the Mac as the sole host and `Protocol.AirPlay` as the sole protocol, then `pyatv.connect` and `stream.play_url` with in-memory storage. [PR #2846](https://github.com/postlund/pyatv/pull/2846) · [Exact checkout](https://github.com/postlund/pyatv/tree/8848ad3fd9ae46b8eb733bfc667b536a28f04c5a)

## Results

The initial access setting was unknown. Before the second run, the owner reported changing the Mac's AirPlay Receiver access to allow everyone on the same network. The sender tests used no stored credentials and made no receiver configuration changes.

| Sender and observation | Initial run | After owner changed access |
| --- | --- | --- |
| Castor direct connection | Exited 1 with `the receiver plays no video over AirPlay` | Same exit and message |
| pyatv targeted discovery | Found service, reported pairing `Unsupported` | Same result |
| pyatv RTSP exchanges | `SETUP` returned 403 once. `GET /info` returned 200 twice. `RECORD` returned 403 twice. | `SETUP` returned 400 once. `GET /info` returned 200 twice. `RECORD` returned 455 twice. |
| pyatv playback result | `PlaybackError` after retries. Zero `POST /command` requests. | Same error and zero `POST /command` requests. |
| Synthetic origin requests | Castor 0, pyatv 0 | Castor 0, pyatv 0 |

Castor excludes models matching `^Mac\d+,\d+$` when processing discovery records and when connecting directly. Its connection error cannot establish how this receiver would handle Castor's play queue. pyatv independently sets its pairing classification to `Unsupported` for the same model family. Neither sender requested a PIN from the operator. The changed RTSP status codes did not produce a successful recording session, an `insertPlayQueueItem` request or media playback. The traces do not establish why the Mac returned those codes. [Castor model filter](https://github.com/kdpkke/castor-android/blob/2d81b038293d0b039d73bb2c6d708900931e1e36/services/apiserver/internal/device/airplay/features.go) · [Castor connection and discovery](https://github.com/kdpkke/castor-android/blob/2d81b038293d0b039d73bb2c6d708900931e1e36/services/apiserver/internal/device/airplay/family.go) · [pyatv pairing classification](https://github.com/postlund/pyatv/blob/8848ad3fd9ae46b8eb733bfc667b536a28f04c5a/pyatv/protocols/airplay/utils.py)

## What would reopen M8b

Repeat M8a on an available Apple TV or AirPlay-capable television using the receiver's ordinary pairing flow. Record the receiver model and software, whether `insertPlayQueueItem` succeeded, whether the receiver fetched the generated URL, and whether the video played. No Apple TV or AirPlay-capable television was available. These results cover one Mac and the Linux builds of the two clients. There is no Android or Windows hardware playback result.
