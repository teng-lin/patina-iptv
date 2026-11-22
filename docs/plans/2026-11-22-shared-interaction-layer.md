# Shared interaction decisions for the NSH app and the Tizen bridge

Status: proposed on 2026-11-22 and implemented on top of merged #209.
The JavaScript bridge (`crates/iptv-web`, `hosts/tizen`) remains the
supported app on Tizen 5.5. The opt-in NSH package in
[#205](https://github.com/teng-lin/patina-iptv/pull/205) runs `iptv-app`
through the browser adapters. It uses the same input and screen code as
other NSH hosts. See the [browser TV plan](2026-11-21-browser-tv-architecture.md)
for the migration gates.

## Problem

Patina's interaction logic exists twice.

- **Shared, no NSH (today):** `iptv-core` has the guide and browse focus rules
  (`guide`, `browse`), the info card and play bar models (`panels`,
  `play_bar`), zap targets and channel history (`playback`), tune delays
  (`browse_playback`), favorites, reordering and lists. `iptv-nav` has the
  keymap and the tap or hold detector, with no dependencies.
- **In `iptv-app`, bound to NSH signals:** the glue that routes a key to those
  models: the full-screen key table (`player_keys.rs`), the Back chain
  (`lib.rs`), OK on a program and favorites (`lists.rs`,
  `browse_playback.rs`), and the focused program's details (`header.rs`).
- **In the bridge, written again:** the same glue in `iptv-web/src/app.rs`
  (about 1,400 lines), and hold and repeat rules in `hosts/tizen/app/main.js`.

Since #209, `iptv-web/src/grid.rs` drives `iptv_core::guide::Focus`
over a `lists::ListView` and uses `iptv_core::browse::Columns` and
`Outcome` for the rail, groups and guide. The NSH app uses those same
models in `lists.rs`. Sharing the focus transitions still leaves different
rail items, paging, program selection effects and Back behavior to account
for. The bridge also uses the core's group zap target and `Focus::open_groups`.

## Findings that shape the plan

- **Guide navigation already shares the focus model.** The migration
  covers the surrounding key handling, the decisions made after a focus
  outcome and the focused program's presentation.
- **The bridge's interaction glue is small.** A few hundred lines across
  `iptv-web`'s `app.rs` (1,509 lines in all), `grid.rs` and `search.rs`,
  plus key handling in `main.js`. The rest is import, guide refresh, search
  wiring and serialization, which only the bridge needs.
- **`iptv-app`'s routing is large and tied to screens the bridge lacks.**
  `input.rs`, `player_keys.rs`, `lists.rs`, `browse_playback.rs`,
  `favorites.rs`, `header.rs` and the Back section of `lib.rs` are about
  3,200 lines (rendering and tests included). `overlays.rs` declares 22
  overlays, and each action goes to the top open one before the guide or
  the player. Settings and NSH state are read inline.
- **That routing is already thin.** It asks a core model and acts on the
  answer (`Panels::select` → `Pick`, `Focus::navigate` → `Outcome`). The
  decisions are what must be shared; the dispatch is not.
- **The shared models do not fit the bridge as they are** (see step 2).
- **The bridge's support window may narrow.** Once NSH meets the gates
  on a newer TV generation, that generation can move to the NSH package.

## Target architecture

Unchanged in shape: domain crates, then two presenters (`iptv-app` with NSH
views, `iptv-web` with JSON for a page). What changes is the rule:

- Every **decision** both presenters make (what a key means, where focus
  goes, what OK on a program does, which channel a zap reaches, what a card
  shows) is a model or a pure function in `iptv-core` or `iptv-nav`, tested
  there once.
- Each presenter keeps its own thin **routing** and its host-only parts.
  For the bridge's page that is AVPlay, setup, the TV keyboard and the app
  lifecycle.
- View-models are plain data (titles, Unix times, progress, flags) with
  explicit time and locale inputs.
- Hosts run a subset by passing what they have (as `PlayBar::move_focus`
  takes `enabled`), and state their deliberate exceptions.

## Where each decision lives

The two shared crates are independent today: neither depends on the other,
and that stays. Each keeps its own question.

| Question | Crate | Speaks in | Examples |
| --- | --- | --- | --- |
| What did the viewer ask for? | `iptv-nav` | keys, press phases, times, a context → `Action` | tap or hold, repeats, the bridge's key profile, key choices such as "Up/Down zap or open the list" (`player_keys::up_down_key`, `seek_key`, in the app today) |
| What does that do to the screen's state? | `iptv-core` | `Move`s and model methods → outcomes | OK on a program, the zap target, focus restore after Favorites change, paging, the card and bar models, view-models |
| The search screen's state | `iptv-search` | its own state machine | OK on a result, what Back returns to |
| Which model gets the action, and carrying out the outcome | each presenter | its own state and effects | overlay order, tuning, saving, rendering |

Rules that follow:

- `iptv-core` never takes a key or an `Action`; `iptv-nav` never reads
  guide or playback state. A decision that needs both is two decisions:
  `iptv-nav` picks the action (given a context and host choices passed in
  as plain values), `iptv-core` applies it to state.
- The context (`iptv_nav::Context`) is the one value that crosses: the
  presenter derives it from its state (`input::context` in the app) and
  hands it to the keymap.
- No new crate. `iptv-web` gains a dependency on `iptv-nav`; it depends on
  `iptv-core` and `iptv-search` already.

## Migration

0. **Bridge contract.** Share decisions for supported behavior and keep
   the remote and panel differences listed in the
   [bridge README](../../hosts/tizen/README.md#differences-from-the-android-tv-app).
   Preserve the existing NSH behavior across platforms.
1. **Shared scenarios first.** The same synthetic key traces, with a
   controlled clock, run against the headless app
   (`iptv-app/tests/app/tv_remote_journey.rs`) and the bridge's fake host
   (`iptv-web/src/tests.rs`). They assert the visible surface, the focused
   content, playback effects and the listed exceptions. The lost or late
   key-up trace runs in both presenters. Their existing journeys and
   bridge regression tests also cover an overlay opening during a hold,
   held Back, rapid zaps with a stale failure, and Search → player → Back.
2. **The bridge adopts the shared models**, in separate pull requests:
   - **Transport.** The page sends key down and up with its own event
     timestamps (a key can wait behind an unanswered one, so the worker's
     arrival time is wrong), and a timer for a hold on a remote that does
     not repeat. Holds are canceled on blur, hide, setup or keyboard
     capture and a source change. Repeat coalescing and the groups list's
     settling stay. To prove on the TV before the rest.
   - **Keys.** `iptv-nav`'s `HeldPresses`, `PressDetector` and `Keymap`
     with a bridge profile: only the short and long pairs the bridge
     supports (a long binding defers the tap to release), its color keys,
     and Back and Exit. The bridge's string keys become `iptv_nav::Action`,
     so a changed action fails to compile.
   - **Panel.** The bridge uses `PlayBar` with Favorite, Guide and Search,
     Favorite initially focused and a 7 s idle timeout. The NSH app keeps
     its existing controls and 3 s play bar timeout.
3. **Shared decisions, extracted one at a time.** Each becomes a pure
   function that both presenters call, in the crate the table above gives
   (state decisions in `iptv-core`, search ones in `iptv-search`, key
   choices in `iptv-nav`):
   - OK on a program: live, already live, replay, or details, with whether
     details exist as an input (the bridge shows a notice). Tune timing
     stays apart: the app uses `BrowsePlayback` (switch delay, autoplay,
     stay on the guide), the bridge tunes at once.
   - OK on a search result and where Back returns from the player (the
     bridge tunes live even for an upcoming program result).
   - Zap: both use `zap_target` already. What differs: the app skips
     blocked rows, advances from a pending zap and can switch the guide to
     the target's group.
   - Previous channel: the app's `ChannelHistory` records live starts
     only; the bridge's Recent records archive plays too.
   - Favorites and focus restore: the app rebuilds Favorites and Recent
     and the player's list; the bridge rebuilds Favorites only. Which
     channel, pane and time survive a removal, a reorder and a refresh.
   - Page Up and Down: the bridge fits the time to the destination row;
     the app moves the row only.
   - The Back chain's overlapping part (hint card, back to now, the last
     step, which differs by host).
   - Not a candidate as first written: a failed stream. The app stays in
     full screen behind a retry card; the bridge leaves full screen. Share
     it only after defining rejected, retryable and terminal failures.
4. **View models.** Program timing comes from `iptv-core`. NSH retains
   guide-minute precision and its localized captions. The bridge uses its
   English 24-hour labels and wall-clock progress with the TV's UTC offset
   at each instant.
5. **Page rendering.** The bridge receives guide labels, progress, ruler
   marks, search result times and player program labels in its view. The
   page handles drawing, AVPlay, setup, keyboard capture and lifecycle.
   See the [bridge protocol](../../hosts/tizen/README.md#input-and-view-protocol).

## Risks

- Steps 3 and 4 change code that ships on Android, desktop and iOS. Each
  must preserve behavior and pass those platforms' tests.
- Adopting the shared key and panel models changes key timing and focus on
  the TV. Each part of step 2 is checked there, with key-to-frame latency.
- Thin routing can still diverge; the shared scenarios (step 1) and the
  README's list of exceptions are the guard.
- The bridge's module size is recorded per step, measured on the Tizen
  build (the staged module is 2.27 MB today), not on the stable wasm CI
  build.

## What was considered and dropped

The first version of this plan proposed one interaction layer in a new
crate: plain functions from an action and the state to state changes and
effects, with `iptv-app`'s full-screen key table, Back chain and guide glue
moved into it. Dropped because:

- it would restructure about 3,200 lines of the shipping app's input code to
  remove a few hundred lines from the bridge;
- a pure version of `input.rs` must model every overlay and setting it
  consults (22 overlays), most of which the bridge lacks;
- step 1 and the extracted decisions remove the causes of the divergences
  found so far at a fraction of that risk.

It can be reconsidered if a second non-NSH presenter appears, or if an NSH
web mode (the app's logic in a small Worker module, with a renderer that
turns elements into DOM and CSS) becomes the plan for browser TVs.

## Host choices

- The bridge follows the NSH app for the decisions it shares. The package
  in #205 uses the NSH presenter directly and has its own migration gates.
- The bridge keeps its own color keys, full-screen Left and Right behavior
  and 7 s play bar, as documented in its README.
- The transport and bridge performance still need a run on the supported
  Samsung TV, including the new key-to-frame measurement.
