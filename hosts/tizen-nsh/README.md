# Patina's NSH interface on Samsung Tizen TVs (experimental)

The same Patina as on the desktops and Android (`crates/iptv-app` and the
whole runtime, store and import pipeline) built for Samsung TVs with the
external NSH platform [nsh-platform-tizen](https://github.com/teng-lin/nsh-platform-tizen),
pinned by revision in `Cargo.toml`.

**Opt-in on Tizen 5.5.** The JavaScript bridge (`hosts/tizen`) is the
supported app on 2020 sets. See [Migration gates](#migration-gates).

## How it runs

| Part | Where | What it does |
| --- | --- | --- |
| Module | `src/lib.rs` (`patina-tizen`) | `iptv-app`'s root and UI set in the platform's runner (`nsh_platform_tizen::app!`). The engine, screens, `iptv-presentation` holders, runtime and SQLite store run in one WebAssembly instance in a Web Worker. |
| Worker services | `app/patina-worker.js` | The imports iptv-web's adapters link: fetch and its streams, timers, clocks, randomness, the time zone, key-value storage, and the store's 64 KiB chunks in IndexedDB (read before the module starts, saved after each commit). |
| Page | `app/index.html`, `app/patina-host.js` | Starts the platform, which draws the app's views on the page; `tv.player` is the platform's AVPlay component, refusing sources that need request headers other than `User-Agent` (AVPlay cannot send them, and sends its own `User-Agent`). |
| Package | `app/config.xml`, `build.sh` | Its own application id (`PatinaNsh1.Patina` by default), CSP mode, the `tizen` profile. |
| Package files | `build.sh`, `catalog.py` | iptv-app's data, kept out of the module (its `browser-package` feature): `i18n/<tag>.tsv`, one language's messages each, read by the Worker when the language is first shown; `fonts/`, the page's fonts (`patina-host.js`); `assets/banner.png`, the splash. |

Back at the top level (the guide's rail, the first run's welcome screen,
or library recovery with its details closed) asks "Close Patina?" (No focused,
then Yes), as
Samsung's termination guide asks of a TV app: `src/lib.rs` calls
`iptv_app::confirm_leaving`, and Yes sends `{"leave": true}` to the page
(`nsh_platform_tizen::host`, the page's `onHost`), which exits. The
platform never registers the remote's Exit key.
Up or Left selects No; Down or Right selects Yes, without wrapping.
OK chooses and Back dismisses. The
[exit policy](../../docs/interaction.md#samsung-tizen-exit-policy) records the
open certification questions and the
[pending TV checks](../../docs/interaction.md#pending-device-checks).

Decisions stay in Rust: the Worker script performs the requests and
writes the chunks it is handed; the destination policy, the store and
everything else are the app's (`docs/security.md`, "Browser build of the
app").

When the page is hidden, AVPlay closes the stream and reports
`pictureLost` for its source token. On return it opens the same source
with its existing pause setting. Patina keeps the backdrop opaque until
`firstRender` reports a picture for the active token. Ordinary buffering
leaves the backdrop clear after the first frame.

## Build and install

Requirements as for `hosts/tizen`: the pinned nightly with `rust-src`
(`tools/ci/fuzz-imports.sh --toolchain`), a `clang` that targets wasm32
(SQLite's C code), Tizen Studio's CLI, a signing profile for the TV, and the
TV in Developer Mode. `build.sh` first checks that `../nsh-patina` is clean
at `nsh.rev` (`tools/ci/pins.py check-nsh`), as `tools/check.sh` does.

```sh
hosts/tizen-nsh/build.sh --stage                                  # target/tizen-nsh/stage
TIZEN_SIGNING_PROFILE='<profile>' hosts/tizen-nsh/build.sh --install <tv-ip>:26101
NSH_OPT=3 NSH_SIZE=0 hosts/tizen-nsh/build.sh --stage             # opt-level 3, no size flags
```

Every build uses the `tizen` profile (release, opt-level z: fat LTO, one
codegen unit, panic=abort, stripped), `-C target-cpu=mvp -C
target-feature=+sign-ext,+mutable-globals`, `-Zbuild-std=std,panic_abort`
and the same feature set for SQLite (`CFLAGS_wasm32_unknown_unknown`). By
default it also builds for size: std with `optimize_for_size`,
`-Cpanic=immediate-abort` (a panic is a trap without its message),
`-Zlocation-detail=none`, SQLite's C code in the link-time optimization
when `clang` has rustc's LLVM major version (`-flto`,
`-Clinker-plugin-lto`) and without the SQLite options Patina never uses
(`SQLITE_WASM_RS_FLAGS`), then binaryen's `wasm-opt -Oz` limited to MVP
plus the two features, when `wasm-opt` is on `PATH` (`NSH_SIZE=0` and
`NSH_WASM_OPT=0` turn them off for comparisons). `build.sh` prints the module's total, Code
and Data bytes (`sections.py`); compile time on the TV follows the Code
section. `NSH_UNSTRIPPED=1` keeps the names section for `twiggy` (never for
a package). A package that installs over the same version is refused by the
TV. Set `NSH_PACKAGE_VERSION` to a higher version to update in place
and keep the installed app's data.

When an Add includes a guide URL, the browser swaps in the playlist and
its required guide together. Cancel during saving discards both before
that swap. The wizard waits for the result so it can show a completed add
if Cancel arrived after the swap.

## Migration gates

A TV generation moves from the bridge to this package when it meets all of:

| Measure | Gate |
| --- | --- |
| Module | Small enough to cache compiled (about 6 MB was the estimate for a 2020 set) |
| First launch after an install | Within 30 s, behind a splash |
| Cached launch | Within 8 s |
| Key to frame on the guide | Within 80 ms |
| A publish step on the app's thread | At most 50 ms |

The 2020 set measured (UN55TU7000, Tizen 5.5) misses the launch,
key-to-frame and publish-step gates, and the TV closed the app twice while
zapping (cause not established). The decision of 2026-11-21 keeps the bridge
as the supported app there, with no further size or performance work for
Tizen 5.5. A newer set (Tizen 6.5+, Chromium 85+) has to be measured before
deciding about Tizen as a whole; memory during a guide refresh is also still
to be measured. The
[original measurements](https://github.com/teng-lin/patina-iptv/blob/5db8843cd3b4f82a4b7821e7ef2b52a74c73c3d4/docs/plans/2026-11-21-browser-tv-architecture.md#decision-on-tizen-55-2026-11-21)
are in the history.

## What does not work yet

- Recording, backup files and picture in picture (no workers, one
  decoder).
- Stalker portals (a page cannot send their `Cookie` and `User-Agent`), and
  sources whose streams require custom request headers. A source with
  only a configured `User-Agent` is accepted and AVPlay sends its own.
- Tracks, volume and the player's format events (the AVPlay component
  does not report them).
