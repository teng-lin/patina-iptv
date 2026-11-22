# Patina on browser TVs: target architecture, crates and boundaries

Status: proposed on 2026-11-21; decisions updated after a second round of spikes the same day. Steps 1a to 1e are implemented (see their status lines); step 2 runs Patina on the TV (`hosts/tizen-nsh`); steps 3 and 4 are measured below ("Module size and launch"), and the 2020 set stays on the bridge. This is the Patina integration plan that M7 of
[External NSH TV support](2026-11-15-nsh-tv-platform-support.md) asks for. It is
based on the current code and on spikes run on a 2020 Samsung TV (Tizen 5.5).
Sections marked **Today** describe the code as it is; everything else is a
proposal that has not been implemented.

## Decision

1. Patina keeps **one UI** (the NSH shell, `iptv-app`) and **one application
   runtime** (`iptv-runtime`) on every platform, including browser TVs
   (Samsung Tizen first, LG webOS later).
2. The application stack is **made to build for the browser**, not rewritten
   for it. A port is introduced only where two implementations exist (network,
   execution, private files, credentials, playback transport, player). The one
   SQLite store is built for both targets.
3. **NSH stays unmodified.** TV rendering is an external NSH platform owned by
   the NSH TV extension, never a patch to NSH's own web runtime.
4. The plain-JavaScript Live TV host (`hosts/tizen`, PR #190) is a **frozen
   bridge**: bug fixes only. It is retired per TV generation once the NSH build
   meets the gates in [Migration](#migration).

## Constraints measured on the device

Samsung UN55TU7000 (2020, Tizen 5.5, Chromium 69, ARMv7, 4 cores). Details and
how to rerun are in `hosts/tizen/README.md` and the spike repository
`../nsh-platform-tizen/README.md`.

| Constraint | Measured |
| --- | --- |
| WebAssembly features | MVP plus sign-extension and mutable globals only. Stable Rust's std is rejected; it needs a rebuilt std (pinned nightly). No 64-bit integers across the JS boundary. |
| JavaScript and CSS | ES2017. No optional chaining, nullish coalescing, `WeakRef`, flex `gap`, `min()`/`clamp()`. NSH's web runtime uses these, so it cannot load. |
| Compiling a module | About 3 s per MB, at every first launch after an install or update. |
| Caching a compiled module (IndexedDB) | Works at 4.5 MB and 6.2 MB. Fails at 12.5 MB (transaction timeout, then quota). |
| NSH engine through a DOM renderer | Guide-like screen: key to painted frame median 62 ms; about 29 ms of engine and text layout per focus move. |
| SQLite (rusqlite 0.42 with `sqlite-wasm-rs`) | Builds for the TV's feature set (1.6 MB). 10,000 inserts, an index and a query take 321 ms. |
| Guide import (`iptv-epg`) | A 68 MB XMLTV guide parses in about 9 s of CPU; 26 MB of Wasm memory. |
| Network | A packaged app can fetch any origin. It cannot set `User-Agent` or `Cookie`. |
| Video | AVPlay under a transparent page works; channel change about 1.6 s. A second AVPlay instance stays idle on this model. |
| Storage | IndexedDB quota about 92 MB. No keystore for web apps. |

Not measured: any TV newer than 2020, LG webOS, background behavior (Samsung's
documentation says an app's JavaScript is paused while hidden).

## Target architecture

```text
 Hosts         NSH-generated hosts            hosts/android                 hosts/tizen
               (desktop, iPad)                (Java, Media3 player)         (page, AVPlay player, packaging)
                      │                             │                             │
 Shell         iptv-app: screens (same on every platform) + host/<platform> (assembles Ports)      iptv-tui
                      │
 Application   iptv-runtime: owner, orchestration and services; all I/O through Ports
                      │
 Adapters      iptv-net          iptv-web           iptv-store-sqlite        iptv-player-mpv
               (native network)  (browser adapters) (SQLite, both targets)   (libmpv; wire contract for all)
                      │
 Portable      iptv-import  iptv-guide  iptv-library  iptv-search
               iptv-providers  iptv-epg  iptv-settings  iptv-core  iptv-nav

 External      NSH (pinned upstream, unmodified)
               NSH TV extension: remote input (vendor/nsh-tv), Tizen platform (engine → DOM)
```

Dependencies point down only. What changes from today is the middle: the
application layer stops naming native adapters, so the same shell and runtime
link against either the native or the browser adapters.

### Rules that keep the boundaries

1. **Portable crates**: plain Rust. No NSH, no I/O, no clock, no threads, no
   async runtime. They build for `wasm32-unknown-unknown` (CI checks this
   today).
2. **`iptv-runtime`**: no NSH. All I/O goes through `Ports`. No
   `std::thread`, Tokio runtime, `std::fs` or local-time lookup outside
   modules gated by the `native` feature. New CI check: the crate builds for
   wasm32 without `native`.
3. **`iptv-app` screens**: no `cfg(target_os)`, Tokio, `iptv-net`, keyring or
   JNI outside `host/`. New CI check: the crate builds for wasm32.
4. **Adapters execute, they do not decide.** Refresh timing, list building,
   admission and retry policy stay in portable crates or the runtime.
5. **JavaScript draws and performs platform I/O.** It holds no product
   decisions. The frozen bridge is the one exception until it is retired.
6. **A port needs two implementations.** No trait is added for a boundary
   with one implementation.
7. **Budgets are part of the contract.** The TV module's size and launch
   times are recorded and reported by CI, as NSH records its baselines.

## Ports

`Ports` (`iptv-runtime/src/ports.rs`) is already how a host constructs an
owner. It grows from platform inputs into the full I/O boundary.

| Port | Contract | Native | Browser TV | Today |
| --- | --- | --- | --- | --- |
| Network | `iptv_import::Fetch`: request, streamed body, elapsed, sleep. The adapter owns TLS, timeouts, redirects and the destination policy. A factory makes clients per configuration (the Stalker link client keeps cookies). | `iptv-net` `NetClient` | `iptv-web` `HostFetch` | Trait exists. The runtime holds `iptv_net::NetClient` directly. |
| Execution | Run store jobs and blocking work; spawn local tasks; delay. | Store reader and writer threads, Tokio pool | Cooperative, on the owner's thread | `TaskHost { spawn, delay }` exists for the owner. Services start their own threads. |
| Store | The existing `iptv-store-sqlite` API. Not a trait: one implementation, two SQLite builds. Opening, persistence and the key are host inputs. | SQLCipher file, WAL, key from the credential broker | `sqlite-wasm-rs`; persistence and encryption are open decisions | Native only. |
| Private files | Read and atomically replace small named blobs (settings, markers). | `file_writer` thread | IndexedDB key-value (`iptv-web` has it) | Direct `std::fs`. |
| Credentials | `database_key` broker. | Keyring, Android Keystore via JNI | None, or an app-held key | Exists in `Ports`. |
| Playback transport | Turn a resolved source into what the player opens. | Loopback `PlaybackRelay`, which scopes headers per origin | Identity: AVPlay fetches the stream itself | Concrete `iptv_net::playback`. |
| Player | The `tv.player` wire contract: props and events. | libmpv; Media3 on Android | AVPlay in `hosts/tizen` | Contract in `iptv-player-mpv` (`wire.rs`); Android already implements it in Java. |
| Recording host | Capture, wake scheduling, background ownership. | `iptv-net` capture, OS scheduling | Unavailable (capability model reports it) | Exists; native only. |
| Clock and entropy | Wall time, local offset, random bytes. | OS | Host imports | Portable crates already take `now`; the runtime reads the OS. |

## Crates: responsibilities and boundaries

### Portable core (unchanged)

| Crate | Responsibility | Depends on |
| --- | --- | --- |
| `iptv-core` | Screen state machines and models: guide focus, lists, favorites, menus, panels, playback health, multiview, wizard, parental policy, capabilities, translations. | none |
| `iptv-nav` | Remote and keyboard input: key phases, short and long presses, the keymap to actions. | none |
| `iptv-settings` | One declarative schema that drives storage, validation, backup and the settings screen. | none |
| `iptv-providers` | M3U, Xtream and Stalker clients as request descriptions and response parsers; stable content ids; secrets that redact themselves; the content address policy. | core |
| `iptv-epg` | XMLTV parser, channel matching, compact guide store, guide snapshot format. | core |
| `iptv-guide` | The guide schedule from a catalog and a guide store; stream resolution for playback. | core, epg, providers |
| `iptv-library` | Playlist and guide identifiers and policy; playlists, guide sources and assignment screen states. | core, epg, providers |
| `iptv-search` | Channel and program search; the search screen's state. | core, epg |
| `iptv-import` | The import pipeline over `Fetch` and sinks; the guide refresh policy. | core, epg, library, providers, settings |

Boundary: these never learn which platform they run on. They are the only
place product rules may live besides `iptv-runtime`.

### `iptv-runtime` (application)

- **Responsibility**: the application owner. Orchestrates imports, the
  library, guide refresh, favorites, search, parental admission, on-demand
  browsing, playback opening, settings, and the query/inspection/repair facade
  of `docs/runtime-api-v1.md`.
- **Today**: 39 of its 46 top-level source files use Tokio, threads, files or
  SQLite. Work is closures over the concrete `Store`. It starts threads in
  `store_service`, `workers`, `file_writer`, `favorites` and
  `credential_store`, and a multi-threaded Tokio executor. For wasm32 its
  dependency tree contains `tokio`, `reqwest`, `rustls`, `ring` and `mio`,
  and the build stops at `getrandom`.
- **Target**: the same owner and the same decisions, with I/O behind `Ports`.
  A default `native` feature holds what only native hosts can do: recording
  capture, backup files, the credential store, staged guide probes, the
  thread-based executors and the `iptv-net` dependency. Without it the crate
  builds for wasm32.
- **Must not**: import NSH; name `iptv-net` outside `native`; contain platform
  `cfg`; expose store records across the declared boundary (already a rule).

### `iptv-app` (shell)

- **Responsibility**: Patina's screens in NSH, rendering `iptv-core` state and
  sending actions; the persistent video layer; the theme.
- **Today**: 68 source files; 37 call the runtime. Platform dependencies are
  already gated by OS. It always links `iptv-net` (image loading in
  `content_image.rs` starts its own Tokio runtimes; the playback relay),
  `iptv-player-mpv` and multi-threaded Tokio. Credential keys, the recording
  host and Android transport sit beside the screens.
- **Target**: two parts in one crate.
  - *Screens*: NSH, portable crates, `iptv-runtime` and the player wire
    contract only. Images load through the runtime's network port.
  - *`host/<platform>`*: the only code with platform `cfg`. Each builds
    `Ports` for its platform: directory, key broker, network factory,
    executor, playback transport, recording schedule. `host/web` uses
    `iptv-web`.
- **Must not**: decide product behavior that belongs in `iptv-core`; reach a
  platform API from a screen.
- It becomes a crate of its own only if the module boundary proves too weak.

### Adapters

| Crate | Responsibility | Boundary |
| --- | --- | --- |
| `iptv-net` | Native network: reqwest client implementing `Fetch`, the loopback playback relay, recording capture. | Native only. Referenced from `iptv-runtime`'s `native` feature and native `host/` modules. Never in a browser build. |
| `iptv-web` | Browser adapters: `HostFetch`, the poll-after-delivery executor, timers, entropy, key-value persistence, the host ABI (imports and exports). Temporarily also the bridge's Live TV state (`app.rs`, `store.rs`). | Portable crates only. No decisions outside the bridge module, which is deleted with the bridge. |
| `iptv-store-sqlite` | All persistence: sources, catalog replaced per refresh, user overlays by stable id, guide windows, search queries, migrations. | One implementation. Target: also builds for wasm32 through rusqlite's `sqlite-wasm-rs` backend. It must not assume two connections or a file system. |
| `iptv-sqlcipher-init` | The native SQLCipher initialization barrier. | Native only. |
| `iptv-player-mpv` | libmpv surfaces, and the player wire contract every host implements. | The wire contract (`wire.rs`, `props.rs`, `events.rs`, `tracks.rs`) builds everywhere, as it already does for Android. libmpv stays native. Splitting the contract into its own portable crate is the cleaner end state; a wasm32 `cfg` is the smallest first step. |
| `iptv-tui` | Optional terminal host. | Unchanged. |

### Outside the workspace

| Owner | Responsibility | Boundary |
| --- | --- | --- |
| NSH (`../nsh-patina`, the commit in `nsh.rev`) | Engine, layout, reactive graph, components, the external-platform protocol. | Pinned upstream. No TV patches. |
| NSH TV extension (the `nsh-tv` workspace of the 2026-11-15 plan; `vendor/nsh-tv` is its input adapter) | Remote input normalization. The Tizen platform: engine in WebAssembly, mount batches encoded as operations, text engine, key delivery, a page runtime that applies operations to the DOM, a native-component registry, module loading and the compiled-module cache, the `nsh-platform-tizen` command. | Knows nothing about Patina. Requires no NSH change. The spike `../nsh-platform-tizen` is its starting point. |
| `hosts/android` | Android host: Java runtime, Media3 player, JNI brokers. | Unchanged. |
| `hosts/tizen` | Tizen packaging and signing, the TV build (rebuilt std, feature flags, size report), Patina's host services for the page (fetch, storage, entropy, clock), the AVPlay implementation of `tv.player`, lifecycle and screensaver. Temporarily the bridge UI. | No list, focus, provider or refresh logic outside the frozen bridge. |

The 2026-11-15 plan's first approach for Tizen was to reuse NSH's web
renderer. On Chromium 69 that gate fails (see Constraints), so the extension
owns a renderer built on mount batches instead. Reusing NSH's web renderer on
newer TV engines is untested.

## What each product links

| Product | Host | Shell | Runtime | Network | Store | Player |
| --- | --- | --- | --- | --- | --- | --- |
| Linux, macOS, Windows | NSH-generated | `iptv-app` + `host/<os>` | `native` | `iptv-net` | SQLCipher | libmpv |
| iPad | NSH-generated | `iptv-app` + `host/ios` | `native` | `iptv-net` | SQLCipher | libmpv (MPVKit) |
| Android TV and tablet | `hosts/android` | `iptv-app` + `host/android` | `native` | `iptv-net` | SQLCipher | Media3 (Java) |
| Tizen, NSH build (target) | `hosts/tizen` + NSH TV extension | `iptv-app` + `host/web` | without `native` | `iptv-web` | SQLite in Wasm | AVPlay (JS) |
| Tizen, bridge (today, frozen) | `hosts/tizen` | JavaScript page | `iptv-web` bridge module | `iptv-web` | guide snapshot in IndexedDB | AVPlay (JS) |

## Boundary leaks today and where they go

| Leak | Evidence | Resolution |
| --- | --- | --- |
| The runtime mixes decisions with native execution. | Jobs are closures over `Store`; services own threads; `NetClient` is held directly. | Network and execution ports; `native` feature (steps 1a, 1b, 1d). |
| The store is native only. | Documented "not wasm"; SQLCipher with vendored OpenSSL. | Build it for wasm32 with `sqlite-wasm-rs`; host-supplied opening and persistence (step 1c). |
| The shell mixes screens with host glue. | `content_image.rs` runs its own Tokio runtimes; credential, recording and Android code beside the views. | `host/<platform>` modules; images through the network port (step 1e). |
| The player contract lives in a crate named for one player. | `iptv-player-mpv` exports `PLAYER_VIEW`, props and events to every host. | Build the contract for wasm32; split it out when convenient. |
| `iptv-web` re-implements runtime logic. | Its guide lifecycle mirrors `iptv-runtime/src/stored.rs`. | Deleted when the TV runs the real runtime. |

## Migration

Each step keeps every native test passing. Steps 1a, 1b, 1d and 1e landed
together in one pull request (#195), each as its own commits; 1c is #193.

- **0. Bridge.** Merge PR #190. Freeze the JavaScript host.
- **1a. Network port.** The runtime takes a client factory over `Fetch`
  instead of `iptv_net` types. *Gate*: no `iptv_net::` outside `native`.
  *Status*: done (`Ports::network`, `network.rs`). The Stalker link client
  is the same factory's client, cached per configuration; Stalker's
  cookies are request headers, so nothing native stays in it. A browser's
  fetch cannot send `Cookie` or `User-Agent` (forbidden headers), so Stalker
  still does not work there (Still open, 3). Artwork loads through it too.
- **1b. Execution port.** Store queues, workers and the Tokio pool sit behind
  an executor supplied by `Ports`. *Gate*: no thread or runtime creation
  outside `native`. *Status*: done (`Ports::execution`, `execution.rs`):
  lanes, long-lived workers and a per-owner task executor, native as
  before; `execution::Cooperative` for the browser, which also drives
  `Sliced` jobs at most one step per turn, the rest of the turn going to
  reads (the shape of the store's resumable publish). Differs from the plan: the refresh pool, the file writer and
  the favorites writer are blocking workers, which a cooperative host
  does not have, so in the browser they report themselves unavailable;
  imports there need the pool's work as local tasks over the async store
  calls (next step); until then `Runtime::has_workers` is false there and
  the shell offers none of those services. The cooperative lanes share one
  turn and, in the browser, one connection (the store has no reader
  there); a `Sliced` step's budget is rows, as the store's `step(rows)`.
  The runtime's clocks are `crate::time` (std's natively, the host's in a
  browser build); the store's inspection API still takes std's `Instant`,
  so bounded inspections take #193's `*_until` form, the expiry read on the
  runtime's clock. In the browser a request carrying an origin-bound
  header (a portal's identity, a playlist's API key) refuses every
  redirect, a same-origin one included: the page cannot tell them apart
  before following, so such a source fails there where it redirects. The store driver (open, chunk persistence,
  a `Services` wrapper that pushes a publish on the writer lane as a
  `Sliced` job over the copy store, holding the lane's connection only per
  step) follows #193: see 1c.
- **1c. Store on wasm32.** `iptv-store-sqlite` builds with the
  `sqlite-wasm-rs` backend on an in-memory VFS that persists changed pages
  to IndexedDB; staging databases are in-memory files; publishes run in
  slices on a copy (see Decisions). Probe the VFS and sliced publishing
  first. *Gate*: the store's tests pass in a Wasm test run, and on the TV a
  guide publish never holds the app's thread for more than 50 ms at a time.
  *Status*: the store's part is #193. The driver is `Ports::store_host`
  (`iptv_runtime::store_host::StoreHost`: open, persist after each writer
  job, the publish copy) and `publish.rs`, a `Sliced` job on the writer
  lane (prepare on the live store, 200-row steps on the copy, the swap and
  the writes that follow). The web host implements it on IndexedDB chunks
  (`host/web/store.rs`) with the five SQLite hooks. Differs from the plan:
  imports and guide downloads run in the browser as cooperative tasks, one
  at a time (`Services::submit_import`), rather than staying unavailable;
  their blocking store requests run the writer lane inline, which is safe
  because only one import runs and it awaits its own publish. The 50 ms
  gate is not measured on the TV yet.
- **1d. Native-only services behind `native`.** Recording, backup, credential
  store, staged probes, with the capability model reporting them unavailable.
  *Gate*: `cargo check -p iptv-runtime --no-default-features --target
  wasm32-unknown-unknown`. *Status*: the gate passes with step 1c merged.
  Recording capture, the recording host and the playback relay are behind
  `native` (a stub keeps the API, reports recording unavailable, and
  plays through the identity transport). Staged probes are refused where
  tasks cannot block, and `foundation_capabilities` reports them
  `UnsupportedHost`. Randomness is a port (`Ports::entropy`). Backup files,
  private documents and the encrypted open still use the file system
  without `native`: they compile, and wait for the runtime to open the
  browser store (`Store::open_browser`, chunk persistence, the sliced
  publish driver over 1c's API and the `rust_sqlite_wasm_host_*` hooks the
  final module supplies), which is the next change.
- **1e. Shell split.** `host/<platform>` modules; images through the network
  port. *Gate*: `cargo check -p iptv-app --target wasm32-unknown-unknown`.
  *Status*: the gate passes with step 1c merged. `src/host/native` and
  `src/host/web` (a stub over `iptv-web`'s adapters); screens ask
  `crate::host`. Five files keep a platform `cfg` (listed in the shell's
  architecture test, which only lets the list shrink).
- **2. Tizen platform.** The spike becomes the extension's Tizen platform:
  native-component registry (for `tv.player`), text input, fonts, images,
  the remaining mount items. `hosts/tizen` gains the AVPlay player and host
  services for the NSH build. *Status*: done as a sibling host,
  `hosts/tizen-nsh` (the bridge stays as it is), on nsh-platform-tizen at
  b80462a8. The player is the platform's AVPlay component behind a filter
  that refuses sources needing headers. The app starts on the TV, draws its
  screens and takes the remote's keys (the browser host uses the remote's
  key phases: the platform has no NSH keymap).
- **3. Measure and shrink.** Build Patina's real module. Link only the base
  components in use; record size, launch and key latency on the device.
- **4. Switch per TV generation.** *Gates*: module small enough to cache
  (about 6 MB on the 2020 set), first launch within 30 s behind a splash,
  cached launch within 8 s, key to frame within 80 ms. Generations that meet
  them move to the NSH build; the others stay on the bridge.

## Decisions after the second round of spikes

Measured on the same TV on 2026-11-21.

### Settled

1. **The app runs in a Worker.** The whole Rust app (engine, screens,
   runtime, store) runs in one Wasm instance in a Web Worker; the page draws
   operations, plays video and forwards keys. Text is measured with an
   `OffscreenCanvas` in the Worker (supported on this TV). Same guide screen,
   video playing, 32 paced moves each:

   | | Key to frame idle (p50 / p90) | While the app thread works 30 ms of every 50 | Page frames over 50 ms (idle / working) |
   | --- | --- | --- | --- |
   | Page thread | 70 / 112 ms | 108 / 146 ms | 18 / 32 |
   | Worker | 81 / 109–184 ms | 93–94 / 130–155 ms | 5–12 / 3–10 |

   The Worker costs about 11 ms per key at idle (message and copy) and wins
   under load, where the page stops dropping frames. This is what step 1b's
   execution port is designed around: one thread, cooperative jobs.

2. **The TV keeps the store's data in an in-memory SQLite database, restored
   from IndexedDB at launch.** Patina's real store for one account (7,257
   channels, 248,820 programs, the real pipeline, measured natively) is
   57.2 MB, of which 34.3 MB is program descriptions. Without descriptions it
   is about 23 MB. A 21 MB database of the same shape on the TV:

   | Step | Time |
   | --- | --- |
   | Read from IndexedDB, copy in, open (launch) | 0.6–1.9 s; 47 MB of memory |
   | 100 one-channel, four-hour guide queries | 25 ms |
   | Write 257,000 rows in one transaction | 10.8 s |
   | Serialize and write the whole database to IndexedDB | 0.9 s + 3.5 s; 66 MB peak |

   Consequences:
   - **Descriptions are off on browser TVs** (the native setting
     `store_descriptions`, defaulted off there). With them the database
     would not fit beside the app in memory or comfortably in the quota.
   - **Whole-database snapshots are too expensive per change** (about 4.4 s).
     Persistence is a memory VFS (built on `rsqlite-vfs`, the layer under
     `sqlite-wasm-rs`) that records which 64 KB chunks of the database file
     each commit changed; the host writes only those chunks to IndexedDB
     after the commit. Committed work reaches storage a moment after the
     commit: a crash can lose the last seconds, which is acceptable on a TV
     and is stated. Measured with a probe VFS on the TV (22.7 MB database):

     | Step | Time |
     | --- | --- |
     | A favorite toggle: chunks changed, written to IndexedDB | 1 chunk (64 KB), 34 ms |
     | Writing every chunk (first save, or after a publish) | 347 chunks, 3.5–4.7 s (0.6 s of it copying out of Wasm) |
     | Launch: read all chunks, copy in, open | 1.7–2.9 s; 28 MB of Wasm memory |
     | 100 guide queries after restoring | 60–77 ms |

3. **One connection, but publishing on a copy.** Today a second, read-only
   connection lets UI reads see the last commit while the writer holds a
   long publish transaction (`store_service.rs`; 58 read and 102 writer call
   sites). In Wasm there is one thread, so a publish cannot run beside UI
   reads, and on the TV a publish takes seconds. Correctness is not at risk:
   serial jobs see every earlier write. Latency is: every read would wait,
   and a multi-second publish would freeze the app. The design that keeps
   today's semantics: publishes (catalog replacement, guide replacement)
   run in slices on a second in-memory database copied from the first, while
   reads continue on the first; the copy replaces it when the publish
   commits. That costs a second copy of the database in memory during a
   publish (about 23 MB). Staging databases (`CatalogStage`, `EpgStage`)
   become in-memory files of the VFS instead of files on disk. Measured with
   the probe VFS on the TV, replacing 250,000 programs in 1,000-row slices
   with five guide queries on the original between slices:

   | | Result |
   | --- | --- |
   | Whole publish | 13.8 s in 250 slices, then 128 ms to commit and swap |
   | Longest slice | 117 ms, so slices of about 400 rows meet the 50 ms gate |
   | Queries on the original during the publish | median 2 ms, longest 6.2 ms |
   | Memory peak (both copies plus the save) | 110 MB of Wasm memory |

   **Native hosts keep today's algorithm** (WAL, a writer thread and a
   read-only reader connection, `store_service.rs`). Copy-on-publish is the
   browser executor's implementation of the same contract: UI reads see the
   last commit during a publish, and other writes wait for it in order (held
   until the swap, so none is lost). The shared change is in
   `iptv-store-sqlite`: catalog and guide replacement gain a resumable form
   (run until a step budget, then continue) that the browser executor
   drives in slices; the native writer keeps calling the one-shot form.

   After a swap every chunk counts as changed, so a guide refresh rewrites
   the whole database (3.5–4.7 s, in the background). Comparing chunks with
   their stored versions before writing would skip unchanged ones; not
   measured.

### Still open

1. **Encryption at rest on the TV.** No keystore, so an app-held key only
   obscures. Recommendation: no encryption on browser TVs, stated in
   `docs/security.md`, which already describes the bridge's credentials in
   clear.
2. **Which TV generations get the NSH build.** The 2020 set compiles about
   3 s per MB and cannot cache a 12.5 MB module; SQLite adds 1.6 MB (6.5 s to
   compile here). Patina's module is estimated at 12–19 MB before trimming.
   Needs a 2022 or later set.
3. **Stalker portals on browser TVs.** Their API calls set `User-Agent` and
   `Cookie`, which a page cannot. Unsupported unless a portal accepts
   requests without them (untested).
4. **Memory during a guide refresh.** Two copies of the database plus the
   save peaked at 110 MB of Wasm memory in the probe, beside the app. Within
   the 300–380 MB per-app limit Samsung documents for some models, but it
   has to be measured with Patina's whole app running.

## Module size and launch (steps 3 and 4)

Patina's real module (`hosts/tizen-nsh`), measured on the UN55TU7000 on
2026-11-21. Every number comes from the `tizen` profile: release, fat LTO,
one codegen unit, panic=abort, stripped, `-C target-cpu=mvp -C
target-feature=+sign-ext,+mutable-globals`, std rebuilt on
nightly-2026-10-24 (`-Zbuild-std=std,panic_abort`), SQLite with
`-mcpu=mvp -msign-ext -mmutable-globals`. Each build was installed fresh
(uninstalled first), then launched normally twice with `tizen run`; the
timings are the platform's own record (`localStorage['nsh.metrics']`), read
afterwards with DevTools. Code and Data are the module's sections
(`hosts/tizen-nsh/sections.py`); compile time follows Code.

| Build | Total | Code | Data | First launch: compile | Cache write | Second launch: load | App start | First paint |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| opt-level 3 | 16.15 MB | 13.61 MB | 2.50 MB | 49.6 s (45–54 s over three launches) | fails (`QuotaExceededError`; an earlier run: transaction timeout) | compiles again: 54.2 s | 5.0–10.5 s | 57–76 s |
| opt-level z | 11.54 MB | 8.91 MB | 2.55 MB | 32.1 s | succeeds, 12.7 s | from the cache: 13.4 s | 4.9–7.1 s | 47 s first, 22 s cached |

Findings:

- The cache limit is not a fixed 6 MB: the 11.5 MB module cached (the
  quota was about 68 MB with no other data; the earlier failures were the
  quota and a transaction timeout during the clone). Reading the cached
  module back is slow on this set (13.4 s for 11.5 MB), so a cached launch
  is still far from the 8 s gate.
- "App start" is the engine's first pump, including opening the store
  (empty here) and building the first screen: about 5 s on this set.
- Where the bytes are (opt-level z, unstripped, `twiggy`, by the crate a
  function's name starts with; generic code counts where it is
  instantiated, so `core`, `alloc` and `hashbrown` are mostly Patina's and
  NSH's types): `nsh_core` 2.5 MB, `core` 1.0 MB, `alloc` 0.9 MB, SQLite
  (C) 0.73 MB, `iptv_runtime` 0.72 MB, `iptv_app` 0.65 MB,
  `iptv_store_sqlite` 0.38 MB, `hashbrown` 0.36 MB. Data is 2.5 MB, of
  which the translations (`messages.tsv`, 0.66 MB), the banner (0.40 MB)
  and the three Roboto fonts (0.27 MB) are files.

## Not goals

- Recording, backup files, picture in picture, or multiview on TVs with one
  decoder.
- Porting NSH's own web runtime to Chromium 69, or patching NSH.
- A storage abstraction beside SQLite, or a second implementation of
  application logic.
- New features in the JavaScript bridge.

## Evidence

| Claim | Source |
| --- | --- |
| Bridge performance, limits and what it stores | `hosts/tizen/README.md`, PR #190 |
| NSH engine on the TV, compile time and cache limit by module size, per-key cost | `../nsh-platform-tizen/README.md` |
| Worker against page thread | `../nsh-platform-tizen` (`worker.html`, `bench-common.js`), runs on 2026-11-21 |
| Real store size | Native import of one real account into a temporary unencrypted store (`dbstat` per table); the database was deleted after measuring |
| SQLite persistence on the TV | In-memory database built, serialized, written to IndexedDB, read back and restored across launches (probe, 2026-11-21) |
| Chunk persistence and sliced publishing | Probe VFS on `rsqlite-vfs` 0.2 tracking 64 KB chunks, run on the TV across launches (2026-11-21) |
| SQLite on the TV | Probe built 2026-11-21 with rusqlite 0.42 (`bundled`, `ffi-sqlite-wasm-rs`), rebuilt std, `CFLAGS_wasm32_unknown_unknown="-mcpu=mvp -msign-ext -mmutable-globals"`, and the five `rust_sqlite_wasm_host_*` hooks supplied by the app |
| Runtime and shell dependencies | `cargo metadata`, `cargo tree --target wasm32-unknown-unknown`, and a `cargo check -p iptv-runtime --target wasm32-unknown-unknown` that stops at `getrandom` |
| Estimated module size | Per-crate code size of the native release binary; an estimate until step 3 builds it |
