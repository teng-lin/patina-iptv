# Third-party AirPlay video feasibility · 26 November 2026

The Linux pyatv sender at PR #2846 commit `8848ad3f` played synthetic HLS video on a paired Mac17,2 receiver. The owner observed playback. **This meets the M8a requirement for one working video queue.** The earlier unpaired trials on both Macs failed, and Castor declined both models locally. Product integration and validation on other receivers and sender platforms remain pending.

## Receivers and method

A Linux workstation targeted two Apple silicon receivers, one at a time. The first advertised `Mac16,12` and ran macOS 26.6.2 (25G83). The second advertised `Mac17,2` and ran macOS 26.6 (25G72). Both advertised AirPlay on TCP port 7000. The initial unpaired tests served a generated 12-second H.264 Baseline and AAC MP4 from a temporary LAN HTTP origin, with a separate URL for each client. The origin counted requests during each playback attempt.

- **Castor.** Built the unmodified desktop `castor` CLI from commit `2d81b038` with Go 1.27.4 and its pinned Whisper submodule. Ran `castor --config <temporary-config> cast url <synthetic-url>` with `device.type` set to `airplay` and `device.host` set to the Mac. [Castor Android source at `2d81b038`](https://github.com/kdpkke/castor-android/tree/2d81b038293d0b039d73bb2c6d708900931e1e36)
- **pyatv.** The handoff specifies open PR #2846. The initial unpaired trials used its unmodified head `8848ad3f` and Python 3.13.17. Called `pyatv.scan` with the Mac as the sole host and `Protocol.AirPlay` as the sole protocol, then `pyatv.connect` and `stream.play_url` with in-memory storage. [PR #2846](https://github.com/postlund/pyatv/pull/2846) · [Exact checkout](https://github.com/postlund/pyatv/tree/8848ad3fd9ae46b8eb733bfc667b536a28f04c5a)

## Results on Mac16,12

The initial access setting was unknown. For the second run, the owner reported enabling access for anyone on the same network and later confirmed in System Settings that Require Password was off. The sender tests used no stored credentials and made no receiver configuration changes.

| Sender and observation | Initial run | Same-network access reported by owner |
| --- | --- | --- |
| Castor direct connection | Exited 1 with `the receiver plays no video over AirPlay` | Same exit and message |
| pyatv targeted discovery | Found service, reported pairing `Unsupported` | Same result |
| pyatv RTSP exchanges | `SETUP` returned 403 once. `GET /info` returned 200 twice. `RECORD` returned 403 twice. | `SETUP` returned 400 once. `GET /info` returned 200 twice. `RECORD` returned 455 twice. |
| pyatv playback result | `PlaybackError` after retries. Zero `POST /command` requests. | Same error and zero `POST /command` requests. |
| Synthetic origin requests | Castor 0, pyatv 0 | Castor 0, pyatv 0 |

Castor excludes models matching `^Mac\d+,\d+$` when processing discovery records and when connecting directly. Its connection error cannot establish how this receiver would handle Castor's play queue. pyatv independently sets its pairing classification to `Unsupported` for the same model family. Neither sender requested a PIN from the operator. The changed RTSP status codes did not produce a successful recording session, an `insertPlayQueueItem` request or media playback. The traces do not establish why the Mac returned those codes. [Castor model filter](https://github.com/kdpkke/castor-android/blob/2d81b038293d0b039d73bb2c6d708900931e1e36/services/apiserver/internal/device/airplay/features.go) · [Castor connection and discovery](https://github.com/kdpkke/castor-android/blob/2d81b038293d0b039d73bb2c6d708900931e1e36/services/apiserver/internal/device/airplay/family.go) · [pyatv pairing classification](https://github.com/postlund/pyatv/blob/8848ad3fd9ae46b8eb733bfc667b536a28f04c5a/pyatv/protocols/airplay/utils.py)

## Results on Mac17,2

Before casting, the receiving Mac fetched the first 64 bytes of the MP4 from Linux and received HTTP 206. The origin recorded that preflight separately. During the initial unpaired trials with both senders, the origin received **zero playback requests**.

| Linux sender | Session and playback observations |
| --- | --- |
| Castor at `2d81b038` | Exited 1 with `the receiver plays no video over AirPlay` when connecting to the configured Mac. |
| pyatv at `8848ad3f` | Discovered the AirPlay service and reported pairing `Unsupported`. `SETUP` returned 400. Two `GET /info` requests returned 200 and two `RECORD` requests returned 455. The client raised `PlaybackError` before sending a play-queue command. |

The Mac’s AirPlay receiver logged a setup failure with `kRequestErr` during the unpaired pyatv attempt. Neither sender started video in that comparison.

### Receiver-side diagnosis on Mac17,2

The owner confirmed working video from the native Apple sender. A subsequent Linux comparison used generated H.264/AAC HLS and progressive MP4. pyatv 0.18.0, PR #2846 at `8848ad3f`, and PR #2948 at `b77759d2` all received 400 on the initial RTSP SETUP. The fixture origin received zero playback requests.

The receiver's standard macOS logs for the 21:45:50–21:46:07 UTC trials show the setup property-list handler returning `kRequestErr` (-6711). They do not identify a rejected field. On PR #2846, the sender sets its verifier before SETUP succeeds and then retries using the same stream object. The retries skip base setup and send RECORD, explaining the subsequent 455 responses.

PR #2846's URL player parses the stored credential directly. With no stored pairing it selects null verification, while the library's feature-based credential helper selects transient pairing for this advertisement. An explicit call to the public AirPlay pairing API reached the receiver's ordinary approval UI. The sender timed out after 10 seconds. The receiver recorded an unanswered approval after 15 seconds and removed the notification without user action. That trial stopped before pair-setup and pair-verify. [URL player](https://github.com/postlund/pyatv/blob/8848ad3fd9ae46b8eb733bfc667b536a28f04c5a/pyatv/protocols/airplay/__init__.py) · [Credential selection](https://github.com/postlund/pyatv/blob/8848ad3fd9ae46b8eb733bfc667b536a28f04c5a/pyatv/protocols/airplay/auth/__init__.py) · [Video session setup](https://github.com/postlund/pyatv/blob/8848ad3fd9ae46b8eb733bfc667b536a28f04c5a/pyatv/protocols/raop/protocols/airplayv2.py)

In an owner-attended retry at 23:06 UTC, pyatv PR #2846 used in-memory credentials, srptools 1.0.1 and a 30-second sender timeout for the approval request. The owner accepted and entered the displayed code. All pair-setup and pair-verify requests completed with HTTP 200. The first post-verification RTSP SETUP timed out, as did two subsequent GET /info requests. The sender raised PlaybackError before issuing a video queue command.

The receiver confirmed pair verification and allowed the authenticated SETUP. It created a session and renderer, recorded 64 NTP timing timeouts, and removed the renderer when setup failed after 32.283 seconds. During that window the Linux kernel logged UFW blocking two 32-byte UDP payloads from the receiver to the same ephemeral port. pyatv advertises a dynamically bound sender-side UDP timing listener. This implicates the return path for timing traffic.

At 23:10 UTC a second owner-approved run bound the timing listener to UDP 47475 and completed pair verification. The listener recorded zero timing datagrams. At 23:11:02 UTC the sender kernel logged UFW dropping a datagram from the receiver addressed to that exact port. The saved sender UFW rules contained no entry for 47475 when checked during the test. SETUP timed out and the client exited before the video queue.

### Successful Linux-to-Mac HLS playback

At 23:14 UTC the owner approved pairing on the same receiver. The Linux sender allowed UDP 47475 inbound from the selected receiver on Wi-Fi. The pinned timing listener received requests, both SETUP operations and RECORD returned 200, and the receiver accepted the play queue commands. UFW also logged blocked TCP connections to the synthetic HLS origin. A second temporary rule admitted that receiver to the origin on TCP 8089. The fixture server recorded successful responses for all 150 distinct video-on-demand segments during the trial. Its request log omits client addresses. The owner reported playback on the Mac. Receiver-side CoreMedia logs confirmed video decoding and presentation. One display sample counted 151 presented frames with zero drops during that interval.

The harness ended the sender session at its 55-second test limit. The segment count measures fetches and the frame count covers one display sample. Natural completion, audible output, sustained live playback and remote controls still need independent checks. The two receiver-scoped firewall rules were temporary. Both were removed after the trial.

## Remaining validation

Repeat with rolling live HLS, a channel change, stop and return to local playback. Record software versions and repeat on Apple TV or a video-capable smart TV before claiming those receivers. Android and Windows senders still need their own hardware runs.
