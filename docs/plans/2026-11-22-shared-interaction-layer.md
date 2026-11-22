# Shared interaction decisions for the NSH app and the Tizen bridge

Status: proposed on 2026-11-22, revised the same day after checking the
plan against the code (see "What was considered and dropped"). Nothing here
is implemented. It follows the
[decision on Tizen 5.5](2026-11-21-browser-tv-architecture.md): the JavaScript
bridge (`crates/iptv-web`, `hosts/tizen`) is the supported app on 2020 Samsung
TVs, and it should behave like the Android TV app without re-creating that
app's logic.

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

The bridge's guide screen matches Android TV because it calls
`iptv_core::guide::Focus` and `iptv_core::browse`. Where it wrote its own
glue it diverged: a review of #209 found its zap, its held keys and its play
bar differing from the Android TV app.

## Findings that shape the plan

- **The divergences came from not using shared code, not from missing it.**
  `iptv_core::playback::zap_target` exists; `iptv-nav` already decides tap or
  hold and drops or keeps repeats; `iptv_core::play_bar::PlayBar` already
  takes the enabled controls from its host.
- **The bridge's interaction glue is small.** About 400 to 450 lines of
  `iptv-web/src/app.rs` route keys; the other 1,000 or so are import, guide
  refresh, search wiring and serialization, which only the bridge needs.
- **`iptv-app`'s routing is large and tied to screens the bridge lacks.**
  `input.rs`, `player_keys.rs`, `lists.rs`, `browse_playback.rs`,
  `favorites.rs`, `header.rs` and the Back chain in `lib.rs` are about 4,400
  lines. `input.rs` passes each action through about 15 overlays (VOD,
  recordings, settings, the wizard, parental, multiview, ...) before the
  guide or the player, and reads settings and NSH state inline.
- **That routing is already thin.** It asks a core model and acts on the
  answer (`Panels::select` → `Pick`, `Focus::navigate` → `Outcome`). The
  decisions are what must be shared; the dispatch is not.
- **The bridge may be a stopgap.** If NSH runs well on 2022 and later TVs,
  the bridge serves only older sets.

## Target architecture

Unchanged in shape: domain crates, then two presenters (`iptv-app` with NSH
views, `iptv-web` with JSON for a page that only renders). What changes is
the rule for the presenters:

- Every **decision** (what a key means, where focus goes, what OK on a
  program does, which channel a zap reaches, what a card shows) is a model
  or a pure function in `iptv-core` or `iptv-nav`, tested there once.
- Each presenter keeps its own thin **routing**: it maps its screens to
  those models and carries out the outcomes. Routing holds no rule that the
  other presenter also needs.
- View-models are plain data (titles, Unix times, progress, flags); text
  comes from `iptv-core`'s i18n, so both presenters show the same strings.
- Hosts run a subset by passing what they have (as `PlayBar::move_focus`
  takes `enabled`), not by forking a model.

No new crate: the functions fit in `iptv-core`.

## Steps (one pull request each)

1. **The bridge adopts what already exists.** `iptv-app` is not touched,
   apart from the one core change below.
   - The page sends raw key down and up; `iptv-nav` (`PressDetector`,
     `Keymap`) decides tap or hold and maps keys to actions. The page's
     repeat, once-per-press and held-Left rules go.
   - `iptv_core::panels` and `play_bar` replace the bridge's own bar.
     Core change: the info card's action list (`QUICK_ACTIONS`) is a fixed
     six; it takes the host's enabled subset, as the play bar does.
   - `zap_target` and `ChannelHistory` replace the bridge's zap and give
     Right as the previous channel.
2. **Shared decisions, extracted one at a time.** Each is a pure function
   (or a method on an existing model) that `iptv-app` then calls in place of
   its inline logic, and the bridge calls in place of its copy:
   - OK on a program: watch, only go full screen, replay from the archive,
     or show details (`iptv-app/src/lib.rs`, the bridge's `Grid::watch`);
   - the zap target when the playing channel is not in the list shown
     (`player_keys.rs`, the bridge's `zap`);
   - the favorites toggle with the shown list's rebuild and the focus
     restore (`favorites.rs` and `lists.rs`, the bridge's
     `toggle_favorite`);
   - what a failed stream returns to.
3. **View-models:** the focused program's details (`header.rs`, the page's
   JS), the info card and the play bar, as plain data.
4. **The page only renders:** all text, times and progress come from the
   core; the page protocol is documented in the bridge's README.

## Risks

- Step 2 and step 3 change code that ships on Android, desktop and iOS, a
  few dozen lines each. Each must preserve behavior and pass those
  platforms' tests.
- Thin routing can still diverge. Mitigation: a rule found in both
  presenters is moved to the core when it is found, and reviews check for
  it.
- Tap or hold in Rust needs key-up events and timestamps from the TV's
  browser (Chromium 69 delivers both); to confirm on the TV in step 1.
- The bridge's module is about 1.5 MB; each step records its size.

## What was considered and dropped

The first version of this plan proposed one interaction layer in a new
crate: plain functions from an action and the state to state changes and
effects, with `iptv-app`'s full-screen key table, Back chain and guide glue
moved into it. Dropped because:

- it would restructure about 4,400 lines of the shipping app's input code to
  remove about 450 lines from the bridge;
- a pure version of `input.rs` must model every overlay and setting it
  consults, of which the bridge uses about a fifth;
- step 1 and the extracted decisions remove the causes of the divergences
  found so far at a fraction of that risk.

It can be reconsidered if a second non-NSH presenter appears, or if an NSH
web mode (the app's logic in a small Worker module, with a renderer that
turns elements into DOM and CSS) becomes the plan for browser TVs.

## Open decisions

1. **Bridge exceptions:** exact Android TV behavior, or explicit overrides
   (the bridge's play bar opens with Favorite focused; Android's card opens
   with nothing focused). Recommended: overrides as host inputs, listed in
   the bridge's README.
2. **Timing:** step 1 after #209 merges.
