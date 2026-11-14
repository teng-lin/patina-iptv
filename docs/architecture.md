# Architecture

This is a map of the code for someone about to change it. The rules it
explains are stated in [AGENTS.md](../AGENTS.md); the storage and transport
security model is in [security.md](security.md).

## Crates and the direction of dependencies

The workspace has fourteen crates. Dependencies only point
down: a crate may depend on crates in its own layer or below, never above.

```text
  Shell            iptv-app
                      │
  Application      iptv-runtime
                      │
  Native adapters  iptv-net        iptv-store-sqlite  iptv-player-mpv
                      │                   │
  Portable core    iptv-import  iptv-guide  iptv-search  iptv-library
                   iptv-providers  iptv-epg  iptv-settings
                   iptv-core  iptv-nav
```

| Crate | Depends on (workspace crates) |
| --- | --- |
| `iptv-core`, `iptv-nav`, `iptv-settings` | none |
| `iptv-providers`, `iptv-epg` | `iptv-core` |
| `iptv-guide` | `iptv-core`, `iptv-epg`, `iptv-providers` |
| `iptv-search` | `iptv-core`, `iptv-epg` |
| `iptv-library` | `iptv-core`, `iptv-providers`, `iptv-epg` |
| `iptv-import` | `iptv-core`, `iptv-providers`, `iptv-epg`, `iptv-settings`, `iptv-library` |
| `iptv-net` | `iptv-providers`, `iptv-import` |
| `iptv-store-sqlite` | `iptv-providers`, `iptv-epg`, `iptv-import`, `iptv-library` |
| `iptv-player-mpv` | `iptv-core`, `iptv-net`, platform NSH components |
| `iptv-runtime` | `iptv-core`, `iptv-epg`, `iptv-guide`, `iptv-import`, `iptv-library`, `iptv-net`, `iptv-providers`, `iptv-search`, `iptv-settings`, `iptv-store-sqlite` |
| `iptv-app` | portable crates, `iptv-runtime`, `iptv-net`, `iptv-player-mpv`, NSH |

| Layer | Crates | May use |
| --- | --- | --- |
| Portable core | `iptv-core`, `iptv-nav`, `iptv-settings`, `iptv-providers`, `iptv-epg`, `iptv-guide`, `iptv-import`, `iptv-search`, `iptv-library` | Plain Rust only. No NSH, no platform APIs, no blocking I/O, no clock, no async runtime. |
| Native adapters | `iptv-net`, `iptv-store-sqlite` | Tokio and reqwest; SQLite. No NSH. |
| Native application | `iptv-runtime` | Native services, Tokio, filesystem I/O and adapter value types. No NSH. Host JNI and credential brokers stay in the shell. |
| Player adapter | `iptv-player-mpv` | NSH native components, platform APIs, libmpv. The Android build contains only the wire contract and track parser. |
| Shell | `iptv-app` | NSH, the adapters, platform APIs. |

`tools/check.sh` builds every portable crate for `wasm32-unknown-unknown`.
That build is what keeps platform code out of them.

What each portable crate owns:

- `iptv-core`: UI-independent state. The guide model and focus, channel
  lists, favourites, menus, player panels, the playback health machine,
  multiview and picture-in-picture state, the add-playlist wizard.
- `iptv-nav`: remote and keyboard input. Key events become short or long
  presses, and a keymap resolves them to actions for the current context.
- `iptv-settings`: one declarative schema that drives storage, backup,
  validation and the settings screen.
- `iptv-providers`: M3U, Xtream and Stalker clients mapped to one model, with
  stable content ids and credential types that redact themselves.
- `iptv-epg`: the incremental XMLTV parser, channel matching, and compact
  per-channel schedules.
- `iptv-guide`: the guide schedule built from imported channels and the EPG
  store, behind the `Schedule` trait of `iptv-core`.
- `iptv-import`: the import pipeline that drives the two above.
- `iptv-search`: channel and programme search, and the search screen's state.
- `iptv-library`: playlist and guide identifiers, policy and list models,
  guide edits, and the playlists, guide sources and guide assignment screen
  states. SQLite re-exports its former model paths. Guide feed IDs are
  `iptv_epg::FeedId`; playlist IDs are `iptv_library::SourceId`.

## Sans-IO ports

The portable crates never perform I/O. They describe it, and an adapter does
it. There are three kinds of port.

**Requests as values.** A provider client in `iptv-providers` builds an
`HttpRequest` and parses an `HttpResponse`. It never sees a socket.

**`Fetch`** (`iptv-import/src/fetch.rs`) is the network boundary of an import:

- `fetch(request)` sends a request and reads the whole body;
- `fetch_stream(request)` returns once the status is known and yields the body
  in chunks, for large playlists and XMLTV files;
- `elapsed()` is the adapter's monotonic clock, for an end-to-end deadline.

The adapter owns transport policy: TLS, timeouts, redirects, decompression.
`iptv-net`'s `NetClient` is the native implementation (reqwest with rustls);
tests use an in-memory fake. The trait does not require `Send` futures, so a
single-threaded web adapter would fit as well.

**Sinks** are where an import puts its results, so nothing large is held in
memory:

- `CatalogSink`: catalog rows (groups, channels, films, series) as they are
  parsed, in steps that succeed or fail as a whole;
- `GuideSink`: one feed's programmes as they are parsed, with `finish_feed`
  or `discard_feed` at the end;
- `ProgressSink`: progress events for the import screen.

A `CancelToken` stops an import between requests, between chunks and while a
request is in flight. Time is a port too: every time-dependent function takes
`now` from its caller, which is why tests need no real waits.

## State, views and actions

State lives in the core; views render it and send actions.

1. **Input.** NSH reports a keystroke. `iptv-app/src/input.rs` turns it into an
   `iptv_nav::RemoteKey` with a phase, a `PressDetector` decides short or long,
   and `Keymap::resolve` maps it to an `Action` for the current `Context`
   (guide, full screen, a menu, an editor).
2. **State.** `input::apply` applies the action to the app state. That state is
   the `Ui` struct in `iptv-app/src/lib.rs`: a set of NSH signals, most of them
   holding a core type (`Focus`, `ChannelList`, `Panels`, `Menu`, and so on).
   The transition itself is a function of the core type, so it is tested in the
   core without a UI.
3. **Views.** Each region (header, rail, groups, grid, player panels) reads the
   signals it needs and re-renders when they change. Views hold no state of
   their own that matters after a rebuild.

Focus is restored by content identity, never by list index. A list can be
re-sorted, filtered or refreshed between two frames, so the state remembers
*which* channel or search result was focused. The last played channel is
saved the same way, by its content key.

The scenario tests in `crates/iptv-app/tests/app/` run this whole loop
headless, with the synthetic fixtures, each in its own process.

## The video layer

There is one persistent player surface under the UI
(`iptv-app/src/video.rs`). It is a native component named `tv.player`:

- on Android the Kotlin host (`hosts/android`) registers it, on Media3;
- on Linux, macOS, iPad and Windows `iptv-player-mpv` registers it, on libmpv.

Both sides implement the same contract of props (`source`, `paused`,
`pictureSize`, track choices, volume) and events (`state`, `size`, `error`,
`position`, `tracks`), summed up at the top of `video.rs`.

The contract is declared once, in `iptv-player-mpv`. `props.rs` names each
prop and nested field with its wire name; the serialized snapshot, the shell's
typed bindings and the names the shell and the mpv adapter use are all
generated from that declaration. `events.rs` holds the typed `PlayerEvent`
that the mpv adapter writes and the shell reads from either host. The Kotlin
host keeps its names in one file (`PlayerContract.kt`), and two synthetic JSON
fixtures (`iptv-player-mpv/fixtures`) hold the hosts together: the Rust tests
read and write them exactly, and the Kotlin test hands every fixture prop to
`setProp`, failing on a name it does not know, and compares every fixture
event with what the host emits. The shell supplies the playback transport
policy when registering the player.

The invariants:

- **Modes change layout only.** Guide preview, full screen and overlays are
  the same player laid out differently. Changing mode never recreates it.
- **Never hide, detach or re-parent the player view.** On Android that
  destroys its video surface. A surface with nowhere to show shrinks to
  nothing instead.
- **Multiview** adds one more `tv.player` per further tile while it is open
  and drops it when the tile goes. Surface 0 stays.
- **Picture in picture on the desktops** shows surface 0's player in a second
  window through the `tv.pip` view. There is no second player and no second
  stream (`mpv/pip.rs`).

Players never receive provider credentials. Each player talks to a loopback
relay in `iptv-net` (`playback.rs`), which fetches the stream, its redirects
and its HLS or DASH subresources, and scopes identity headers to the stream's
origin. Android reaches the same relay through JNI (`android_transport.rs`).

`player.rs` drives playback: it resolves a channel to a stream, feeds it to a
surface, and runs the core's health machine (retry with backoff, stall
detection, the error and waiting overlays).

## Import, staging and publish

An import never writes to the live catalog while it downloads.

1. **Stage.** The runtime starts the import off the UI thread. It creates a
   `CatalogStage`, a private scratch database opened through the store so that
   it is encrypted with the same key. `iptv_import::import_source_into` streams
   rows into it through `CatalogSink`.
2. **Publish.** Only when the whole import has passed does
   `Store::replace_catalog_stage` copy the stage into the store, in one
   transaction. Readers see the old catalog or the new one, never a mix.
3. **Guide.** The EPG works the same way: `import_epg` streams programmes into
   an `EpgStage` (its own file, written by its own thread in bounded batches),
   and `Store::replace_epg` publishes it in one transaction.

A failed or cancelled import publishes nothing: the stage is dropped and its
file deleted, and a new source is stored only once its first import succeeded.
A crash can leave a stale stage file, which is removed at the next start. A
stored source's channels and guide are on screen before its refresh reaches
the network, and stay when the refresh fails. Whether a stored guide is
downloaded again is decided by `iptv_import::plan_startup` from the
`epg.update_*` settings.

## Application runtime and commands

`iptv-runtime` is the native application layer. It builds and tests without
NSH or the player. The shell holds one `Rc<Runtime>` and installs a task host, timers
and an event sink. Runtime events enter the shell at
`runtime_events::apply`; views issue named commands rather than closures
that can reach a `Store`. `iptv-runtime/src/model.rs` exposes command values, without
exposing the connection or worker API.

Runtime owns the current schedule, library snapshot, refresh tickets and
attempts, combined-source parts, guide status, favourites, search indexes,
backup progress and settings. Its services own the store lanes, bounded
refresh pool, file writer and shared two-worker Tokio executor. Refresh
scheduling consumes foreground notifications and emits refresh requests;
it does not wake a sleeping device. Platform handles, caches and UI gesture
flags remain in the shell, with an explicit list in
`iptv-app/tests/app/architecture.rs`. The shell guards SQLite references and
statics; `iptv-runtime/tests/architecture.rs` guards toolkit and host dependencies.

Every local future and worker captures the owner that accepted it. A late
reply keeps that owner's settings, paths and epoch; it cannot adopt a newer
application's state. Weak context registries select the owner without owning
its data. Android's off-thread `onStop` callback uses a weak service registry
to flush the active owner's accepted file writes. Teardown drains accepted
writes, rejects later work and cancels imports and link resolves.

The private store service owns two long-lived connections to the encrypted
catalogue. The writer serializes mutations, staging and reads that must
follow queued writes. The read-only connection serves guide, search and
organization queries without waiting behind a publish transaction. Both
live on dedicated threads; no SQLite operation runs on the UI thread.
Each request carries its application epoch. Recursive requests to the same
worker and requests from a retired owner are rejected.

Imports use the bounded refresh pool so their blocking store waits cannot
hold Tokio's network workers. Channel indexing and organization preparation
run off the UI thread. The final `EpgSchedule` wraps prepared data and local
callbacks on the UI thread, retaining its `Rc` model. The storage prerequisite
measured the expensive preparation at 100k channels; no `Rc` to `Arc` rewrite
is needed for that work to leave the UI thread.

The shell constructs `Runtime::new(Ports)` with its private directory,
credential-key callback, OS backup-exclusion hook and startup instrumentation.
Native workers capture those inputs with their accepting service. JNI entry
points and credential-broker implementations remain in the shell. The host's
local task executor and timers are installed separately through `bind_tasks`.
The shell retains one strong handle; local context selection uses weak handles
and restores the accepting owner on every poll, including nested callbacks and
unwinding. Local controllers use explicit state accessors; store connections,
writer queues and native service handles remain private.

Run `cargo test --locked -p iptv-runtime` to test application services without
linking a display toolkit or reaching an OS credential broker. Public-API tests
cover suspended task delivery, unwinding, weak owner lifetime, and native worker
capture of the original private directory and key broker.
Scenario processes stay isolated because NSH and the player have their own
platform globals.

## The NSH boundary

NSH is the UI toolkit: a retained shadow tree with signals, laid out and
drawn by a native host per platform. The shell and player component adapter
depend on it; the application runtime does not import it.

- `nsh-core` and `nsh-base` are used on every platform. The platform crates
  (`nsh-platform-linux`, `-macos`, `-ios`, `-windows`) are target-specific
  dependencies. Android's platform crate is pulled in by the generated runner.
- `#[native_module]` in `lib.rs` registers the `App` component.
  `#[native_components(<platform>)]` registers `tv.player` for each libmpv
  platform.
- `nsh package` generates a runner workspace under `crates/iptv-app/.nsh/`
  that links the app library. That workspace has its own lock,
  `tools/ci/package.Cargo.lock`. Windows is the exception: it ships the app
  crate's own executable, with no generated runner.
- NSH is a path dependency at `../nsh`, on the fork's `tv-keys` branch. The
  exact commit is in [`nsh.rev`](../nsh.rev), which CI and `tools/check.sh`
  enforce. Patches to the fork stay small, carry tests and go upstream later.

Pure models and state machines belong in the portable crates. Native
application orchestration belongs in the runtime; adapters own platform I/O.
