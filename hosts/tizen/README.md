# Patina on Samsung Tizen TVs (experimental)

Live TV on Samsung TVs from 2020 (Tizen 5.5) on: an Xtream Codes account or
an M3U playlist, its TV guide as a list and as a full guide grid, the
channel list beside the playing channel, full-screen video and channel
zapping, search of channels and programs, Favorites (in your own order) and
Recently watched. Movies, series and recordings are not part of this host
yet.

The TV app talks to your provider directly; no server is needed.

## How it works

NSH's web runtime needs a newer browser engine than these TVs have
(Tizen 5.5 is Chromium 69), so this host is plain JavaScript around
Patina's own core in WebAssembly:

| Part | Where | What it does |
| --- | --- | --- |
| Core | `crates/iptv-web` | Imports the source and its guide with `iptv-import`, builds the schedule with `iptv-guide`, and owns the list, guide focus and reorder state (`iptv-core`) and the search indexes and screen (`iptv-search`). Emits views as JSON: only what is on screen. |
| Worker | `app/worker.js` | Runs the core in a Web Worker and performs the HTTP requests it describes, so importing a large guide never blocks the remote. |
| Page | `app/main.js`, `index.html`, `style.css` | Renders the views, sends remote keys, plays streams with the TV's native player (AVPlay) on the video plane under the page. |

The core is built for the Wasm features Tizen 5.5 validates: MVP plus
sign-extension and mutable globals. Stable Rust's standard library uses newer
features (reference types, bulk memory) that this engine rejects, so
`build.sh` rebuilds it with a pinned nightly toolchain (`-Zbuild-std`).

## Build and install

Requirements: Tizen Studio with the TV extension and Samsung Certificate
Extension, a Samsung certificate profile that lists your TV's DUID, the TV in
Developer Mode with this computer's address, and the pinned nightly toolchain
with `rust-src`:

```sh
rustup toolchain install nightly-2026-10-24 --component rust-src
TIZEN_SIGNING_PROFILE='<profile>' hosts/tizen/build.sh --install <tv-ip>:26101
```

Without `TIZEN_SIGNING_PROFILE` the script stages an unsigned package in
`target/tizen/stage`. The signed package is `target/tizen/Patina.wgt`.

## Using it

- First launch asks for the playlist. Up/Down move between fields, OK opens
  a field's keyboard (Done returns), Save loads it. The red button opens it
  again later.
- In the list: Up/Down move, OK plays in the preview, OK on the playing
  channel goes full screen, Left opens the groups (moving through them
  previews their channels), Back returns to the channels. Back in the
  channels goes full screen when a channel plays, and asks to exit when
  none does.
- Full screen: Up/Down or Channel Up/Down zap within the list, OK or Left
  opens the list, Back asks to exit (or returns to the guide grid or the
  search that played the channel).
- Guide grid: Right in the channels, or the remote's Guide key, opens it for
  the list shown. Up/Down move between channels, Left/Right between
  programs (the time scrolls with them), Channel Up/Down turn a page of
  channels. OK plays the channel; on an ended program its archive still
  has, it plays that program from its start. Back first returns to now,
  then to the list.
- Search: the Search row above the groups (Left, then Up past Favorites),
  or the remote's Search key. Type with the TV's keyboard; Done shows the
  results, Cancel or Back closes the keyboard. Channels are found by name,
  programs by title (on now or later). Down reaches the Channels/Programs
  tabs, Left/Right switch them, OK plays the channel (of the program).
  Before typing, recent searches are listed. Back on the field leaves.
- Favorites: Yellow adds or removes the focused (or playing) channel; Blue
  shows the Favorites list (the first group). Blue on a favorite starts
  moving it: Up/Down move it one place, Left/Right to the top or the end,
  OK, Back or Blue end the move and keep the order.
- Green updates the TV guide now.

## What is kept on the TV

Per playlist, in the app's IndexedDB: Favorites in their order, Recently
watched (both by channel key, so they survive playlist refreshes) and
recent searches, in the same record as before search and reordering (an
existing install keeps its favorites), and the guide as an
`iptv_epg` snapshot with its refresh state. The guide is downloaded again
when the native hosts would: by `iptv_import::plan_startup` / `decide`
(every 24 hours, at start when stale, when the playlist's channels or guide
feeds change, early when it runs out, after a failure only after an hour).
Only channels in the playlist are kept, without descriptions, and programs
that ended more than a day ago are dropped. Replacing the playlist removes
the previous one's data.

## Measured on a 2020 UN55TU7000 (Tizen 5.5, Chromium 69)

With a provider account of about 7,300 live channels and a 68 MB XMLTV
guide:

- First launch: channels show about 3 s after launch; the guide is
  imported and matched (6,022 channels bound) about 18 s after launch, in
  the background.
- Later launches (stored guide, normal launch without a debugger): the
  channel list with its guide shows about 5 s after the page starts.
- Channel change, input to moving video: about 1.6 s (raw TS on AVPlay).
- Remote input to rendered list while the guide imports: median 31 ms,
  worst 79 ms. When idle, a page jump that loads a screen of new logos can
  take about 0.2 s.
- The guide grid, search and moving favorites are not yet measured on the
  TV. On a desktop browser with a synthetic account of the same size
  (7,300 channels, 242,000 programs): grid key to rendered page median
  3 ms; search answer to rendered results 12 to 19 ms; the program index
  takes 143 ms to build and 3.4 MB, and the core's memory grows from
  25.4 MB to 28.4 MB with it.

The page records its timings in local storage (`patina.metrics`, the last
five launches): `keys`, `guideKeys` and `searchKeys` (remote key to
rendered page, per screen), `search` (query to rendered results),
`searchIndex` (its build) and, on request (`send({cmd: 'memory'})` in
DevTools), `memory`.

## Known limits

- The account is kept in the app's local storage on the TV, not in the
  encrypted store the native hosts use (see [security](../../docs/security.md#samsung-tizen-host)).
- Content-supplied addresses (guide feeds, streams, logos) must be public,
  and redirects are checked again. The browser resolves host names itself,
  so a public name that resolves to a LAN address is not detected, and the
  player and image loader follow their own redirects unchecked.
- CI builds `iptv-web` for wasm32 with stable Rust; the TV build
  (`build.sh`, rebuilt std for MVP WebAssembly) is not built in CI.
- The TV sends its own User-Agent: providers that require a specific one on
  API requests do not work. Stalker portals are not offered.
- Program descriptions are not kept (they are most of a guide's size), so
  the grid shows a program's title and time only.
- The playlist is downloaded at every launch (about 1–3 s); only the guide
  is stored.
