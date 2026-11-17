# Rust review: iptv-player-mpv

This internal native adapter owns persistent libmpv surfaces and the player wire
contract. Its main confirmed defect is incomplete completion handling when a
load is refused before libmpv opens a file: the shell can remain buffering with
an outstanding load count. Worker replies can also wait indefinitely for an
unrelated native event before notifying the shell. Workspace Clippy passed;
native playback tests in the parent-coordinated run have failures and do not
establish a passing decoder baseline. No code changes or commits were made.

## Baseline and validation

- Application revision: `0e197073a272eceb0631e4d53b08d2823950c416`.
- Edition 2024; declared Rust version 1.99.1. Treat the native module as
  application integration and its exported wire types as shared library contracts.
- Parent-coordinated workspace Clippy with `--all-targets`, runtime/app
  test-support features and `-D warnings`: passed.
- The host wrapper stops at an existing NSH pin mismatch. The independent
  workspace tests use `/usr/lib/libmpv.so.2` on this machine. The review does
  not treat ready timeouts with an incompatible guarded runtime as proof of
  a decoder defect; the early-refusal reporting problem is independently
  established from its control flow.
- During the coordinated run, native tests including
  `a_control_thread_that_ends_fails_the_stream_until_the_next_open`,
  `a_dropped_live_stream_keeps_its_picture_until_the_retry_plays` and
  `two_live_relays_prefetch_with_independent_native_capabilities` failed.
  The invocation exited 101 after 484.11 seconds: 50 passed and 10 failed
  in this crate’s library tests. The complete failure list is retained in
  `/tmp/patina-rust-review-20261116/workspace-tests.log`.
- The normal app build and workspace doctests passed. The NSH revisions have
  identical tracked trees despite their commit mismatch; the wrapper remains
  blocked by its revision check.
- A temporary ctypes probe against the actual native library confirmed that
  consuming a command wakeup does not keep the following event wait awake:
  the second wait returned `MPV_EVENT_NONE` only after its full 250 ms timeout.
- Linux source, GL and native library behavior were examined. macOS, iOS and
  Windows source branches were inspected without executing those hosts.

## Must fix

### iptv-player-mpv-001 Guarded load refusal does not complete the outstanding load

**Locations:** `crates/iptv-player-mpv/src/mpv/worker.rs:328`, line 355 and
line 374; `crates/iptv-player-mpv/src/mpv/mod.rs:865`.

**Rule:** Expected failures must produce an observable error
(`rust-best-practices`, Errors and panics; Rust API Guidelines `C-GOOD-ERR`);
every accepted asynchronous operation needs a completion outcome.

**Problem:** A load increments `State.loads`, clears `expected` and reports
buffering. Missing load-hook authority, the absent native relay-guard marker,
or refused native guard options then return only `Note::TransportError`.
That note records a classification clue and emits nothing. No `Loading(None)`
or other completion decrements the load count. The optional `stop` cannot
repair this: an old entry's end is ignored after `expected` was cleared, and
an idle player may produce no end at all. The UI remains buffering and future
successful loads can remain suppressed by the leaked count. Guard refusal
must remain fail-closed while still giving the viewer a terminal error.

**Proposed fix:** Introduce an explicit rejected-load completion, accounting
for exactly one queued load and carrying its classification. Ignore older
completions using the same newest-load policy as successful loads. Do not
turn every `TransportError` clue into an immediate failure: ordinary end-file
handling deliberately gathers relay and native-log evidence first.

```rust
// A new Note variant, returned by each pre-load refusal:
Note::LoadRejected(&'static str),
// At a refusal site:
return vec![Note::LoadRejected(
    "ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED",
)];
```

```rust
Note::LoadRejected(code) => {
    let newest = {
        let mut st = self.state.borrow_mut();
        st.loads = st.loads.saturating_sub(1);
        st.loads == 0 && st.url.is_some()
    };
    if newest {
        fail(&self.state, out, code, None);
        self.video.redraw();
    }
}
```

The snippets are the proposed shape, not an applied patch. Also account for
the outstanding load when a hook substitution fails and guard authority is
permanently retired. Preserve source replacement and current-entry checks.

**Regression coverage:** Extend existing worker and native unit modules with
each early refusal; assert exactly one error followed by idle, an exhausted
load count and no upstream request. Cover two queued loads where only the
latest fails, and a refused load followed by an accepted synthetic one. Keep
the native guard capability requirement intact.

**Disposition:** Open; source-control-flow evidence confirmed. Existing native
test failures are supporting context, not a standalone reproducer of every branch.

### iptv-player-mpv-002 Command responses can be stranded before the main-loop notification

**Location:** `crates/iptv-player-mpv/src/mpv/worker.rs:241`, following the
command response sends at lines 234–237 and before `notify` at line 307.

**Rule:** Avoid waits whose completion requires unrelated work
(`rust-best-practices`, Concurrency and async); provide timely completion of
queued operations without blocking the UI.

**Problem:** After `execute` sends owned response notes, the worker always
enters `mpv_wait_event(..., -1.0)`. It calls `notify` only after that native
event loop returns. The wakeup that delivered the command may have already
been consumed by the preceding loop. Queries such as position/timeline do
not necessarily generate another event; pre-load refusals may not either.
Replies already in the channel can therefore remain invisible to the shell
until another command or unrelated native event wakes the worker. This is
separate from missing load completion in finding 001.

**Evidence:** A synthetic ctypes probe initialized an idle native handle,
drained its events, issued `mpv_wakeup`, consumed its `NONE` event and called
`mpv_wait_event` again with a bounded timeout. The second wait blocked for
0.250 seconds and returned `NONE` at timeout. The production call uses an
infinite timeout at this point.

**Proposed fix:** Drain already pending events without blocking when commands
have produced notes, then notify once for the combined batch.

```rust
let mut timeout = if sent { 0.0 } else { -1.0 };
```

Preserve the existing nonblocking drain after the first event and the
main-thread wake coalescing.

**Regression coverage:** Extend the existing worker tests with a quiet handle
and a command whose response produces no native event. Assert the notification
arrives without sending a second command or polling the response receiver
directly. A deterministic wake/command barrier is preferable to a tight timing
threshold in the ordinary parallel suite.

**Disposition:** Open; source ordering and actual native wait semantics confirmed.

## Should fix

### iptv-player-mpv-003 Dropping a UI player synchronously joins its control thread

**Locations:** `crates/iptv-player-mpv/src/mpv/worker.rs:209`;
`crates/iptv-player-mpv/src/mpv/mod.rs:1098`.

**Rule:** Destructors must not block (`rust-best-practices`, Errors and panics);
keep synchronous native control operations off the render/UI thread (the
crate's own worker contract).

**Problem:** `Inner::drop` runs as a native UI component is removed. It drops
its worker and waits in `JoinHandle::join` before freeing the render context.
Waking the event wait does not interrupt a synchronous native command already
being executed. The UI therefore waits without a deadline for that command.
This also recreates a possible render/control dependency cycle if the native
command is waiting for rendering that the joining UI cannot service. The
unbounded UI join is established by source; a native deadlock reproducer was
not executed, so this report does not claim one was observed.

**Proposed fix:** Separate stop signaling from joining. On the rendering
thread, request shutdown and release rendering callbacks/context in the valid
platform context; reap the control worker away from the UI, retaining its
handle until it finishes. Keep the waker alive until callback retirement is
complete and destroy libmpv only after its worker and render context are gone.

```rust
let worker = self.worker.get_mut().take();
if let Some(worker) = &worker {
    worker.request_stop(); // Sets quit and wakes the event wait; no join.
}
self.free_render();
if let Some(worker) = worker {
    worker.reap_off_main(); // Owns the handle until the control thread exits.
}
```

The new methods describe a lifecycle change requiring native API validation;
do not apply the sketch as a blind reorder of existing destructors. Account
for the retired `dead` worker as well, and avoid spawning unbounded reaper
threads during repeated multiview changes.

**Regression coverage:** Exercise component removal while native control is
pending, verify UI progress and bounded retained resources, then ensure render
callbacks and the native handle are retired exactly once. Cover full close,
tile removal and repeated open/close on each affected platform. Use existing
harnesses and child-process deadlines for any potential native hang.

**Disposition:** Open; unbounded main-thread join confirmed, deadlock risk
requires native reproduction during implementation planning.

## Consider

- Keep `FirstRender` optional on unsupported hosts; the contract explicitly
  forbids substituting ready/size events for renderer evidence. Its absence
  on Windows is not reported as a defect.
- Keep the narrow native restrictions and custom guarded runtime requirement.
  Correct error reporting when the requirement is missing; do not enable an
  unrestricted stock libmpv fallback to make playback tests pass.
- Unix numeric-locale initialization deliberately changes process state once.
  Its safety argument assumes other locale users are quiescent. Any change
  to startup/threading or this call must revisit that assumption; no concrete
  concurrent locale-reader failure was established here.

## What's already good

- The wire declaration generates prop/event names and fixtures check Android
  interoperability without coupling the shared contract to a toolkit.
- Handles, render contexts and callback state have explicit owners and safety
  comments, and restricted loads fail closed before upstream access.
- PiP and auxiliary windows reuse existing players rather than opening extra
  streams; source changes carry entry/token checks for important stale events.
- Native tests use generated clips and local servers, including origin
  isolation, retained frames, seeking and background reauthorization scenarios.

## File coverage

The checklist below records production-code inspection, with platform source
inspection distinct from platform execution. Test-heavy files were examined
for their harness design and relevant scenarios; that is not an exhaustive
audit of every assertion or an indication that failing native tests passed.

### Production source and build inputs

- [x] `Cargo.toml` — production source inspected, including conditional branches.
- [x] `src/events.rs` — production source inspected, including conditional branches.
- [x] `src/lib.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/ffi.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/gl.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/gtk.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/gtk_pip.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/gtk_placement.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/handle.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/held.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/ios.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/mac.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/mac_pip.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/map.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/mod.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/pip.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/test_host.rs` — test harness and relevant scenarios inspected; assertions not exhaustively audited.
- [x] `src/mpv/tests.rs` — test harness and relevant scenarios inspected; assertions not exhaustively audited.
- [x] `src/mpv/win.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/worker.rs` — production source inspected, including conditional branches.
- [x] `src/mpv/x11.rs` — production source inspected, including conditional branches.
- [x] `src/props.rs` — production source inspected, including conditional branches.
- [x] `src/tracks.rs` — production source inspected, including conditional branches.
- [x] `src/wire.rs` — production source inspected, including conditional branches.
- [x] `build.rs` — production source inspected, including conditional branches.

### Test fixtures and auxiliary targets

- [x] `fixtures/player-events.json` — shared synthetic wire fixture and its consuming tests inspected.
- [x] `fixtures/player-props.json` — shared synthetic wire fixture and its consuming tests inspected.

## Disposition summary

| Finding | Severity | State | Required evidence before closure |
|---|---|---|---|
| iptv-player-mpv-001 | Must fix | Open | Guard refusal completes exactly one load and reports a terminal error |
| iptv-player-mpv-002 | Must fix | Open | Quiet-handle command responses notify without another event or command |
| iptv-player-mpv-003 | Should fix | Open | Native teardown preserves UI progress and retires resources exactly once |
