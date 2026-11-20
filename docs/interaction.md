# Interaction contract

One set of rules for choosing, watching and returning, whether the viewer uses a
TV remote, touch, or a mouse and keyboard. Platform follow-ups (#87 TV, #88
tablet, #89 desktop) build on this contract rather than defining their own.
[TV controls](tv-controls.md) gives the remote's key table and
[accessibility](accessibility.md) the assistive-technology journeys.

Each rule is marked **Now** (implemented and covered by tests) or **Planned**
(agreed in #86, not yet built). Update the mark in the change that delivers it.

## Principles

- **Choosing is watching.** Choosing a channel or a live programme plays it full
  screen, on every input and in both guide layouts.
- **Layout is appearance, not behaviour.** The guide layout, window size and
  orientation change how content is arranged, never what an action does.
- **Every way in has a visible way back**, and it returns to the same content.
- **Presentation never plays.** Resizing, rotating, collapsing panes or switching
  input never opens, retunes or stops a stream.
- **Shortcuts are optional.** Long OK, long press, right-click and keyboard
  shortcuts speed things up; no essential action needs one.

## Input modes

The app tracks the last input used, not the device type.

| Mode | Trigger | Shows |
|---|---|---|
| Keys | A remote key or keyboard key | Focus highlight and key hints |
| Touch | A touch | Visible controls; no focus highlight or key hints |
| Pointer | Mouse or trackpad movement or click | Visible controls and hover; no key hints |

- **Now:** Keys and Touch. A mouse counts as Touch (`Input` in
  `crates/iptv-app/src/lib.rs`). An iPad starts in Touch, every other host in Keys.
- **Planned:** a separate Pointer mode. A tablet with a keyboard and mouse
  switches between modes as each is used, and keeps its touch controls.

## Destinations

The navigation rail holds a stable set of destinations
(`iptv_core::browse::rail_items`): Search, Live TV, Movies, Shows, Continue
watching, Recently added, Recordings, Watchlist, Playlists and Settings.

- **Now:** Live TV is the start screen. Movies and Shows always appear.
  Continue watching and Recently added appear when any enabled playlist has
  movies or series. Watchlist is dimmed until #90 ships.
- **Planned:** the same destinations in every layout class. A compact window
  may present them differently (for example a collapsed rail), but never drops
  one or reorders it.

## Choosing and inspecting

| Target | Remote | Touch | Mouse and keyboard | Result |
|---|---|---|---|---|
| Channel, or programme on air | OK | Tap | Click or Enter | Plays full screen |
| Channel already playing | OK | Tap | Click or Enter | Returns to full screen |
| Past programme within the channel's archive | OK | Tap | Click or Enter | Replays from the start |
| Past programme without replay, or upcoming programme | OK | Tap | Click or Enter | Opens its details |

- **Now:** all four rows (`Ui::watch`). Choosing waits for `guide.switch_delay`
  before tuning so that fast choices skip channels.
- **Now:** the setting **Keep the guide after choosing** (`guide.stay_on_guide`,
  off by default) keeps the guide open instead of going full screen, in both
  layouts. `guide.autoplay` tunes the focused channel while browsing, without a
  choice.
- **Now:** selecting a programme to inspect it never starts playback.

"Full screen" means the video fills Patina's screen or window. On desktop it
never resizes the operating-system window; that is a separate action (double-click
on the video, or the window's own control).

## Getting back

Back returns to the guide on the channel and programme the viewer chose, with
the playing stream continuing in the preview or behind the guide.

| Input | Way back from full screen |
|---|---|
| Remote | Back |
| Touch | Back gesture where the platform has one, and a visible Back button |
| Mouse and keyboard | Escape or Backspace, and a visible Back button |

- **Now:** Back, Escape and Backspace follow one order: close a chooser or
  dialog, then hide the player's panels, then leave full screen for the guide.
  The play bar shows a Back button except on TVs (`Ui::back_controls`).
- **Now:** with `guide.back_returns_to_now` (on by default), Back in a guide
  scrolled away from now first returns to the programmes on air.
- **Planned:** visible Back and Close follow the input mode rather than the
  device type, so a TV with a pointer, or a tablet used by touch after a
  keyboard, shows them.

## Secondary actions

Programme and channel menus (favourites, recording, details, replay) need a
visible entry point for touch and pointer users.

- **Now:** Menu or long OK on a remote; Shift+F10 on a keyboard.
- **Planned:** a visible "more" control on the focused or tapped item for touch
  and pointer, with long press and right-click as shortcuts to the same menu.

## Layout classes

**Planned.** One core policy picks the arrangement from the available logical
window space, the text size and the host's capabilities. Breakpoints come from
what the content needs at the current text size, not from the device type.

| Class | Arrangement |
|---|---|
| Expanded | Guide beside the player (preview layout), or VOD list beside details |
| Medium | Panes side by side where both stay usable; otherwise the compact arrangement |
| Compact | One pane at a time; the other is one action away |

- Below the space a pane needs, the layout collapses rather than shrinking text
  or touch targets further.
- Remote focus treatment does not change with window size.
- **Now:** the guide scales from 1.0 to 0.8 with the desktop window
  (`crates/iptv-app/src/scale.rs`), and some screens collapse on their own
  measure, such as Search's two-row header.

## What a presentation change keeps

Collapsing or expanding panes, resizing, rotating, entering split screen and
switching input keep:

- the selected content (channel, programme, title, season, episode) by identity,
  never by row index;
- filters, sort and the scroll anchor;
- the playing stream on the same persistent player surface. The player view is
  resized and moved, never hidden, detached or re-parented;
- open details and their focused action.

They issue no playback command of their own.

## Empty and unavailable states

**Planned.** Movies and Shows explain why they are empty, with one action that
fixes it:

| State | Message says | Action |
|---|---|---|
| No source can provide movies or series (M3U only) | This playlist type has no movies or series | Add a playlist |
| Imports turned off for the source | Movie and series imports are off | Open the playlist's import settings |
| Import in progress | Titles are being imported | None; the list fills when it finishes |
| Import failed | Why it failed, by error kind | Retry |
| Imported, but the catalogue is empty | The provider sent no titles | Refresh |

Imports are never switched on automatically, and M3U channels never appear as
movies or series.

## Admission and capability

Every play action, from any input or layout, goes through the same parental
lock, provider connection admission and capability checks. Unavailable actions
say why, using the host's reported capabilities ([capabilities](capabilities.md)),
not the platform's name.

## Verification

- Core state and transition rules: `cargo test -p iptv-core`.
- Choosing, staying and returning: app scenarios `personalization_overlay`,
  `tv_shared_controls`, `pip_control`, `cov_nav` and `cov_nav_touch`.
- Native behaviour on Android TV and tablets: the
  [disposable Android validation](android-validation.md) on `tv36` and `tab36`.
  Record emulator and physical-device evidence separately, with the build, OS,
  input and viewport, using synthetic data only.
