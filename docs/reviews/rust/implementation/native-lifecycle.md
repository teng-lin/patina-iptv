# Native player lifecycle investigation and W8 implementation advice

This is an independent review of the internal native adapter, using the Rust
review and best-practices skills, edition 2024 and Rust 1.99.1. It covers W14
(`iptv-player-mpv-003`) and the existing completion phases relevant to W8
(`iptv-player-mpv-001/002`). It does not approve an unexamined W8 implementation.
No source edits, Cargo invocations, builds or commits were performed. Three
already-built native scenarios were run as separate bounded child processes.

Review worktree: `/home/blackmyth/src/patina-rust-review/patina-iptv`, initially
based on PR #95 at `97dbc6b00ac9f3afba966bd4def112f11d8501f3`. The player source
was unchanged when this investigation began; root is implementing other packages.

## W14 disposition

Retain W14 as an investigated ownership risk; do not apply the original report's
illustrative destructor reorder. A UI-thread join and its unrestricted wait on a
running synchronous native command are established. The bundled render API
explicitly forbids that wait dependency while rendering is active. No permanent
production deadlock was reproduced, no portable/native time bound was established,
and no replacement teardown protocol has been validated. The observed existing
ownership order is retained for this investigation, with the residual wait
recorded rather than described as fixed.

The three bounded existing checks pass with the exact guarded Linux runtime.
They exercise shutdown, refusal recovery, real rendering and ordinary context
recreation; they are not a deterministic removal-during-command experiment.
A lifecycle redesign would require that additional experiment and affected-host
execution before implementation. This evidence does not make the unproven
redesign a prerequisite for independent correctness fixes in the same PR.

## Ownership and teardown graph

- The UI component owns `Rc<Inner>`. `PLAYERS` holds only weak entries
  (`mpv/mod.rs:148`), and queued main-loop work looks the player up by ID.
  Destruction removes that entry before callbacks can look it up again.
- `Inner` owns the platform view, an `Arc<Mpv>`, optional active and retired
  Workers, its render-context pointer and an `Arc<Waker>` (`mod.rs:241`).
- A Worker owns another native-handle Arc; its thread owns a further Arc plus
  queue, quit and guard state (`worker.rs:127`, `:160`). The thread's notify
  closure retains the Waker Arc. Its end guard notifies after the note sender
  disconnects, including unwinding.
- The render callback receives a raw pointer to the same Waker allocation
  (`mod.rs:666`). It posts a coalesced main-loop ID lookup instead of calling
  control functions or rendering in the callback (`mod.rs:168`).
- `Inner::drop` removes registry/PiP visibility, drops active and retired
  Workers, frees the render context, then takes the final native handle
  (`mod.rs:1098`). `Worker::drop` sets quit, wakes the event wait, and joins
  (`worker.rs:209`). The wake interrupts `mpv_wait_event`, not an ordinary
  synchronous native command already executing.
- `free_render` makes the platform context current, removes the update callback,
  frees the render context, and clears retained GL resources (`mod.rs:678`).
  The platform view and Waker remain fields until after this destructor body.
  `Mpv::drop` calls blocking `mpv_terminate_destroy` (`handle.rs:242`).
- GTK realize/unrealize callbacks are weak. Actual unrealization retires borrowed
  PiP areas before freeing the render context (`gtk.rs:96`). Ordinary PiP uses
  another area sharing the primary context, without moving the primary area or
  opening another player (`gtk_pip.rs:1`). Surface 0 must stay mounted.
- macOS uses the layer's current CGL context (`mac.rs:355`) and forgets its
  borrowed PiP view before dropping the component (`mac.rs:365`). iOS removes
  lifecycle observation before releasing its component (`ios.rs:548`) and uses
  the view target's current EAGL context. Windows uses the software render API
  (`win.rs:153`), so make_current has no GL work; the control/render ownership
  and callback-retirement constraints still apply. These hosts were source-read,
  not executed.

The graph keeps native ownership and callback data alive through cleanup. It
also has a wait edge from the UI/render thread to the control thread while a
render context can still exist. Removing that edge without considering the
remaining native-handle destructor would leave another blocking operation on
that thread; moving only JoinHandle destruction is not a complete design.

## Exact runtime contract examined

The local installed headers accompany the pinned Linux native release. The
library SHA-256 was recomputed as
`7c3a72bf7522da2d1be21af43362329f18b24b8d6ac1955bec5614f41cadc683`, matching
`tools/mpv-runtime-unix/runtime-linux.json`. The manifest tag is
`mpv-runtime-linux-8bd2761d32a14451f45398048db01beee83c525dd6df946206faa61edda49fe7`.

Primary local evidence is under
`/home/blackmyth/src/patina-iptv/target/mpv-runtime-linux/include/mpv/`:

- `render.h:62–83`: serialize render API use, use the original current GL
  context, and avoid a render-thread wait dependency on ordinary client calls.
  Without advanced control, violating this can cause native timeout degradation;
  advanced control makes the application responsible for avoiding a permanent
  deadlock. Patina does not request advanced control in create_render.
- `render.h:120–125` and `:739–748`: freeing an active render context disables
  video and must precede core destruction. That permission does not establish
  a validated thread/ownership order for Patina's callback and view graph.
- `client.h:518–542`: terminate_destroy blocks and concurrent use of that same
  handle during destruction is unsafe. Worker/native ownership must be retained
  until control work is finished.
- `client.h:1691–1735`: zero-time event waits poll, negative waits are infinite,
  and wakeup interrupts the event wait. Wakeup does not cancel arbitrary
  synchronous native commands.

The proposed fix must honor these concrete contracts, rather than infer that
freeing rendering early, detaching a thread, or adding per-removal reapers is safe.

## Bounded native evidence

Used the already-built executable
`/home/blackmyth/src/patina-iptv/target/debug/deps/iptv_player_mpv-a8f4f1fe06a29f70`
with the pinned lib directory in LD_LIBRARY_PATH and mandatory player tests
enabled. Each exact test ran in a fresh process with a 45-second outer deadline;
the runner would kill and reap the process group on timeout. No timeout or skip
occurred, and no process was left running.

| Existing scenario | Result | Elapsed |
|---|---|---|
| `mpv::worker::tests::the_thread_ends_with_mpv` | Passed | 0.06 s |
| `mpv::worker::tests::a_refused_load_and_a_stop` | Passed | 0.03 s |
| `mpv::tests::a_source_plays_through_the_render_api` | Passed, real surfaceless EGL | 11.15 s |

Sources/results are retained in
`/tmp/patina-rust-fixes-20261116/native-lifecycle-existing-results.json` and
`native-lifecycle-existing-{1,2,3}.log`. These are reused baseline binaries,
not a rebuild of the in-progress implementation or proof of its final tree.
The native GTK executable was inventoried, but Xvfb is absent from PATH here,
so the private-display scenario was not run. No Android/Apple/Windows execution
is claimed.

## Required experiment before changing lifecycle order

Use the existing Linux player unit module or native GTK harness, in a child of
that same executable, rather than creating a new test target. The experiment
must make the key interleaving observable:

1. Create a real render context and establish decoded synthetic video.
2. Queue an actual supported native control operation. Add a test-only entry
   barrier so the parent knows the worker entered that operation; do not infer
   this from a command merely being queued.
3. Remove the component/tile on the render/UI thread while that operation is
   held. Observe UI progress and retirement ordering; release it from a separate
   helper/parent, not the blocked UI. Give the parent an outer deadline and kill
   and reap the child on failure.
4. Distinguish a deliberately blocked test operation (which demonstrates the
   join's structural wait) from a native operation that demonstrably waits for
   rendering. Only the latter can establish the suspected native dependency
   cycle. Record both markers and native outcome; do not invent a production
   deadlock from a synthetic barrier alone.
5. Before a replacement protocol is accepted, assert callback retirement once,
   no handle destruction during control/render use, eventual resource release,
   bounded retained teardown ownership, and preserved primary surface/PiP
   behavior. Run affected hosts because valid GL cleanup contexts differ.

If no native unsafe wait is demonstrated, preserve the observed ownership
tradeoff and leave the limitation explicit. Do not assert a global time bound
from the short elapsed times of the passing tests above.

## W8 completion and notification advice

### Early load refusal completion

**Rule:** Every accepted asynchronous operation needs a terminal outcome; keep
invariant-bearing native state explicit (rust-best-practices, errors/ownership).

The three early guard refusal paths in `execute` (`worker.rs:328`, `:355`, `:374`)
return only TransportError. That note records clues and does not consume
State.loads or emit a terminal event. Relay-start failure already returns
Loading(None); successful dispatch returns Loading(entry). Every path must
consume exactly one accepted load, with the latest request/entry identity
checked before failing current UI state. Existing FIFO load accounting must not
be bypassed by the new failure variant.

A hook-substitution failure (`worker.rs:263`) is a different phase: execute
already sent Loading(entry), so its load count has already been consumed before
the hook result is handled. Use a distinct terminal/guard-retirement outcome
with relevant identity; do not route it through another decrement. Keep source
replacement, old events, and the permanently retired guard state consistent.
Ordinary TransportError clues should retain end-file classification behavior.

Required regressions include every pre-load refusal, two queued loads, a refused
load followed by success, stop before delivery, and a post-Loading hook failure.
Assert one terminal error/idle, zero leaked accepted-load count, no double
consumption, and no unauthorized upstream request.

### Notify responses before returning to an infinite wait

**Rule:** Do not wait for unrelated work before publishing an already-completed
operation (rust-review, correctness/concurrency).

After command execution has queued any owned notes, use a zero timeout for the
first event drain (`worker.rs:241`); otherwise keep the normal infinite wait.
Retain the existing subsequent nonblocking drain and one coalesced notify.

A test using Format on an idle handle is insufficient because it can return no
notes at all. Use a paused loaded handle with a known Position/Timeline reply,
a drained native queue and an acknowledged wait barrier, or a test-only
response-only command. Assert the worker's notification arrives without sending
another command or polling its response receiver directly. A test which merely
reads the channel does not exercise the broken UI notification path.

## What's already good

- The callback posts ID-based, coalesced main-loop work and owns no raw Inner.
- Render resources are freed with a current platform context before destroying
  the native core, while the callback data remains alive.
- Guard failures stay closed to provider-controlled destinations; the fix must
  retain the guarded runtime and native capability requirements.
- Existing real-render and worker scenarios can supply native evidence without
  adding another integration executable.

## W8 implementation delivered for coordinated validation

Root subsequently authorized this reviewer to implement W8 in the player crate.
The patch changes only `mpv/mod.rs`, `mpv/worker.rs` and `mpv/tests.rs`; the
lifecycle order and native guard requirements remain unchanged.

- Each Cmd::Load now owns a fresh internal LoadId. Loading, early LoadRejected
  and later GuardFailed outcomes retain that identity. Arc identity avoids ticket
  wraparound collisions while a command/reply is retained.
- State retains only the current identity and a pending-completion flag, replacing
  the aggregate outstanding-load counter. Stop/new open invalidates the current
  identity. Superseded and duplicate replies cannot complete a newer request.
- Every early guard refusal returns LoadRejected with the existing container
  unsupported classification. Its current request emits one error/idle outcome.
  An accepted Loading consumes the pending phase once. A later hook failure
  retires the guard and reports failure against the current request/entry without
  consuming the completed phase again. Rejected native destinations stay blocked;
  the retired transport is closed.
- A batch with completed response notes now drains events with an initial zero
  timeout before notifying. Quiet idle still uses an infinite event wait.
- Existing newest-load tests now cover request identity, duplicate replies, late
  older refusal after a newer success and stop fencing. Added state tests cover
  a refusal followed by a new accepted load, duplicate terminal errors and later
  hook failure without consuming accounting again.
- A worker test executes all three native guard refusal branches with synthetic
  failure injection that can only remove authority, checking one matching
  completion, an empty native playlist, released relay and zero upstream request.
- A dedicated test-only response command consumes queued events/wakeups on an
  idle handle and returns one owned reply. Its regression waits for notify
  before inspecting notes, and sends no second command; the old infinite wait
  cannot rely on an unconsumed command wakeup to pass it.

Direct rustfmt and diff whitespace checks passed. No Cargo/build invocation was
run by this reviewer, as root coordinates the build cache and validation. Root
should run the shared temporary environment runner with these commands:

```sh
cargo test --locked -p iptv-player-mpv --lib
cargo clippy --locked -p iptv-player-mpv --all-targets -- -D warnings
```

The native library selection must remain the verified guarded runtime. Relevant
new exact test names are:

- `mpv::tests::guard_refusals_complete_once_and_a_new_load_can_succeed`
- `mpv::tests::stopped_or_replaced_loads_ignore_late_guard_failures`
- `mpv::worker::tests::each_native_guard_refusal_completes_its_request_before_network_io`
- `mpv::worker::tests::response_only_commands_notify_without_another_native_event`

At the earlier W8 checkpoint, the bounded native results remained baseline
evidence and the pending-command experiment had not yet been added. The later
experiment below now covers that synthetic interleaving; production teardown
order remains unchanged. A deliberately held command is distinct from a
proven native/render dependency cycle.

## Bounded pending-control component-removal experiment

Added `mpv::tests::dropping_a_render_owner_waits_for_its_pending_control_command_in_a_bounded_child` to the existing player unit executable. It starts actual guarded native playback with a live surfaceless render context, acknowledges entry to a test-only pending control command, then drops the real component owner on its UI thread. A test-only Worker drop acknowledgement tells an independent releaser that teardown reached the join. The releaser verifies removal has not returned and native Arc ownership is retained, then releases the synthetic command after 150 ms. The child verifies removal waited for that hold and the native owner is released afterward.

The synthetic command itself has a five-second receive deadline. Its parent runs the same exact test in a child of the same executable and kills/reaps that child if the entire experiment exceeds thirty seconds. No detached ownership, unbounded reaper, new executable, production timer or destructor ordering change was introduced. The platform GL context stays alive on the teardown thread until removal completes.

This experiment measures the existing UI-thread join and retained native ownership through a deliberately pending command. It cannot establish that a production synchronous libmpv operation deadlocks, nor that ordinary destructor execution has a fixed bound. The evidence continues to support retaining the established worker-before-render-before-handle order. Root must run the new case against the guarded pinned native runtime; it is not yet verified by this document's author.

## Decoder pause/resume observation correction

The native recording pause case initially failed to observe RecordingFile's idle-write timeout after sixty seconds, despite ready/paused native playback. The production guarded PlaybackTransport imposes its own fifteen-second downstream write deadline. That timer begins before upstream backpressure reaches RecordingFile's independent fifteen-second timer, so the outer relay can drop reqwest first and the inner recording relay sees disconnect rather than its own timeout. Increasing fixture bytes cannot make the inner timeout a reliable decoder-path oracle.

The corrected same-harness decoder case observes the worker's actual owned PlaybackRelay downstream timeout through a test-only response command. It then still requires a new authenticated RecordingFile range request, current native timeline after the seek, a rendered frame, unchanged native playlist entry and no playback error. Both production timers and native URL guards remain unchanged. Separate direct RecordingFile tests cover its own progress-idle policy; this native case proves the real stacked playback path. Root runs it separately because it deliberately waits through a real idle deadline. Pending validation; no decoder success is claimed here.

Root reports the earlier W8 guarded suite passed: 64 tests, zero failed or ignored, approximately nineteen seconds. The new W12/W14 native cases are additional checks.

## Coordinated native result

The guarded Linux player unit suite passed 66 tests, including the real decoder
pause/range-resume case and the bounded pending-control child experiment.
Command: `cargo test -p iptv-player-mpv --lib -- --test-threads=1` through
the coordinated pinned-runtime runner. Log: `player-final-tests.log` in the
temporary run directory. This establishes the stated synthetic cases only;
GTK, Android and other host gates remain separate.

## Coordinated native validation

Root's guarded `player-final-tests.log` now reports 66 passed, zero failed/ignored in 42.55 seconds, including the corrected decoder pause/range-resume case and bounded pending-control child. The child measured its acknowledged synthetic hold at 150.109153 ms and actual component removal at 151.764968 ms. This confirms the intended experiment's UI-thread wait and retained lifetime through completion; it continues to make no production-deadlock or fixed-destructor-bound claim.
