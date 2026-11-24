# Interaction contract

One set of rules for choosing, watching and returning, whether the viewer uses a
TV remote, touch, or a mouse and keyboard. Platform follow-ups (#87 TV, #88
tablet, #89 desktop) build on this contract rather than defining their own.
[TV controls](tv-controls.md) gives the remote's key table,
[desktop viewing](desktop.md) the wheel, keyboard shortcuts and windows, and
[accessibility](accessibility.md) the assistive-technology journeys.

Each rule is marked **Now** (implemented today) or **Planned** (agreed in #86,
not yet built). Update the mark in the change that delivers it.
[Verification](#verification) lists the tests that cover the **Now** rules.
Unmarked statements, such as the principles, are goals that the marked rules
implement; each section states where current behavior still falls short.

## Principles

- **Choosing is watching.** Choosing a channel or a live program plays it full
  screen, on every input and in both guide layouts, unless the viewer has chosen
  to keep the guide open.
- **Layout is appearance, not behavior.** The guide layout, window size and
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

- **Now:** all three modes (`iptv_presentation::input_mode::InputMode`). Each press records
  whether a finger or a mouse, trackpad or pen made it; on desktop, mouse
  movement anywhere (and a sideways wheel in the guide) counts as Pointer,
  without moving focus. A remote's Back counts as a key. The
  main window starts in Touch on phones and tablets (Android and iOS) and in
  Keys on TVs, desktops and terminals; a separate desktop player window
  starts in Pointer. A tablet with a keyboard
  and mouse switches between modes as each is used.
- **Now:** the first key after touch or a pointer moves focus onto a row on
  screen if the focused channel has scrolled out of view.
- **Now:** a remote key held while touch opens or closes a PIN prompt does
  nothing more until it is released or pressed again: no tap, hold or step
  back acts on the screen the prompt covered or exposed
  ([TV controls](tv-controls.md#taps-and-holds)).

## Destinations

The navigation rail holds a stable set of destinations
(`iptv_presentation::browse::rail_items`): Search, Live TV, Movies, Shows, Continue
watching, Recently added, Recordings, Playlists and Settings.

- **Now:** Live TV is the start screen. With **Play the last channel on start**
  (`general.resume_last_channel`, on by default) the last channel starts full
  screen over it. When that channel is one of the playlist's favorites and
  the Favorites category is offered (`channels.show_favorites`, not yet
  editable in Settings), the guide behind it shows Favorites, so Back lands
  there with the channel focused; otherwise it shows All channels
  (`iptv_presentation::lists::resume_entry`). The list is chosen once the channel
  plays (after its PIN, if it has one); later favorite edits and the
  viewer's own choices are left alone. Movies and Shows appear before the
  first playlist and whenever an enabled playlist is Xtream Codes or Stalker
  (also with its movie and series import off); once every enabled playlist
  is M3U, which never carries them, they leave the rail and the Movies and
  Shows screen's own navigation. Recordings is left out on a host that
  cannot record (the browser TV host). Continue watching and Recently added appear when any enabled playlist has
  movies or series.
- **Planned:** the same destinations in every layout class. A compact window
  may present them differently (for example a collapsed rail), but never drops
  one or reorders it.

## Choosing and inspecting

In the guide:

| Target | Remote | Touch | Mouse and keyboard | Result |
|---|---|---|---|---|
| Channel, or program on air | OK | Tap | Click or Enter | Plays in the TV Guide preview |
| Channel already playing live | OK | Tap | Click or Enter | Returns to full screen |
| Past program within the channel's archive | OK | Tap | Click or Enter | Replays from the start |
| Past program without replay, or upcoming program | OK | Tap | Click or Enter | Opens its details |

- **Now:** all four rows (`Ui::watch`). Choosing a new live channel waits for
  `guide.switch_delay` before tuning, so that fast choices skip channels; a
  replay starts at once. On an iPad, a tap on the guide beside the open groups
  column only closes the column.
- **Now:** choosing a live channel in TV Guide tunes it in the preview.
  Choose the active channel again to expand the same stream. Up and Down
  wrap between the first and last channel rows and between the first and last
  displayed groups. In the overlay layout, **Keep the guide overlay after
  choosing** (`guide.stay_on_guide`, off by default) keeps the guide open
  after tuning. A replay goes full screen. `guide.autoplay` tunes the focused
  channel while browsing, without a choice.
- **Now:** opening a program's details or menu never starts playback. A
  long press or right-click on the program title in the header opens its
  program menu. Screen readers can invoke the title's menu action; on
  iPad, VoiceOver calls it **More options**. On
  channel names and timeline cells, those gestures open
  channel options. A
  long press or right-click moves the focus to what it opens the menu for;
  `guide.autoplay` then holds on that channel, after the menu or its details
  close, until the viewer moves the focus to another channel. A choice still
  plays.
- **Now:** a screen reader's activation (TalkBack's double tap, VoiceOver's
  and Narrator's activate) is a choice like a tap: a channel row or a
  program cell plays as the table says, and the guide's buttons act. The app
  hears it as a press with no position (`accessibility_visibility`).
- **Next (#192):** on Android, touch exploration only reaches the guide once
  NSH routes hover by `pointerEvents` as it routes touches. Until then an
  inert full-screen layer above the guide (the full-screen video's tap
  target, kept mounted while the guide shows) takes TalkBack's hover, so
  the accessibility focus stays on the window and a double tap chooses
  nothing. The NSH fix goes upstream first.
- **Now:** with a screen reader on, the play bar over full-screen video
  stays until the viewer hides it (a double tap on the video, Back, or a
  key off a TV) instead of hiding after its few seconds: moving from
  control to control by swipes takes longer than that (WCAG 2.2.1,
  `iptv_presentation::play_bar`). Multiview's pointer chrome, on the bar's timer,
  stays the same way.

### Touch gestures in the guide

- **Now:** a drag decides its direction once it has moved 10 points: mostly
  vertical scrolls the channels, mostly horizontal pans the timeline (a
  diagonal goes the way of its larger part, and stays that way; on an
  iPad the channel list's own scrolling waits for that decision, so a
  1.5:1 diagonal pans too: NSH's iOS pan gate, `nsh.rev`), and a
  drag released while moving flings on and slows down. A gesture the
  system cancels (a notification shade, a call) stops where it is,
  without a fling. A drag that started on a channel or a program, past
  or upcoming, never chooses it: under the threshold a touch is still a
  tap (`iptv_app::timeline`).
- **Now:** crossing the drag threshold, in the guide or the channel list,
  cancels a preview `guide.autoplay` was waiting to start; it does not
  come back until the focus moves to another channel. The threshold is
  read from the first finger's own touches, so a drag at the list's end
  that scrolls nothing counts too, and so does a scroll view taking over a
  finger that moved; a still finger's cancel does not
  (`iptv_app::timeline::Threshold`). A tap's
  own choice still plays after its switch delay. Scrolling never tunes,
  records or opens a menu or details (`BrowsePlayback::dragged`;
  `tests/app/tablet_gestures.rs`).
- **Now:** a timeline pan that starts on a channel's program on air
  keeps that channel for browse playback while it moves the focus along
  the row (`iptv_presentation::browse_playback::PanAnchor`, by the channel's
  identity): a choice waiting for its switch delay still plays, and
  panning back to now starts no preview again. Any other move of the
  focus (keys, a tap, another list, a zap) ends that hold.
- **Now:** a day of listings that a pan pages in keeps the channels where
  the finger scrolled them.

"Full screen" means the video fills Patina's screen or window. On desktop it
never resizes the operating-system window; that is a separate action (double-click
on the video, or the window's own control).

## Getting back

Back from full screen returns to where the viewer chose: the guide on the
chosen channel, Recordings for a recording, a movie's or a show's details,
or the multiview grid for a tile.
In the guide the playing stream continues in the preview or behind the guide,
and focus on a live channel follows the clock to the program now on air.

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
- **Now:** Back from a movie or an episode played full screen, by any
  input, returns to its details over the video on the same player, nothing
  opened again: a movie on Play or Resume, a series with the played episode
  and its season selected (`vod_view::return_from_player`; the play bar's ‹
  reads Movies or Shows). Back from the details returns to the list or
  shelf the title was chosen from, on that title, and from there to the
  guide, the title playing on in the preview (#147). The list opens
  scrolled to that title however deep it is (#176), and its card (a
  movie's, or the series' of the episode played) shows the progress just
  watched without the list being read again (#177).
- **Now:** an episode ending never changes the screen on its own
  (`vod_view::observe`). Full screen, the next episode plays full screen
  with `vod.autoplay_next_episode` on; with autoplay off, or when the next
  episode would need a PIN, the series' details open on it with "Ready for
  the next episode". On that series' details, the next episode plays in
  the preview behind them with autoplay on, selected with its season, and
  Play or OK takes it full screen; with autoplay off, or when it would
  need a PIN, playback stops and the details select it with the same
  notice. Anywhere else (a list or shelf, the
  guide, another title's details, settings), playback stops as after the
  viewer's Stop, and the series' card and Continue watching hold the next
  episode. A PIN for it is asked for only when the viewer plays it (#187).
- **Now:** with `guide.back_returns_to_now` (on by default), Back with focus on a
  past or upcoming program in the guide first returns to the programs on
  air, after any dialog or hint card closes.
- **Now:** visible Back and Close follow the input mode on hosts with a remote
  Back key: a TV used by touch or a pointer shows them, and a remote key hides
  them again. Tablets and desktops always show them, keyboard users included.
  List editing keeps its host's model (dragging with Save and Cancel, or OK and
  the arrows) whichever input drives it.

## Secondary actions

Program and channel menus (favorites, recording, details, replay) need a
visible entry point for touch and pointer users.

- **Now:** in the guide, the channel list and recordings: Menu on a remote, and
  long OK on Android; M on a keyboard, and Shift+F10 on desktop. Long press and
  right-click open channel actions on a channel header or player-list row,
  and group options on a group. On tablets and desktops, guide timeline cells
  open the same channel menu for the selected program's time block.
  Long OK on a player-list row opens its channel actions. Menu and M still
  give access to program and group actions from the player list. In the
  guide grid, the program menu also includes actions for its channel and
  group. Menu or M on the single-playlist All channels group adds a
  playlist. During full-screen TV playback, either OK press and Menu open
  the play bar ([TV controls](tv-controls.md)).
- Choose **Channel actions** from a program menu to use that program's
  time block. The channel menu opens on Favorites, followed by the recording
  action for the selected block. A live block can start or stop a recording.
  An upcoming block can schedule or cancel one. The player list uses the
  live block. On Linux and macOS, **Open in new window** plays the channel
  live in a separate Patina window.
- **Reorder channel** and **Reorder channels** open a draft order in the guide
  from channel, group and program menus. OK or Enter picks up or places a
  channel. Up and Down move it one row, and Left and Right move it to either
  end. Touch and mouse users can drag the handles and choose Save or Cancel.
  The playing channel keeps its player surface throughout the edit.
- **Reorder groups** in a group's menu uses the same controls in the guide's
  groups column. It starts on the group whose menu you opened.
- **Show favorites only** filters that group. **Guide time offset** corrects
  the times in a provider group in 15-minute steps. Existing recordings keep
  their accepted capture windows. **Use external player** applies to explicit
  channel choices on Android and Linux. Android offers installed video apps.
  Linux tries VLC, then mpv.
- **Now:** long press, right-click and the remote's Menu key open guide
  channel and group options. The guide has no overflow buttons in its header
  or groups column. The ruler shows the date and time and offers **Now** after
  browsing away; drag, wheel or keyboard shortcuts browse program time.
- **Now:** while touch or a pointer drives the app, a visible "More options"
  control (⋯) opens actions on channel-list rows and recordings rows, and
  group options on the shown group in the player's channel list. Keys hide
  these controls; they have Menu. Channel rows hide their ⋯ while multiview
  picks a channel (rows have no menu then).

## Layout classes

One core policy picks the arrangement from the available logical
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
- **Now:** the class policy (`iptv_app::layout::Class`): Compact below 620
  points and Expanded from 960, each divided by the text factor (the guide's
  scale times the text size: the app's own text size times the system's,
  Android's font scale or iOS Dynamic Type). Search's two-row header uses
  Compact.
- **Now:** the guide scales from 1.0 to 0.8 with the desktop window
  (`crates/iptv-app/src/scale.rs`). Tablets keep the full sizes in every
  window: an Android tablet's screen, and an iPad's full screen in either
  orientation, Split View, Slide Over and Stage Manager windows. A phone
  draws the guide at 0.7 (rows 50 → 35 dp, type 20 → 14), with the
  system's font scale still on top: the tablet's sizes were far too large
  on a 360-point screen. For now a phone runs in landscape (#168): its
  screen (smallest width under 600 dp, not a TV) asks for landscape on
  either side, as the rotation lock allows, until portrait has a design of
  its own. Tablets, an unfolded phone and TVs rotate freely.
- **Now:** the guide's header (`iptv_app::layout::GuideHeader`) puts its
  date and time ruler under the preview and the focused program in Compact, and
  in Medium wherever the program needs more room. Movies, Shows and Playlists
  are available in the navigation rail. The
  preview keeps its place at the top left and shrinks to leave that row room,
  but never below 64 points tall, which keeps the corner ‹ inside it. A short
  window grows the header instead, for the preview and for the focused
  program's title and times (wrapped as the column makes them), up to 55%
  of its height so the rail and the guide keep room, 70% at the accessibility
  text sizes; a window too short for that squeezes them, the description
  giving way first. The
  program's times wrap onto a second line rather than clip, and a part
  wider than the column (large text in a narrow window) wraps within it; its
  title stays on one line and is truncated. The date stays within the channel column. The
  guide's rows already take the full width below the header in every class.
- **Now:** a Compact window at least 1.6 times as tall as it is wide (a phone
  held upright, a narrow portrait Split View window) stacks the header: the
  preview spans the window's width inside its margins, the focused program
  sits under it, then the time ruler. Beside the program the preview
  would be a thumbnail (132 × 74 points at 412 points wide); stacked it is
  388 × 218. The stacked header keeps the same 55% (70%) limit, and a window
  where it would pass that keeps the other arrangement. A small desktop window
  (400 × 600 is 1.5) and a phone on its side keep the preview beside the
  program.
- **Now:** the player's channel list keeps its columns inside the window
  (`iptv_app::layout::ListPanel`): the groups column first, then the list in
  what is left. In the preview layout the video goes beside them only where
  it keeps 160 points of width. A narrower window, a phone held upright for
  one, leaves it full screen under the list, as the overlay layout does.
- **Now:** in Movies and Shows, details replace the list in every class and
  the poster grid reflows its columns. In Compact, an empty catalog (no
  titles, nothing filtered) is a pane of its own: its message and action take
  the width beside the rail (Close and the destinations), which stays narrow
  even with the remote on it, and the categories, which have nothing to
  filter, give way (`iptv_presentation::vod::Screen::one_pane`).
- **Planned:** collapsing the categories beside the posters in Compact; until
  then they share the width with the poster grid.
- **Now:** the play bar keeps its three groups on one row where the window
  has room for all their buttons. On a phone its buttons are 44 points and
  its side margins 12, so a phone on its side (740 points across and wider)
  keeps the one row, the quality and the channel's name on a line above the
  buttons; a narrower window (an iPad's Split View or
  Slide Over, a narrow desktop window) gives the right-hand group the rows
  under the transport, its buttons wrapping, so none is cut off at the edge.
  Choosers over the video keep clear of the bar's laid-out height and are
  never wider or taller than their room; their lists scroll. The guide's
  corner ‹ keeps clear of the leading safe area (an iPad window's controls).
- **Planned:** a list beside its details in Expanded, and collapsing the
  player's panels by class.

## What a presentation change keeps

**Planned** as a whole, with the current exceptions below. Collapsing or
expanding panes, resizing, rotating, entering split screen and switching input
keep:

- the selected content (channel, program, title, season, episode) by identity,
  never by row index;
- filters, sort and the scroll anchor;
- the playing stream on the same persistent player surface. The player view is
  resized and moved, never hidden, detached or re-parented;
- open details and their focused action.

They issue no playback command of their own.

- **Now:** resizing between expanded and compact windows keeps the focused
  guide channel or the focused preview, the poster browser's focused title and
  a show's selected episode with its details open, on the same player node, with
  no stream opened (`tests/app/pane_collapse.rs`).
- **Now:** on a tablet, rotating, half and third split-screen windows and
  system text up to 2.0 keep a playing movie full screen with its progress,
  and a show's details with the selected episode, on the same player view
  (never detached or re-parented) with no stream opened; the play bar's
  Back and play/pause and the details' Close stay inside the window
  (`tests/app/tablet_journey.rs`).
- **Now:** on desktop, full-screen playback keeps its player, stream and
  channel through the operating system's full screen, resizing to a narrow
  window, display scale and text size changes, and Back returns to the same
  channel (`tests/app/desktop_workflows.rs`).

- **Now:** the first key after touch can move focus to a visible row (see
  [Input modes](#input-modes)). With `guide.autoplay` on, focus that lands on a
  live program after a pane closes can tune it.

## Which player is heard

Exactly one player is heard at a time, and the viewer chooses it; focus
never moves the sound (#135). The state is the core's
(`iptv_presentation::heard`, kept by `iptv_presentation::multiview` and
`iptv_presentation::playback_windows`), by the surface's identity.

- **Now:** in multiview, moving the focus between tiles (the arrows, a
  tap, opening a tile's menu, changing its channel, enlarging or reducing
  it) never changes what is heard. A tile's menu offers **Listen to this**
  on every tile that is not heard, reached by OK on a remote and by a tap
  or click on touch and pointer; the pointer chrome also has a speaker
  button on those tiles. With two or more tiles the heard tile shows a
  speaker beside its channel and the others a muted speaker; a screen
  reader hears "· muted" after a muted tile's name.
- **Now:** entering multiview keeps the sound on the channel that was
  playing. Adding a tile does not take it, and changing the heard tile's
  channel keeps the sound on that tile. Removing the heard tile (its menu,
  its ✕, or its channel leaving the guide) gives the sound to the tile
  heard before it that is still there, else to the tile on the full-screen
  player, else to the first.
- **Now:** a full-screen detour plays the watched tile aloud; back on the
  grid the heard tile is heard again. Enlarging, reducing, reflowing and
  resizing keep the heard tile, and none of them opens a stream.
- **Now (Linux, macOS):** the main player and its player windows follow
  the same rule; see [desktop viewing](desktop.md#several-players).
- **Now:** the [interruption policy](#interruptions-and-the-background)
  pauses and resumes the heard player, whichever it is.
- **Now:** an optional setting, **Sound follows the selected tile**
  (Settings, Playback, under Multiview; off by default), for viewers who
  prefer the focused tile to be heard (#179). Its row is reached like
  every settings row, by a remote's OK, a tap, a click or the keyboard.
  With it on, each focus move of the viewer's in multiview (the arrows, a
  tap or a click on another tile, a tile added, or a picked channel
  already on another tile) makes that tile heard, as **Listen to this**
  would, and the speaker badge moves with it. Only a focus that moves
  takes the sound: opening a tile's menu, changing the focused tile's
  channel and an arrow at the grid's edge move nothing, and **Listen to
  this** still works. A full-screen
  detour, enlarging or reducing (also of another tile, which takes the
  focus) and removing a tile keep the rules above in both modes; focus
  left by them takes the sound only at the next focus move. Desktop
  player windows and picture in picture keep the explicit rule. The
  choice applies at the next focus move, also in a multiview already
  open.

## Text entry

- **Now:** while a text field has the focus, the keys typed go to it: a
  letter, a digit or Space that is a player shortcut elsewhere neither
  pauses, mutes nor zaps. A software keyboard keeps the field above it, in
  landscape, portrait and split screen, and its dismissal ends the editing
  ([keyboard dismissal](../crates/iptv-app/tests/app/keyboard_dismissal.rs)).
  After it, a hardware keyboard's first key and a touch each drive the
  results as their input mode does (`tests/app/tablet_journey.rs`).

## Interruptions and the background

The host reports what happens to the app's sound and window; one portable
policy decides what playback does (`iptv_presentation::interruption`). It only
pauses and resumes: it never opens, retunes or restarts a stream.

| Host report | Playback |
|---|---|
| Another app takes the audio (its own media) | Pauses; the viewer resumes |
| A call or a prompt takes the audio for a while | Pauses; resumes when it ends, unless the viewer acted on playback meanwhile (played, paused, sought, zapped, chose, stopped) |
| Headphones unplugged, a Bluetooth headset disconnected | Pauses; never resumes by itself |
| A notification's sound | Plays on, quieter (the system ducks it) |
| Home or another app, picture in picture off | Stops and closes the stream; on return it reopens what played, after fresh admission |
| Another app beside Patina takes the focus (iPad Split View, Slide Over, Stage Manager; the window stays visible) | Plays on; nothing pauses or reopens |
| Home with picture in picture on | Plays on in the window, on the same player and stream |
| The picture-in-picture window closed | Stops as above |

In multiview the policy pauses and resumes the heard tile (the one the
viewer chose with Listen to this, whatever has the focus; the others are
muted), as the tile's own pause.

| During a call | Playback |
|---|---|
| The viewer chooses Listen to this on another multiview tile | Stays silent until the call ends (Android: every player is silenced while the focus is lost for a while); the app pauses nothing more, and the call's end resumes nothing: the heard tile is the viewer's choice. Only moving the focus changes nothing |
| The viewer picks a channel a guard refuses (blocked, locked) | Nothing changes: the pause is still the call's, and its end resumes |
| Keys and controls that are no playback action (Info, menus, highlights) | Nothing changes |

- **Now (Android):** one audio focus for all the activity's players
  (`AudioFocusHost.kt`, its rules in `AudioFocusState.kt`): held while a
  player plays aloud (a source buffering or ready, set to play, not muted;
  not an ended or failed one); never taken back from another app until a
  player plays again; kept through a call while a stream is open, and
  after the call until the app has decided (2 s at most); given up when
  every stream is stopped, closed or in the background. The changes reach
  the app as the player's `audio` event. A stream the viewer stopped stays
  stopped whatever the focus does, and resuming opens no second provider
  connection (`tests/app/tablet_journey.rs`). The background stop and
  return, and picture in picture, as before (`PlayerComponent.kt`,
  `PipHost.kt`).
- **Now:** the Recordings screen states what limits a recording in the
  background, from what the host reports: precise background starts
  unavailable, a wake that could not be registered, or a device that must
  stay awake (`iptv_presentation::capabilities::recording_notice`).
- **Now (iPad):** the app runs in either orientation and in Split View,
  Slide Over and Stage Manager windows. Another app taking the focus beside
  Patina leaves the scene visible (inactive): playback carries on, with no
  pause and no new stream. When Patina leaves the screen (Home, the app
  switcher, a Slide Over window pushed away, another app taking the full
  screen) iOS sends it to the background: the player stops its stream there
  and on return asks for a freshly admitted one, opened once on the same
  player (`mpv/ios.rs`, `tests/app/ipad_windows.rs`).
- **Planned:** the picture-in-picture action says that closing its window
  stops playback; iPad's audio session (interruptions and route changes)
  under the same policy, and its foreground-only capture explained at the
  recording action.

## Empty and unavailable states

**Now.** Movies and Shows explain why they are empty, with one action that
fixes it. The state comes from every enabled playlist, the same ones the
catalog reads whichever playlist is on screen: each one's type, its own
movie and series import setting, a refresh running now, and its last
import's result, including a movie or series list that failed while its live
channels imported. When several playlists apply, the first matching row
below wins, after "some playlist has titles".

| State | Message says | Action |
|---|---|---|
| Import in progress | Importing movies and shows… | None; the list fills when it finishes |
| Import failed (whole import, or its movie or series list) | Last refresh failed, and why by error kind (no provider text) | Retry: runs that playlist's refresh again |
| Imports turned off for the source | Movie and series imports are off for this playlist | Playlist settings: opens that playlist's actions at its "Movies and series" switch |
| Imported, but the catalog is empty | No movies (or shows) found; refresh the playlist to look for new ones | Refresh now |
| No source can provide movies or series (M3U only, or none) | Movies and shows come from Xtream Codes and Stalker playlists; M3U has live channels only | Add playlist |

On a remote the action sits where the titles would be: Right from the
categories, then OK. If the action moves to another playlist while the remote
is on it, the remote goes back to the categories, so OK never acts on a
playlist the screen did not show. In a Medium or Expanded window with no
action (an import running, or titles only elsewhere) the remote never rests
on the empty place: an action that goes away (Refresh now started) sends it
to the categories, Right from the categories reaches Search and Sort, Down
from them stays, Left from Search returns to the categories, OK on a
category stays, and closing Search keeps the remote on it. Search closed
while its list is still being read keeps the remote on Search until the
list arrives, then moves it to the titles or the action if there is one. A
remote sent off the action's place this way stays off a later action, even
when the window becomes Compact, until it moves there itself. Once an action
shows, Right and Down reach it as before. In a Compact window (see
[Layout classes](#layout-classes)) the message and action take the width and
the categories give way. A remote or keyboard that was on the categories goes
to the action, or to the rail when there is no action (an import running) or
when it was just sent off another playlist's action; Left reaches the rail
and Right returns. An action that moves to another playlist, or goes away
(a refresh started), sends it to the rail, and if the window widens or the
text shrinks the rail stays on screen with the remote on it. Without an
action, Right from the rail reaches Search and Sort instead, and closing
Search keeps the remote there. In every window, a remote sent off an action,
or on Retry when the action moves, stays off the new one through a failed
read and its Retry, until it moves there itself (Right, or OK on a
category). The action stays inside a 400×600 window at Medium and Extra
large text, also with the test pseudolocale
(`tests/app/vod_empty_compact.rs`). The "Movies and series" switch keeps no
own value when it matches the global setting, and a deleted playlist's own
value goes with it. A search or filter that finds nothing says "No titles
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
  `cargo test -p iptv-core -p iptv-presentation` for Domain and interaction
  rules, the wasm builds of both crates and `cargo build -p iptv-app`.
- Which player is heard: presentation tests in `iptv_presentation::heard`,
  `iptv_presentation::multiview` (`heard_tests`, both with and without the sound
  following the selected tile) and `iptv_presentation::playback_windows`; app
  scenarios `multiview_heard` and, for windows, `desktop_workflows`.
- Choosing, staying and returning: app scenarios `personalization_overlay`,
  `tv_shared_controls`, `pip_control`, `cov_nav` and `cov_nav_touch`; on a
  remote's raw key phases, `tv_remote_journey`.
- Tablet gestures, the touch journey, rotation, split screen, large text,
  text entry and interruptions: `tablet_gestures` and `tablet_journey`,
  and the [issue #88 tablet validation](https://github.com/teng-lin/patina-iptv/blob/cb6b863b4635f40367ef5dce804d8fefe6ff0819/docs/validation/issue-88-tablet-journey.md).
- iPad portrait, resizable windows, Dynamic Type and another app beside:
  `ipad_windows`, and the
  [issue #138 iPad validation](https://github.com/teng-lin/patina-iptv/blob/cb6b863b4635f40367ef5dce804d8fefe6ff0819/docs/validation/issue-138-ipad-layouts.md).
- Pointer, keyboard and windows on desktops: `desktop_workflows`, and the
  [issue #89 desktop journey](https://github.com/teng-lin/patina-iptv/blob/cb6b863b4635f40367ef5dce804d8fefe6ff0819/docs/validation/issue-89-desktop-journey.md).
- Native behavior on Android TV and tablets: the
  [disposable Android validation](android-validation.md) on `tv36` and `tab36`.
  Record emulator and physical-device evidence separately, with the build, OS,
  input and viewport, using synthetic data only.
- The complete journey on TV, tablet and desktop, with the build, hosts,
  screenshots and open bugs: [issue #86 journey validation](https://github.com/teng-lin/patina-iptv/blob/cb6b863b4635f40367ef5dce804d8fefe6ff0819/docs/validation/issue-86-journey.md);
  the TV remote journey: [issue #87](https://github.com/teng-lin/patina-iptv/blob/cb6b863b4635f40367ef5dce804d8fefe6ff0819/docs/validation/issue-87-tv-journey.md).
