# Retained recording transport fixes

Implemented W12 (`iptv-net-001` and `iptv-net-002`) in the isolated Rust
workspace fix branch. This is a native internal library, Rust 2024 / declared
Rust 1.99.1. Applied `rust-review` and `rust-best-practices`, the individual
net report and the consolidated plan's corrections to the sketch fixes.
No Cargo or test executable was run by this agent; root owns central builds
and checks. No manifest, shared CI input or dependency was changed.

## Changes

Private recording acquisition now uses one helper in `src/file.rs`. On Unix
it opens with `O_NOFOLLOW | O_NONBLOCK`; on Windows it opens the reparse point
itself and rejects opened metadata carrying `FILE_ATTRIBUTE_REPARSE_POINT`.
Every platform validates that the opened descriptor is a regular file before
reading or truncating. Picked playlist/guide file behavior is unchanged.

Recording reads convert that validated descriptor into Tokio's file type and
use its metadata for content length/ranges. Recorder resume obtains alignment,
retained byte count and size-limit decisions from the descriptor's actual
metadata, before truncating a torn packet. Missing spools use create-new
acquisition; zero-budget and full-spool refusal remain explicit.

The recording-file response owns one 15-second progress-idle deadline across
headers and body chunks. Each positive partial write resets it. A pending or
zero write and changing from headers to body do not reset it; an expired
response cannot extend itself merely by entering another send call. The body
uses bounded chunks without a total healthy-response deadline. Success, HEAD,
404 and 416 responses all use the same bounded writer. Finished serving tasks
are drained before applying the four-task admission limit; owner teardown
still aborts all active response tasks.

## Regression coverage added

Existing private unit modules and the existing `tests/it/recording.rs` harness
were extended; no test executable was added.

- `file::private_tests::private_reads_keep_the_opened_handle_and_its_metadata`:
  replace a pathname after opening and verify the descriptor still supplies
  the original bytes/length.
- `file::private_tests::private_reads_and_resume_refuse_a_substituted_symlink`:
  replace a formerly regular path with a symlink and reject both read and
  resume access without changing the target. Unix and Windows variants are
  cfg-gated. The Windows case requires symlink creation permission and is an
  explicit Windows reparse-point regression; it is not claimed run on Linux.
- `file::private_tests::a_substituted_fifo_never_waits_for_a_reader_or_writer`:
  perform read/resume acquisition after a deterministic regular-to-FIFO swap.
  A five-second parent deadline bounds a child of the same unit executable,
  so a regression cannot strand a blocking worker during runtime shutdown.
- `recording::private_spool_regressions::resume_alignment_and_limits_use_the_opened_recording_length`:
  retain whole packets and leave a full refused spool unchanged.
- `recording_file::tests::each_positive_partial_write_extends_the_idle_deadline`:
  a single three-byte send progresses every ten seconds and succeeds after
  thirty virtual seconds. A whole-chunk fifteen-second timeout would fail.
- `recording_file::tests::stalled_success_head_and_error_headers_share_the_write_deadline`:
  blocked success/error headers expire after exactly the idle budget.
- `recording_file::tests::moving_from_headers_to_body_does_not_reset_a_stalled_deadline`:
  the body inherits the last successful header-byte deadline.
- `recording_head_invalid_ranges_and_unknown_capabilities_have_bounded_responses`:
  real loopback HEAD, ranged HEAD, 404 and 416 responses contain no body.
- `four_stalled_recording_clients_expire_and_a_paused_player_can_reopen_a_range`:
  four authenticated unread sockets fill serving slots for a sparse synthetic
  recording; advance the idle clock while keeping those clients alive, then
  verify a fresh range response succeeds with the expected bytes and length.
- `dropping_recording_transport_cancels_stalled_body_tasks_and_listener`:
  owner drop closes a stalled response and listener without waiting for expiry.

Direct rustfmt and `git diff --check` passed. Root's first coordinated net
run passed all unit tests and 64 integration tests (`net-after.log`). Later
owner-scoped diagnostic observation additions still require their coordinated
rerun; this reviewer did not execute those tests. Clippy, native integration
and affected platform results remain separate gates.

At the native reviewer's request, cfg(test/test-support) `RecordingFile`
accessors `write_timeouts()` and `range_requests()` expose one relay's own
counters. Only authenticated response idle expiry and valid authenticated GET
byte ranges increment them; request-header timeouts, wrong capabilities, HEAD
and invalid ranges do not. One per-relay Arc stores both counters, and the
observer is zero-sized when test-support is disabled. The native pause test
can therefore prove that its own connection expired and its decoder made a
fresh range request, rather than accidentally passing on prefetched bytes.
A paused-clock counter-isolation regression was added.

## Limits and remaining integration gates

This hardens mutation of app-private storage; it establishes no remote
arbitrary-file-read exploit. No teardown/native guard policy was weakened.
The loopback range-reopen test establishes protocol support after a paused
client's connection expires; it does not establish that the actual bundled
libmpv/FFmpeg reconnects correctly after a pause longer than fifteen seconds.
That native decoder pause/resume gate was explicitly sent to root for the
player review and must be retained before implementation completion.

Windows reparse execution and other affected platform checks belong to their
actual hosts/CI. Unix source and cfg-gated tests do not prove Windows behavior.

## What's already good

- Capability authentication and the existing four-client bound remain intact.
- Acquired handles own the validated resource through its operation.
- Healthy write progress has no fixed recording-duration deadline.
- Cancellation and bounded test-harness ownership remain explicit.

## Native stacked-relay observation

The native decoder reads RecordingFile through PlaybackRelay. Both layers already have 15-second write deadlines; downstream PlaybackRelay stalls first and ordinarily cancels its upstream response before RecordingFile's own idle timer can fire. A source size/cache adjustment cannot reliably make the upstream timeout counter fire through that production stack. The native regression therefore observes PlaybackRelay's actual downstream deadline, then a fresh RecordingFile range request and resumed native rendering/timeline. Direct RecordingFile tests separately establish its own idle deadline and capability-slot release.

Added cfg(test/test-support) `PlaybackRelay::write_timeouts()`: one owner-scoped counter shared only with that relay's sessions, incremented exclusively by expiration of downstream send_bytes. Header parsing/upstream read deadlines do not count. Production timers and transport behavior are unchanged. The native player agent wires observation through its worker-owned relay; central validation remains the coordinator's responsibility.
