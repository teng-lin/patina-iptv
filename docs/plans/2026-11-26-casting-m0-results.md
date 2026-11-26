# Casting M0 measurements · 26 November 2026

Google's Default Media Receiver (`CC1AD845`) played a generated HLS stream over plain HTTP on the LAN. Removing the CORS header from the same HLS fixture produced `LOAD_FAILED`. A raw H.264/MP2 transport stream failed to reach confirmed playback. Copying its video into HLS and converting its audio to stereo AAC produced advancing playback with both segment lengths tested.

## Test conditions

The sender was Linux on Wi-Fi. The only receiver was the owner's Chromecast with Google TV. Firmware build unknown. Older Chromecasts and televisions with Cast built in remain untested. All media and account paths were synthetic. The origin served a 32-second numbered test pattern and tone. FFprobe checked the generated stream formats. Both sides of the CORS comparison used the same LAN origin. An initial load received no HTTP requests on the first port. Moving the origin to a firewall-permitted port made it reachable.

Playback here means the receiver reported `PLAYING` with a positive media position. No observer confirmed first picture, audible sound, surround output or deinterlacing quality.

## Measurements

| Spike | Experiment and observation |
| --- | --- |
| 1. TLS | The receiver advertised the Cast service over mDNS. The pinned oxicast 0.0.5 probe connected with rustls 0.23.46 and received a Cast receiver status. Oxicast skipped certificate chain validation for this connection and verified the handshake signature. |
| 2. Media loads | The matrix below used the Default Media Receiver with autoplay and no volume instruction. A five-second redirect token worked for the immediate MP4 load. An origin that rejected simultaneous requests also played the MP4. Delayed reuse of the token remains untested. |
| 3. Wire and session behavior | For baseline MP4 and stereo HLS with CORS, the first GET began 157 ms and 361 ms after LOAD, respectively. Each used one observed TCP source port. The receiver fetched the no-CORS manifest in 164 ms and rejected the load. No OPTIONS request appeared in these captures. The HLS marker tests below all reached PLAYING. A second sender joined the existing app and read both the synthetic credential-bearing content ID and the media-level customData marker. A bare LOAD switching between two paths for the rolling stream produced no overlapping HTTP transfers in that switch. This leaves overlap on a slow or held-open upstream unmeasured. The live command mask was 274447 (pause, seek, stream volume and mute, edit tracks, playback rate and stream transfer). The initial seekable window was about 14 seconds and reported a moving start and end. |
| 4. TS repackaging | Host FFmpeg 9.0.2 read generated H.264/MP2 TS with video copy and stereo AAC encoding. The resulting finite playlists contained sixteen 2-second or eight 4-second segments. The first positive PLAYING samples on one repeat arrived at 1.364 s and 0.888 s respectively. These are sender poll times. First picture and AVPlayer compatibility remain unmeasured. |
| 5. Live relay | The workstation looped a generated TS in real time, copied video and encoded stereo AAC into a rolling HLS window. A LAN HTTP relay fetched the playlist and segments from a separate loopback HTTP origin. The receiver reached PLAYING and requested advancing segments. At 9 min 11 s it still reported PLAYING. At 9 min 32 s it reported PAUSED, and the position stayed fixed for over a minute while manifest requests continued. The probe sent no pause command. Cause unknown. The run ended without demonstrating several hours of continuous playback. |
| 6. Device auth | A separate Cast protocol connection requested RSA PKCS#1 v1.5 with SHA-256 and a fresh 16-byte nonce. The reply echoed the nonce. Its leaf key verified the signature over nonce plus TLS peer certificate DER. OpenSSL verified the leaf through one supplied intermediate against the Cast/Eureka roots distributed by Open Screen. The reply included revocation data. CRL verification, Cast policy extensions and TLS certificate lifetime policy were outside this check. |
| 7. Desktop FFmpeg linkage | Checked the archive and library SHA-256 against the repository manifests on all three platforms. Linux loaded the shipped libmpv with ctypes. The mpv API resolved, while avformat_version, avformat_alloc_output_context2, av_interleaved_write_frame and avcodec_find_encoder_by_name did not. The macOS ARM64 export trie and Windows x64 DLL export table contained the mpv API and lacked those FFmpeg entry points. Direct linkage against these libmpv exports cannot supply the remuxer. Only Linux was executed. |
| 8. iPad to Mac AirPlay | Pending owner run with AVPlayer and a LAN HTTP URL, including screen lock, backgrounding, app quit and Wi-Fi loss. |
| 9. Native route picker | Pending owner interaction with AVRoutePickerView embedded in an NSH view on both iPad and macOS. |
| 10. Stream survey | Pending owner counts for HLS vs TS-only, audio codecs, interlacing, redirects, extensionless addresses, CORS and required headers. The synthetic matrix gives no prevalence estimate. |

### Synthetic media matrix

All rows except the CORS control supplied an allow-origin header. Segment format hints were omitted unless indicated. HLS used MPEG-TS segments.

| Content | Receiver observation |
| --- | --- |
| H.264 + stereo AAC, MP4 and HLS | Both reached PLAYING. |
| Identical stereo HLS without CORS | Manifest requested, LOAD_FAILED. |
| H.264 + 5.1 AAC or stereo MP2, HLS | Both reached PLAYING. Audio output unobserved. |
| H.264 + AC-3 or E-AC-3, HLS | First AC-3 load failed. First E-AC-3 load returned IDLE with no confirmed playback. Both reached PLAYING on repeat. |
| HEVC + AAC, HLS | Reached PLAYING. |
| Interlaced H.264 + AAC, HLS | Reached PLAYING. FFprobe reported top-field-first video. |
| H.264 + AAC, Matroska | Reached PLAYING. |
| H.264 + MP2, raw TS | First load returned IDLE without confirmed playback. Repeat returned LOAD_FAILED. |
| Stereo HLS with segment format hints | Neither hint, audio hint alone (`hlsSegmentFormat=ts`), video hint alone (`hlsVideoSegmentFormat=mpeg2_ts`), and both together reached PLAYING. |

## Decisions for the owner

- **Live delivery.** Retain direct fetch first for qualifying streams and the device relay as fallback. This receiver distinguished identical HLS with and without CORS. The second sender's readback supports giving the viewer the agreed notice before direct delivery. The provider survey is still needed to estimate how often live channels qualify.
- **M7 placement.** Keep M7 in its proposed position pending the survey. Raw TS failed while its video-copy AAC/HLS counterpart played. If TS-only channels or incompatible audio are frequent, move M7 directly after M2. The experiment changed container and audio together and cannot assign the improvement to either alone.
- **Android repackaging.** Size a minimal FFmpeg build for TS input, HLS output, video copy and stereo AAC as the baseline. The current Android host declares Media3 playback dependencies. There is no measured Android remux, APK size or power result to choose between that build and a platform-codec implementation. The desktop export checks also imply separate FFmpeg linkage or a packaging change there.
- **Default Media Receiver.** Retain it for the HLS milestone. It accepted plain-HTTP relayed live HLS, explicit and omitted format hints, and returned enough session state for rejoin. Audible playback and older receivers remain acceptance checks before making a broader compatibility claim.

## Linux implementation check

A separate driver on the workstation used the new Rust Cast session and `PlaybackRelay::start_cast` against the same receiver. It loaded two generated live HLS URLs in sequence, opened each gate on the first receiver request, waited for PLAYING, confirmed PAUSED, sent PLAY and waited for STOP before closing the relay. On the final successful run the relay counted nine requests through the first PLAYING observation for each channel. One preceding repeat timed out after 25 seconds waiting for PLAYING while the origin continued to receive playlist requests. The cause remains unknown. This driver exercised the protocol and relay directly. Parental PIN prompts still need an end-to-end acceptance run.

A headless Linux build then drove the app’s picker and playback controls against the same physical receiver with a generated live HLS channel and a one-connection account setting. The first discovery attempt missed the receiver. The system Avahi browser could see it. After adding the Avahi adapter, the app selected the receiver, paused and resumed, and returned playback to the local player through Play here. In a separate run an independent Cast status reader confirmed PAUSED and PLAYING on the original media session, then PLAYING on a different media session after a channel change. That run stopped at a test-driver timeout reopening the play bar after the change. A subsequent attempt could not connect at startup. The test origin and relay were stopped. A final receiver-status check found no running Default Media Receiver.

The app harness also exercises a receiver that never requests media, a late load refusal, loss of the control link during Stop, and the offer to play locally after repeated status failures. macOS and Windows now use the desktop sender. Physical runs from those hosts remain pending.
