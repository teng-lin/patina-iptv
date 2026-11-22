# Patina on Samsung Tizen TVs (experimental)

Live TV on Samsung TVs from 2020 (Tizen 5.5) on: an Xtream Codes account or
an M3U playlist, its TV guide as a guide grid beside the groups with the
playing channel's preview above it (as on Android TV), full-screen video
and channel zapping, search of channels and programs, Favorites (in your own order) and
Recently watched. Movies, series and recordings are not part of this host
yet.

The TV app talks to your provider directly; no server is needed.

## How it works

NSH's web runtime needs a newer browser engine than these TVs have
(Tizen 5.5 is Chromium 69), so this host is plain JavaScript around
Patina's own core in WebAssembly:

| Part | Where | What it does |
| --- | --- | --- |
| Core | `crates/iptv-web` | Imports the source and its guide with `iptv-import`, builds the schedule with `iptv-guide`, and owns the lists, the guide screen's focus and columns, and the reorder state (`iptv-core`) and the search indexes and screen (`iptv-search`). Emits views as JSON: only what is on screen. |
| Worker | `app/worker.js` | Runs the core in a Web Worker and performs the HTTP requests it describes, so importing a large guide never blocks the remote. |
| Page | `app/main.js`, `index.html`, `style.css` | Renders the views, sends remote keys, plays streams with the TV's native player (AVPlay) on the video plane under the page. |

The page and the core exchange JSON messages. The page numbers every `key`
and `settle` command (`seq`) and sends the next one when the view carrying
that number arrives (or after a second without it), so a held key never
queues work in the core; views also arrive for other reasons (the clock,
search text, the guide) and release nothing. A held key is sent with
`repeat: true`, and the core decides what holding it does: a Left pressed
in the guide grid opens the groups column at its first repeat, one pressed
in full screen does nothing more, anywhere else it is a Left like any
other. When the held key is released (or the page loses the focus), its
repeats still waiting are dropped and, if the core put off building a list
(`pending`), `settle` asks for it; repeats that go on after the page lost
the focus or was hidden are not sent until the key is pressed again. A
pressed key drops a play bar timeout (`barTimeout`) still waiting to be
sent. Each `play` event has an `id`, which the page returns in
`playFailed` with the player's reason: the core ignores the failure of a
stream it has replaced since, and for the one that plays it emits the
notice. The page numbers each `source` it sends (`session`), and every
`play` carries the number of its source: a play of a source replaced since
is dropped. A `play` that arrives while the app is hidden is opened when it
shows again.

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
  a field's keyboard (Done returns), Save loads it, Back closes it once a
  playlist is saved. Holding OK or Back acts once. The red button opens it
  again later.
- The remote's Exit key leaves the app from any screen. Back never does.
- The guide screen: the preview of the playing channel at the top left
  ("Nothing playing" until one plays), the focused program beside it
  (title, ★ for a favorite, times, progress, minutes left or until it
  starts, the list), and the guide grid below: 8 channels, two and a half
  hours, the playing channel marked ▶. Moving never tunes.
- Grid: Up/Down move between channels in the same time column,
  Left/Right between programs (Left from the earliest one shows the
  groups column), Channel Up/Down or Page Up/Down turn a page of
  8 channels. Holding Left on the guide screen opens the groups column at
  once (a Left held from full screen only opens the guide). Up from the
  first channel focuses the preview while a channel plays (OK: full
  screen, Down: back to the grid).
- OK on a program on now plays it full screen (on the channel already
  playing, it only goes full screen); on an ended program the archive
  still has, it plays that program from its start. A program that has
  not started, or one the archive no longer has, shows a notice.
- Groups column: Up/Down move through the lists and show each in the grid
  (while a held key moves through them, the list shows once it is
  released, and a held Up stops at the first list: press Up again there
  for the preview); Right or OK enters it (an empty Favorites cannot be entered);
  Left goes to the rail (Search and TV). Back: a past or future program
  returns to now, then the groups, then the rail; on the rail Back does
  nothing.
- Full screen: Up/Down or Channel Up/Down zap within the list shown (a
  channel played from search that the list does not have zaps within its
  own group), Left or Back return to the guide on the playing channel
  (Back returns to the search that played the channel). The Guide key
  opens the guide. When a stream fails, full screen returns to where it
  was played from (not when you have zapped on to another channel
  meanwhile).
- Play bar: OK in full screen shows the channel, its program with start
  and end times, the next program, and the buttons Favorite, TV guide
  and Search, with Favorite focused. Left/Right move between the
  buttons, OK presses one, Back hides it, and it hides after
  7 seconds without a key. Up/Down still zap under it. Holding OK, Back
  or a color key acts once.
- Search: the rail's Search (Left of the groups), Search on the play bar,
  or the remote's Search key. Type with the TV's keyboard; Done shows the
  results, Cancel or Back closes the keyboard. Channels are found by name,
  programs by title (on now or later). Down reaches the Channels/Programs
  tabs, Left/Right switch them, OK plays the channel (of the program).
  Before typing, recent searches are listed. Back on the field returns to
  the guide.
- Favorites: Yellow, or Favorite on the play bar, adds or removes the
  focused (or playing) channel, and a Favorites list on screen follows at
  once; Blue shows the Favorites list in the grid. Blue on a favorite
  there (with at least two) starts moving it: Up/Down move it one place,
  Left/Right to the top or the end, OK, Back or Blue end the move and keep
  the order.
- Green updates the TV guide now. Search results stay while it updates:
  programs the new guide no longer lists are removed, and the Programs
  tab and the focused result are kept where the new guide still has them.
- Notices show at the bottom of the guide screen, at the top elsewhere.
  When the core cannot start or stops, its message covers the screen and
  only Exit works.

## Differences from the Android TV app

- Yellow (not Red) adds or removes a favorite; Red opens the playlist
  setup.
- In full screen, Left opens the guide and Right does nothing (on Android
  TV, Left shows the channel list over the video and Right returns to the
  previous channel).
- The play bar opens with Favorite focused (a deliberate choice: OK again
  presses it), and has no pause, seek or track buttons.
- No hint card after a list is chosen, and no program descriptions.

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
  guide screen shows about 5 s after the page starts.
- Channel change, input to moving video: about 1.6 s (raw TS on AVPlay).
- Remote input to the rendered guide screen (stored guide, idle, with a
  debugger attached): median about 10 ms, worst about 40 ms, over
  28 keys in the grid, the groups column and the rail. OK to full screen,
  including opening the stream, about 0.2 s.
- Search and moving favorites are not yet measured on the TV. On a
  desktop browser with a synthetic account of the same size
  (7,300 channels, 242,000 programs): search answer to rendered results
  12 to 19 ms; the program index takes 143 ms to build and 3.4 MB, and
  the core's memory grows from 25.4 MB to 28.4 MB with it.

The page records its timings in local storage (`patina.metrics`, the last
five launches): `keys` (full screen), `guideKeys` (the guide screen) and
`searchKeys` (remote key to rendered page), `search` (query to rendered
results), `searchIndex` (its build) and, on request (`send({cmd: 'memory'})` in
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
