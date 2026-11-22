# One interaction layer for the NSH app and the Tizen bridge

Status: proposed on 2026-11-22. Nothing here is implemented. It follows the
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

## Target architecture

Today the middle layer exists twice:

```
domain crates → iptv-app (NSH glue + views)  → Android, desktop, iOS
              → iptv-web (its own glue)      → Tizen page
```

Proposed: one interaction layer and two thin presenters.

```
domain crates → interaction layer: keys → actions → state changes + effects; view-models
                  → iptv-app: signals + NSH views
                  → iptv-web: JSON + effect execution → page that only renders
```

Rules for the interaction layer:

- No NSH and no I/O; it builds for `wasm32-unknown-unknown`.
- Plain functions: an action and the current state go in, state changes and
  effects (play, stop, notice, save lists, open a screen) come out.
- The existing small models (`Focus`, `Panels`, `PlayBar`, `Reorder`) stay
  separate values. `iptv-app` keeps each in its own signal, so NSH's
  fine-grained updates are unchanged.
- A capabilities and policy input says what a host has (VOD, recordings,
  multiview, settings, pause and seek) and which defaults it overrides. The
  bridge runs a subset.
- View-models are plain data (titles, Unix times, progress, flags); text
  comes from `iptv-core`'s i18n, so both presenters show the same strings.
- Behavior is tested once, in this layer.

## Steps (one pull request each)

1. **The bridge adopts what already exists.** No change to `iptv-app`.
   - The page sends raw key down and up; `iptv-nav` decides tap or hold and
     maps keys to actions (this replaces the page's repeat and held-Left
     rules).
   - `iptv_core::panels` and `play_bar` replace the bridge's own bar.
   - `ChannelHistory` gives Right as the previous channel.
2. **Full-screen keys and the Back chain** move from `iptv-app` into the
   layer. `iptv-app` adopts them first with no behavior change, then the
   bridge.
3. **View-models:** the focused program's details, the info card and the
   play bar.
4. **Guide-screen glue:** OK on a program (watch, replay, details),
   favorites toggle and list rebuild, move mode, group preview.
5. **Shrink the bridge:** `app.rs` becomes import wiring, effect execution
   and serialization; the page protocol is documented; `main.js` only
   renders.

## Risks

- Steps 2 to 4 change code that ships on Android, desktop and iOS. Each must
  preserve behavior and pass those platforms' tests and scenario checks.
- A single state struct would cost NSH its fine-grained updates; hence
  functions over separate models.
- The bridge's module is about 1.5 MB; each step records its size.
- Tap or hold in Rust needs key-up events and timestamps from the TV's
  browser (Chromium 69 delivers both).

## Open decisions

1. **Where the layer lives:** a small new crate that depends on `iptv-core`
   and `iptv-nav` (recommended), or more modules inside `iptv-core`.
2. **Bridge exceptions:** exact Android TV behavior, or explicit policy
   overrides (the bridge's play bar opens with Favorite focused; Android's
   card opens with nothing focused). Recommended: policy overrides, listed
   in the bridge's README.
3. **Timing:** step 1 after #209 merges.

## Later

The same layer is the groundwork for an NSH web mode on browser TVs: the
app's logic in a small Worker module, with a renderer that turns elements
into DOM and CSS so that the browser does the layout.
