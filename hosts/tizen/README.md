# Patina on Samsung Tizen TVs (experimental)

Live TV on Samsung TVs from 2020 (Tizen 5.5) on: an Xtream Codes account or
an M3U playlist, its TV guide, the channel list beside the playing channel,
full-screen video and channel zapping, Favorites and Recently watched.
Movies, series, recordings and the full guide grid are not part of this
host yet.

The TV app talks to your provider directly; no server is needed.

## How it works

NSH's web runtime needs a newer browser engine than these TVs have
(Tizen 5.5 is Chromium 69), so this host is plain JavaScript around
Patina's own core in WebAssembly:

| Part | Where | What it does |
| --- | --- | --- |
| Core | `crates/iptv-web` | Imports the source and its guide with `iptv-import`, builds the schedule with `iptv-guide`, and owns the list and focus state (`iptv-core`). Emits views as JSON. |
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
  previews their channels), Back returns to the channels.
- Full screen: Up/Down or Channel Up/Down zap within the list, OK or Left
  opens the list, Back asks to exit.
- Yellow adds or removes the focused (or playing) channel from Favorites;
  green updates the TV guide now.

## What is kept on the TV

Per playlist, in the app's IndexedDB: Favorites and Recently watched (by
channel key, so they survive playlist refreshes), and the guide as an
`iptv_epg` snapshot with its refresh state. The guide is downloaded again
when the native hosts would: by `iptv_import::plan_startup` / `decide`
(every 24 hours, at start when stale, when the playlist's channels or guide
feeds change, early when it runs out, after a failure only after an hour).
Only channels in the playlist are kept, without descriptions, and programs
that ended more than a day ago are dropped. Replacing the playlist removes
the previous one's data.

## Measured on a 2020 UN55TU7000 (Tizen 5.5, Chromium 69)

With a real Xtream account of 7,257 channels and a 68 MB XMLTV guide:

- First launch: channels show about 3 s after launch; the guide is
  imported and matched (6,022 channels bound) about 18 s after launch, in
  the background.
- Later launches (stored guide, normal launch without a debugger): the
  channel list with its guide shows about 5 s after the page starts.
- Channel change, input to moving video: about 1.6 s (raw TS on AVPlay).
- Remote input to rendered list while the guide imports: median 31 ms,
  worst 79 ms. When idle, a page jump that loads a screen of new logos can
  take about 0.2 s.

## Known limits

- The account is kept in the app's local storage on the TV, not in the
  encrypted store the native hosts use.
- Content-supplied addresses must be public. The browser resolves host names
  itself, so literal LAN addresses and local names are refused, but a public
  name that resolves to a LAN address is not detected.
- The TV sends its own User-Agent: providers that require a specific one on
  API requests do not work. Stalker portals are not offered.
- Favorites can be added and removed but not reordered.
- The playlist is downloaded at every launch (about 1–3 s); only the guide
  is stored.
