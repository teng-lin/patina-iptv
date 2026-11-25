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
| Core | `crates/iptv-web` | Imports the source and its guide with `iptv-import`, builds the schedule with `iptv-guide`, and uses the search indexes in `iptv-search`. Guide focus, lists, reordering and the search screen live in `iptv-presentation`. Emits views as JSON: only what is on screen. |
| Worker | `app/worker.js` | Runs the core in a Web Worker and performs the HTTP requests it describes, so importing a large guide never blocks the remote. |
| Page | `app/main.js`, `index.html`, `style.css` | Renders the views, sends remote keys, plays streams with the TV's native player (AVPlay) on the video plane under the page. |

The page and the core exchange JSON messages. See the
[input and view protocol](#input-and-view-protocol) for remote events.

The core is built for the Wasm features Tizen 5.5 validates: MVP plus
sign-extension and mutable globals. Stable Rust's standard library uses newer
features (reference types, bulk memory) that this engine rejects, so
`build.sh` rebuilds it with a pinned nightly toolchain (`-Zbuild-std`).

## Build and install

Requirements: Tizen Studio with the TV extension and Samsung Certificate
Extension, a Samsung certificate profile that lists your TV's DUID, the TV in
Developer Mode with this computer's address, Python 3 (it writes the
interface languages' package files), and the pinned nightly toolchain with
`rust-src`:

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
- Back on the guide's rail, or on the first launch's playlist setup,
  asks "Close Patina?" with No focused: Left moves to No and Right to
  Yes, Up and Down to the other answer, OK chooses, Back closes the
  question. Only Yes leaves. The focused answer has the page's focus, so
  Voice Guide reads it. The remote's Exit key is the system's; the page does not
  register it. Samsung's CO-US-04 checklist distinguishes closing a popup/IME
  from closing an application page; verify the TV's system transition rather
  than assuming Exit always terminates over a popup. The question is in the
  TV's language. See the [acceptance ledger](../../docs/platform-back-validation.md)
  for the unresolved Return policy and pending physical-TV checks.
- The guide screen: the preview of the playing channel at the top left
  ("Nothing playing" until one plays), the focused program beside it
  (title, ★ for a favorite, times, progress, minutes left or until it
  starts, the list), and the guide grid below: 8 channels, two and a half
  hours, the playing channel marked ▶. Moving never tunes.
- Grid: Up/Down move between channels in the same time column and wrap
  between the first and last rows. Left/Right move between programs
  (Left from the earliest one shows the groups column). Channel Up/Down
  or Page Up/Down turn a page of 8 channels. Holding Left in the grid for
  400 ms opens the groups column. From full screen, Left opens the guide.
- OK on a live program tunes the channel in the preview. OK on the
  playing channel expands the same stream to full screen. An ended
  program with an archive plays from its start. An upcoming or
  unavailable program shows a notice.
- Groups column: Up/Down move through the lists and wrap at the ends.
  Each selection shows its list in the grid. When a key repeats, the
  list shows once the key is released. Right or OK enters a nonempty
  list, and Left goes to the rail. Back returns to now from a past or
  future program, then goes to groups, then the rail.
- Rail: Search and TV. When video plays, Up from Search focuses the
  preview. OK expands it to full screen, and Down returns to Search.
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
  only the remote's Exit key works.

## Differences from the Android TV app

- Yellow (not Red) adds or removes a favorite; Red opens the playlist
  setup.
- In full screen, Left opens the guide and Right does nothing (on Android
  TV, Left shows the channel list over the video and Right returns to the
  previous channel).
- The play bar opens with Favorite focused (a deliberate choice: OK again
  presses it), and has no pause, seek or track buttons.
- No hint card after a list is chosen, and no program descriptions.

- A tap of Left moves to the previous program when the guide has one.
  Held Back acts once. The remote's Exit key is the system's.
- Programs chosen in Search tune their channel live, including upcoming
  programs. Back returns to the result list. The Recently watched list
  includes channels played from their archive.
- Page Up and Down fit the time to the destination row's listings. Back
  on the rail asks before leaving (Samsung's exit confirmation; Android
  TV leaves at once).

## Input and view protocol

The page sends `key` commands with a stable `iptv-nav` key name, a
`phase` (`down`, `repeat` or `up`), and `atMs` from
`performance.now()` when the event was received. A view's
`keyDeadlineMs` uses that same clock. When the deadline arrives the page
sends `keyPoll` with the current `atMs`. That lets a remote without
auto-repeat hold Left and lets the core own the play bar timeout.

The input queue keeps one command in flight and coalesces queued repeats.
Each key phase, poll, keyboard or cancel command carries a `seq`. The page
sends the next command when its view acknowledges that sequence or after one
second. On keyup the page drops queued repeats for that key, and the core
settles the groups list. The page sends `cancelKeys` on blur, hiding, setup
and keyboard capture. The core clears the held press and settles the list.
Installing a new source also resets input state. The search keyboard sends
`keyboard` with `complete: true` for Done or `false` for Back or Cancel.

Views carry guide and player labels, progress, search result times and the
ruler marks. The worker supplies the UTC offset for each displayed
instant. The page uses the labels and positions directly when drawing.

Each `play` event carries its source's `session` and its own `id`. The page
ignores events for replaced sources and defers playback while hidden. On
failure, the page sends `playFailed` with the playback `id` and the reason.
The core shows a notice when that playback is still current.

At start the page sends `locale` with `navigator.language`; the core
resolves it to an interface language (English until then) and reads that
language's messages from the package (`i18n/<tag>.tsv`, written by
`build.sh` with `hosts/tizen-nsh/catalog.py`) when it first needs one.
Back on the rail makes the core send a `leave` event with the question
and its two answers in that language; the page sends `leave` itself for
Back on the first launch's setup. The page shows the question, and calls
`tizen.application.getCurrentApplication().exit()` only on Yes.

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
  28 keys in the grid, the groups column and the rail. On the build used
  for this measurement, a single OK opened the stream and switched to full
  screen in about 0.2 s.
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
