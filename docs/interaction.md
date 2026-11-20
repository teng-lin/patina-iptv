# Interaction contract

One set of rules for choosing, watching and returning, whether the viewer uses a
TV remote, touch, or a mouse and keyboard. Platform follow-ups (#87 TV, #88
tablet, #89 desktop) build on this contract rather than defining their own.
[TV controls](tv-controls.md) gives the remote's key table and
[accessibility](accessibility.md) the assistive-technology journeys.

Each rule is marked **Now** (implemented today) or **Planned** (agreed in #86,
not yet built). Update the mark in the change that delivers it.
[Verification](#verification) lists the tests that cover the **Now** rules.
Unmarked statements, such as the principles, are goals that the marked rules
implement; each section states where current behaviour still falls short.

## Principles

- **Choosing is watching.** Choosing a channel or a live programme plays it full
  screen, on every input and in both guide layouts, unless the viewer has chosen
  to keep the guide open.
- **Layout is appearance, not behaviour.** The guide layout, window size and
  orientation change how content is arranged, never what an action does.
- **Every way in has a way back the viewer can see or find on the input in
  use**: the Back key on a remote, a visible Back button for touch and pointer.
  It returns to the same content.
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

- **Now:** Keys and Touch (`Input` in `crates/iptv-app/src/lib.rs`). A mouse
  click, or mouse movement over full-screen video, counts as Touch. The main
  window starts in Touch on an iPad and in Keys on every other host; a separate
  desktop player window starts in Touch.
- **Now:** the first key after touch moves focus onto a row on screen if the
  focused channel has scrolled out of view.
- **Planned:** a separate Pointer mode. A tablet with a keyboard and mouse
  switches between modes as each is used; touching it again brings back the
  touch controls.

## Destinations

The navigation rail holds a stable set of destinations
(`iptv_core::browse::rail_items`): Search, Live TV, Movies, Shows, Continue
watching, Recently added, Recordings, Watchlist, Playlists and Settings.

- **Now:** Live TV is the start screen. With **Play the last channel on start**
  (`general.resume_last_channel`, on by default) the last channel starts full
  screen over it. Movies and Shows always appear.
  Continue watching and Recently added appear when any enabled playlist has
  movies or series. Watchlist is dimmed until #90 ships.
- **Planned:** the same destinations in every layout class. A compact window
  may present them differently (for example a collapsed rail), but never drops
  one or reorders it.

## Choosing and inspecting

In the guide:

| Target | Remote | Touch | Mouse and keyboard | Result |
|---|---|---|---|---|
| Channel, or programme on air | OK | Tap | Click or Enter | Plays full screen |
| Channel already playing live | OK | Tap | Click or Enter | Returns to full screen |
| Past programme within the channel's archive | OK | Tap | Click or Enter | Replays from the start |
| Past programme without replay, or upcoming programme | OK | Tap | Click or Enter | Opens its details |

- **Now:** all four rows (`Ui::watch`). Choosing a new live channel waits for
  `guide.switch_delay` before tuning, so that fast choices skip channels; a
  replay starts at once. On an iPad, a tap on the guide beside the open groups
  column only closes the column.
- **Now:** the setting **Keep the guide after choosing** (`guide.stay_on_guide`,
  off by default) keeps the guide open after tuning a live channel, in both
  layouts. A replay and the channel already playing always go full screen.
  `guide.autoplay` tunes the focused channel while browsing, without a choice.
- **Now:** opening a programme's details or menu never starts playback.

"Full screen" means the video fills Patina's screen or window. On desktop it
never resizes the operating-system window; that is a separate action (double-click
on the video, or the window's own control).

## Getting back

Back from full screen returns to where the viewer chose: the guide on the
chosen channel, Recordings for a recording, or the multiview grid for a tile.
In the guide the playing stream continues in the preview or behind the guide,
and focus on a live channel follows the clock to the programme now on air.

| Input | Way back from full screen |
|---|---|
| Remote | Back |
| Touch | Back gesture where the platform has one, and a visible Back button |
| Mouse and keyboard | Escape or Backspace, and a visible Back button |

- **Now:** Back, Escape and Backspace follow one order (`go_back` in
  `crates/iptv-app/src/lib.rs`). The main steps: close a chooser or dialog;
  on desktop, restore a window from the operating system's full screen; hide
  the player's panels; then leave full screen. In the overlay layout, Back from
  the guide while a channel plays returns to the video. On a remote, long Back
  in the player stops playback. The play bar shows a Back button except on TVs
  and terminals (`Ui::back_controls`).
- **Now:** with `guide.back_returns_to_now` (on by default), Back with focus on a
  past or upcoming programme in the guide first returns to the programmes on
  air, after any dialog or hint card closes.
- **Planned:** visible Back and Close follow the input mode rather than the
  device type, so a TV with a pointer, or a tablet used by touch after a
  keyboard, shows them.

## Secondary actions

Programme and channel menus (favourites, recording, details, replay) need a
visible entry point for touch and pointer users.

- **Now:** in the guide, the channel list and recordings: Menu on a remote, and
  long OK on Android; M on a keyboard, and Shift+F10 on desktop. Long press and
  right-click open the same menu on guide programmes and channels, the channel
  list and recordings. During playback, Menu and long OK open the playback bar
  instead ([TV controls](tv-controls.md)).
- **Now:** these menus have no visible entry point, so they are the current
  exception to "shortcuts are optional".
- **Planned:** a visible "more" control on the focused or tapped item for touch
  and pointer, so the menu needs no hidden gesture.

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

**Planned** as a whole, with the current exceptions below. Collapsing or
expanding panes, resizing, rotating, entering split screen and switching input
keep:

- the selected content (channel, programme, title, season, episode) by identity,
  never by row index;
- filters, sort and the scroll anchor;
- the playing stream on the same persistent player surface. The player view is
  resized and moved, never hidden, detached or re-parented;
- open details and their focused action.

They issue no playback command of their own.

- **Now:** the first key after touch can move focus to a visible row (see
  [Input modes](#input-modes)). With `guide.autoplay` on, focus that lands on a
  live programme after a pane closes can tune it.

## Empty and unavailable states

**Now.** Movies and Shows explain why they are empty, with one action that
fixes it. The state comes from the playlist on screen (every enabled playlist
when they are combined): its type, its own movie and series import setting,
a refresh running now, and its last import's result. When several playlists
apply, the first matching row below wins, after "some playlist has titles".

| State | Message says | Action |
|---|---|---|
| Import in progress | Importing movies and shows… | None; the list fills when it finishes |
| Import failed | Last refresh failed, and why by error kind (no provider text) | Retry: runs that playlist's refresh again |
| Imports turned off for the source | Movie and series imports are off for this playlist | Playlist settings: opens that playlist's actions at its "Movies and series" switch |
| Imported, but the catalogue is empty | No movies (or shows) yet; the last refresh found none | Refresh now |
| No source can provide movies or series (M3U only, or none) | Movies and shows come from Xtream Codes and Stalker playlists; M3U has live channels only | Add playlist |

On a remote the action sits where the titles would be: Right from the
categories, then OK. A search or filter that finds nothing says "No titles
found." with no action. Continue watching and Recently added keep their own
empty text. Imports are never switched on automatically, and M3U channels
never appear as movies or series.

## Admission and capability

Every play action, from any input or layout, goes through the same checks.
Unavailable actions say why, using the host's reported capabilities
([capabilities](capabilities.md)), not the platform's name.

- **Now:** streamed playback goes through the parental lock and provider
  connection admission. A recording plays from its local file: it has its own
  parental check and needs no provider connection.

## Verification

- The repository's required checks in [AGENTS.md](../AGENTS.md), including
  `cargo test -p iptv-core` for core state and transition rules, the
  `iptv-core` wasm build and `cargo build -p iptv-app`.
- Choosing, staying and returning: app scenarios `personalization_overlay`,
  `tv_shared_controls`, `pip_control`, `cov_nav` and `cov_nav_touch`.
- Native behaviour on Android TV and tablets: the
  [disposable Android validation](android-validation.md) on `tv36` and `tab36`.
  Record emulator and physical-device evidence separately, with the build, OS,
  input and viewport, using synthetic data only.
