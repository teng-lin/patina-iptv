# Reproducible performance journeys

Run from a clean committed source tree with the pinned NSH checkout and native dependencies installed:

```sh
python3 tools/performance/journeys.py run
```

The canonical command runs on Linux with Xvfb, software GL and ffmpeg available.
It builds the debug `test-support` app and native GTK harnesses once, copies them
to a private temporary directory, and runs each scenario three times, sequentially.
Every scenario creates a fresh isolated synthetic HOME and removes it afterward.
`target/performance/summary.json` is written before the build and after every
sample, including failed samples. Each sample attests the project's compile-time eight-character Git hash
(`build_version.rs` explicitly slices the full hash) and build mode, so a stale supplied harness cannot be labeled with the checkout's current commit. `--output`, `--repetitions`, `--timeout` (seconds,
300 by default), `--harness /absolute/path/to/app-tests`, and `--native-harness` customize a run.
Do not run another timing workload alongside it. The timeout is an evidence
completeness deadline, not a performance budget.

## Workloads and endpoints

`tools/performance/fixtures.json` versions the generated workload specification;
its SHA-256 identifies the report's fixture. Programme times are relative to the
scenario's clock, and local file paths are deliberately excluded from the hash.
During opt-in counter journeys, the guide minute ticker is paused and periodic
EPG checks are disabled; the cached cold profile also disables startup playlist
and guide refresh. This excludes unrelated minute-boundary/background work from
the exact action counters.
The base-compared specification also binds each journey to its exact scenario test.
Changes to fixture generation or scenario identity must update that specification. No provider data,
network stream, credentials, or real media is needed.

| Journey | Fixture/cache state | Measured interval | Deterministic work |
| --- | --- | --- | --- |
| `cached-guide` | 2 channels, 144 hour-long programmes; imported SQLite cache, last channel set; new UI root | Immediately before headless root creation to cached current programme mounted, with the existing 500 ms settling interval | Owner's reader-lane jobs; one native source open across startup and guide arrival |
| `busy-guide` | 10,000 channels, 70,000 day-long programmes; warmed scrolling; SQLite `BEGIN IMMEDIATE`, favourite write queued behind it | 24 Down actions to mounted navigation under the held writer | Reader-lane jobs during the burst; native source opens |
| `selection` | Actual App, one cached channel, no previous playback; realized persistent native surface; generated 160×90 MPEG2 TS loopback media | Normal Enter handler dispatch on a mounted cached guide to the same root's current-source renderer callback | Same app owner's reader-lane jobs and actual native source opens |

The cold measurement is **headless root startup**, not an OS process launch or
physical display scanout. Fixture import and harness loading are excluded. The
busy journey reuses the existing isolated writer-contention scenario and its
existing latency check, rather than adding a second wall-clock budget. Guide
navigation and the pending favourite exercise both UI progress and an actual
blocked writer. Filtering/read independence remains covered by the existing
isolated `a_store_read_does_not_wait_for_a_busy_writer` scenario in `tools/check.sh`.

The selection endpoint is **first-render submission**, not display scanout. The
native GTK test runs `Root::of::<iptv_app::App>()` with a fresh generated encrypted
SQLite profile and waits for its cached guide and persistent native surface.
A test-support driver dispatches Enter through the same `input::key_event` path
as the normal registered handler, starting a monotonic interval immediately before
the action. The actual App then authorizes the selected stream, mounts its source
prop, and receives `firstRender {token}` from its own persistent native player.
The end retains that originating token and must match the token expected at the
action. This is one end-to-end app journey, with reader/open counters from that
same owner, without combining timings from independent executions.

The renderer emits only after a successful `mpv_render_context_render` call for
the current loaded entry, with `MPV_RENDER_UPDATE_FRAME` and a PRESENT frame that
is neither REDRAW nor REPEAT. A held frame from the previous stream cannot finish
the new token's interval. The event occurs once per changed source identity/token
and remains marked through PiP or GL context recreation. NEXT_FRAME_INFO is only
queried while first-render evidence is pending. It confirms renderer submission,
not GPU completion, compositor presentation, remote/OS key delivery, or physical
first-picture latency. A separate ordinary headless regression checks source
acceptance/open counts, but supplies no timing segment to the native journey.

The canonical command requires Linux's native backend. On macOS/Windows the
headless scenarios can be validated independently; the complete report remains
invalid without its actual App native-render journey.

A player `ready` event means ready state; `size` means reported dimensions.
Android sends `size` from both `onVideoSizeChanged` and `onRenderedFirstFrame`;
the shared size event cannot distinguish these callbacks. Neither event carries
its originating source token. The parser requires `first-render` for the native
segment and rejects acceptance/ready/size proxies as missing evidence. Android
must retain a callback's originating identity before it can support this metric.

## Report and CI policy

Schema 1 records commit, dirty status, platform/backend, hardware class, fixture hash and
specification, build mode, repetition count, deadline, raw per-sample monotonic
microsecond events, selection identity, source token, work counts, median, nearest
rank p95, maximum, and explicit validity/errors. No raw URL, provider name, channel
label, credential, or resolved address is recorded. Owner reader counters count accepted reader-lane jobs (including queued work),
not individual SQL queries or writes. They only exist with `test-support`; normal release builds add no counter or file writes.
Existing native-open counters are reused. The player surface is unchanged.

Missing, duplicate, stale, non-monotonic, timed-out or zero-sample evidence is
invalid. Failed scenarios stay in the report even if they wrote a terminal event.
An invalid run never substitutes a zero duration or quietly drops a failed sample.
Ordinary CI gates the committed work ceilings; wall-clock, frame and memory data
remain evidence rather than new shared-runner performance gates. Existing
isolated latency tests keep their established limits. Deliberately adding a read
to the busy navigation burst or reopening the full-app selection twice exceeds its guard. Counter snapshots
follow a 500 ms observation interval after the endpoint; that interval is not
added to the selection/native latency.
Fabricated regression cases exercise these failures without relying on clocks:

```sh
python3 tools/performance/test_journeys.py
```

PR CI compares the candidate baseline with the **base revision** before running
measurements. Removing a journey/counter or increasing a ceiling requires an entry
in `tools/performance/changes.json` containing `base_sha256`, `candidate_sha256`,
and a specific rationale of at least 30 characters. Hashes cover canonical JSON
(`json.dumps(value, sort_keys=True).encode()`). Fixture specification or endpoint changes also require `fixture_base_sha256`
and `fixture_candidate_sha256` covering canonical fixture JSON, plus a new rationale.
A record for one exact base/candidate hash pair cannot authorize another pair. Reviewers must assess the rationale and fixture change; the
record is evidence for review, not automatic permission. Counter types and complete
journey/counter sets are independently checked. There are no adjustable timing
tolerances. The initial policy is introduced when the base has no baseline file;
subsequent PRs execute the base revision's comparator against the base's copies.
An unresolved base commit fails closed. CI uploads the JSON even
when the policy, build, scenario, or guard fails, with the journey and counter in
the diagnostic.

## Android evidence and opt-in physical TV runs

`tools/perf-android.sh SERIAL [--cold]` remains an interactive profiling aid for an
already installed app with a synthetic playlist. Its launch `TotalTime` is an
activity-launch measure, guide marks are a UI proxy, and size/ready measurements
are uncorrelated proxies. Printed budgets from earlier versions have been removed.
With Bash 4.4 or newer, use `--json PATH` to also retain a versioned Android observation envelope. Its
`valid: false` and explicit unsupported measurements prevent an exploratory log
from being promoted to a valid first-picture journey report. The synthetic profile
must be prepared outside personal app storage, using the documented generated
fixtures and local mock server. Never run this against a real-provider profile.

Physical TV measurement is opt-in and separate from emulator validation:

1. Choose an explicitly authorized disposable low-end TV/device and record its
   model, SoC, RAM, Android version, refresh rate and thermal/power settings. Do
   not select a physical serial automatically. Do not clear personal app data.
2. Install the `profile` build, prepare only generated playlists/EPG/media using
   `tools/make-iptv-fixtures.py` and `tools/make-hls-fixtures.sh`, and record the
   fixture hash and exact commit/build mode. Keep the same cache state per run.
3. Collect at least ten runs after a documented warmup, with no competing work.
   Record activity/guide/ready/size proxies under their actual names; a first-picture
   budget requires an identified renderer callback (or an independently documented
   camera method), not the size event. Missing evidence invalidates that metric.
4. Store results with hardware class `physical-tv`, backend and device metadata.
   Emulator evidence uses `emulator` and is never pooled with TV samples or used
   to establish the physical budget. Compare matching hardware/build/fixture only.
5. Retain medians/tails and invalid sample counts, frame/memory traces and thermal
   context before proposing a device budget. No inherited 2,000/1,500/50 ms budget
   is accepted without confirming its endpoint and hardware.
