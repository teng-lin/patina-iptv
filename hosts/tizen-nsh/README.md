# Patina's NSH interface on Samsung Tizen TVs (experimental)

The same Patina as on the desktops and Android (`crates/iptv-app` and the
whole runtime, store and import pipeline) built for Samsung TVs with the
external NSH platform [nsh-platform-tizen](https://github.com/teng-lin/nsh-platform-tizen),
pinned by revision in `Cargo.toml`. It sits beside the JavaScript bridge
(`hosts/tizen`), which stays the default package for 2020 sets until this
build meets the gates in `docs/plans/2026-11-21-browser-tv-architecture.md`
(see "Module size and launch" there for the numbers).

## How it runs

| Part | Where | What it does |
| --- | --- | --- |
| Module | `src/lib.rs` (`patina-tizen`) | `iptv-app`'s root and UI set in the platform's runner (`nsh_platform_tizen::app!`). The engine, screens, runtime and SQLite store run in one WebAssembly instance in a Web Worker. |
| Worker services | `app/patina-worker.js` | The imports iptv-web's adapters link: fetch and its streams, timers, clocks, randomness, the time zone, key-value storage, and the store's 64 KiB chunks in IndexedDB (read before the module starts, saved after each commit). |
| Page | `app/index.html`, `app/patina-host.js` | Starts the platform, which draws the app's views on the page; `tv.player` is the platform's AVPlay component, refusing sources that need request headers (AVPlay cannot send them). |
| Package | `app/config.xml`, `build.sh` | Its own application id (`PatinaNsh1.Patina` by default), CSP mode, the `tizen` profile. |
| Package files | `build.sh`, `catalog.py` | iptv-app's data, kept out of the module (its `browser-package` feature): `i18n/<tag>.tsv`, one language's messages each, read by the Worker when the language is first shown; `fonts/`, the page's fonts (`patina-host.js`); `assets/banner.png`, the splash. |

Decisions stay in Rust: the Worker script performs the requests and
writes the chunks it is handed; the destination policy, the store and
everything else are the app's (`docs/security.md`, "Browser build of the
app").

## Build and install

Requirements as for `hosts/tizen`: the pinned nightly with `rust-src`
(`tools/ci/fuzz-imports.sh --toolchain`), a `clang` that targets wasm32
(SQLite's C code), Tizen Studio's CLI, a signing profile for the TV, and the
TV in Developer Mode.

```sh
hosts/tizen-nsh/build.sh --stage                                  # target/tizen-nsh/stage
TIZEN_SIGNING_PROFILE='<profile>' hosts/tizen-nsh/build.sh --install <tv-ip>:26101
NSH_OPT=3 NSH_SIZE=0 hosts/tizen-nsh/build.sh --stage             # the ladder's first row
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
TV: uninstall this app id first.

## What does not work yet

- Recording, backup files and picture in picture (no workers, one
  decoder). Settings and favorites edits last for the session.
- Stalker portals (a page cannot send their `Cookie` and `User-Agent`), and
  sources whose streams need request headers.
- Tracks, volume and the player's format events (the AVPlay component
  does not report them).
